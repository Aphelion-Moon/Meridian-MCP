use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ANNOTATIONS: usize = 32;
pub const MAX_ANNOTATION_KEY_BYTES: usize = 64;
pub const MAX_ANNOTATION_VALUE_BYTES: usize = 512;
pub const MAX_IDENTITY_VALUE_BYTES: usize = 512;
pub const MAX_FEATURES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct HelperIdentity {
    pub source_revision: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct NativeModuleIdentity {
    pub name: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ExecutableIdentity {
    pub schema: u32,
    pub executable_id: String,
    pub repository_revision: Option<String>,
    pub repository_dirty_digest: String,
    pub dmb_sha256: String,
    pub rsc_sha256: Option<String>,
    pub byond_version: String,
    pub byond_executable_sha256: String,
    pub native_modules: Vec<NativeModuleIdentity>,
    pub helper_identity: HelperIdentity,
    pub hook_identity: HelperIdentity,
    pub startup_mode: String,
    pub launch_parameters_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_record_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct WorkloadInput {
    pub map: Option<String>,
    pub seed: Option<String>,
    pub configuration_profile: Option<String>,
    #[serde(default)]
    pub feature_set: Vec<String>,
    pub scenario: Option<String>,
    pub external_run_id: Option<String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct WorkloadIdentity {
    pub workload_id: String,
    pub map: Option<String>,
    pub seed: Option<String>,
    pub configuration_profile: Option<String>,
    pub feature_set: Vec<String>,
    pub scenario: Option<String>,
    pub external_run_id: Option<String>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ExperimentIdentity {
    pub experiment_id: String,
    pub executable: ExecutableIdentity,
    pub workload: WorkloadIdentity,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct ExperimentLaunchManifest {
    pub schema: u32,
    pub experiment_name: Option<String>,
    pub meridian_mcp_build: crate::build_identity::BuildIdentity,
    pub executable: ExecutableIdentity,
    pub workload_draft: WorkloadInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_configuration: Option<crate::tracy_runtime_config::RuntimeConfigurationIdentity>,
}

#[derive(Clone, Debug)]
pub struct ExperimentState {
    pub directory: std::path::PathBuf,
    pub launch_manifest_sha256: String,
    pub launch_manifest_path: std::path::PathBuf,
    pub identity_manifest_path: std::path::PathBuf,
    pub final_manifest_path: std::path::PathBuf,
    pub executable: ExecutableIdentity,
    pub workload_draft: WorkloadInput,
    pub runtime_configuration: Option<crate::tracy_runtime_config::RuntimeConfigurationIdentity>,
    pub runtime_wake: Option<serde_json::Value>,
    pub locked_identity: Option<ExperimentIdentity>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExperimentError {
    #[error("{field} exceeds its fixed byte limit")]
    ValueTooLong { field: String },
    #[error("{field} contains a control character, environment expansion, or absolute path")]
    UnsafeValue { field: String },
    #[error("annotations exceed the fixed entry limit")]
    TooManyAnnotations,
    #[error("annotation key must be 1-64 lowercase ASCII snake-case bytes")]
    InvalidAnnotationKey,
    #[error("feature_set exceeds the fixed entry limit")]
    TooManyFeatures,
    #[error("feature_set contains a duplicate canonical value")]
    DuplicateFeature,
    #[error("capture workload conflicts with the immutable experiment identity")]
    WorkloadConflict,
    #[error("identity serialization failed: {0}")]
    Serialize(String),
}

pub fn canonical_sha256<T: Serialize>(value: &T) -> Result<String, ExperimentError> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| ExperimentError::Serialize(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn finalize_executable(
    mut identity: ExecutableIdentity,
) -> Result<ExecutableIdentity, ExperimentError> {
    identity.executable_id.clear();
    identity.native_modules.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.sha256.cmp(&right.sha256))
    });
    identity.executable_id = canonical_sha256(&identity)?;
    Ok(identity)
}

pub fn validate_workload(mut input: WorkloadInput) -> Result<WorkloadInput, ExperimentError> {
    for (field, value) in [
        ("map", input.map.as_deref()),
        ("seed", input.seed.as_deref()),
        (
            "configuration_profile",
            input.configuration_profile.as_deref(),
        ),
        ("scenario", input.scenario.as_deref()),
        ("external_run_id", input.external_run_id.as_deref()),
    ] {
        if let Some(value) = value {
            validate_value(field, value, MAX_IDENTITY_VALUE_BYTES)?;
        }
    }
    if input.feature_set.len() > MAX_FEATURES {
        return Err(ExperimentError::TooManyFeatures);
    }
    let mut features = BTreeSet::new();
    for value in &input.feature_set {
        validate_value("feature_set", value, MAX_IDENTITY_VALUE_BYTES)?;
        if !features.insert(value.clone()) {
            return Err(ExperimentError::DuplicateFeature);
        }
    }
    input.feature_set = features.into_iter().collect();
    if input.annotations.len() > MAX_ANNOTATIONS {
        return Err(ExperimentError::TooManyAnnotations);
    }
    for (key, value) in &input.annotations {
        if key.is_empty()
            || key.len() > MAX_ANNOTATION_KEY_BYTES
            || !key.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit() && index > 0
                    || byte == b'_' && index > 0
            })
            || key.ends_with('_')
            || key.contains("__")
        {
            return Err(ExperimentError::InvalidAnnotationKey);
        }
        validate_value("annotation value", value, MAX_ANNOTATION_VALUE_BYTES)?;
    }
    Ok(input)
}

pub fn workload_identity(input: WorkloadInput) -> Result<WorkloadIdentity, ExperimentError> {
    let input = validate_workload(input)?;
    let workload_id = canonical_sha256(&input)?;
    Ok(WorkloadIdentity {
        workload_id,
        map: input.map,
        seed: input.seed,
        configuration_profile: input.configuration_profile,
        feature_set: input.feature_set,
        scenario: input.scenario,
        external_run_id: input.external_run_id,
        annotations: input.annotations,
    })
}

pub fn bind_workload(
    draft: &WorkloadInput,
    capture: &WorkloadInput,
) -> Result<WorkloadIdentity, ExperimentError> {
    let draft = validate_workload(draft.clone())?;
    let capture = validate_workload(capture.clone())?;
    let merged = WorkloadInput {
        map: merge_field(&draft.map, &capture.map)?,
        seed: merge_field(&draft.seed, &capture.seed)?,
        configuration_profile: merge_field(
            &draft.configuration_profile,
            &capture.configuration_profile,
        )?,
        feature_set: merge_vec(&draft.feature_set, &capture.feature_set)?,
        scenario: merge_field(&draft.scenario, &capture.scenario)?,
        external_run_id: merge_field(&draft.external_run_id, &capture.external_run_id)?,
        annotations: merge_annotations(&draft.annotations, &capture.annotations)?,
    };
    workload_identity(merged)
}

pub fn experiment_identity(
    executable: ExecutableIdentity,
    workload: WorkloadIdentity,
) -> Result<ExperimentIdentity, ExperimentError> {
    let experiment_id = canonical_sha256(&(
        executable.executable_id.as_str(),
        workload.workload_id.as_str(),
    ))?;
    Ok(ExperimentIdentity {
        experiment_id,
        executable,
        workload,
    })
}

pub fn verify_locked_workload(
    locked: &WorkloadIdentity,
    supplied: &WorkloadInput,
) -> Result<(), ExperimentError> {
    let supplied = validate_workload(supplied.clone())?;
    for (actual, expected) in [
        (&supplied.map, &locked.map),
        (&supplied.seed, &locked.seed),
        (
            &supplied.configuration_profile,
            &locked.configuration_profile,
        ),
        (&supplied.scenario, &locked.scenario),
        (&supplied.external_run_id, &locked.external_run_id),
    ] {
        if actual
            .as_ref()
            .is_some_and(|value| Some(value) != expected.as_ref())
        {
            return Err(ExperimentError::WorkloadConflict);
        }
    }
    if !supplied.feature_set.is_empty() && supplied.feature_set != locked.feature_set {
        return Err(ExperimentError::WorkloadConflict);
    }
    if supplied
        .annotations
        .iter()
        .any(|(key, value)| locked.annotations.get(key) != Some(value))
    {
        return Err(ExperimentError::WorkloadConflict);
    }
    Ok(())
}

fn merge_field(
    existing: &Option<String>,
    supplied: &Option<String>,
) -> Result<Option<String>, ExperimentError> {
    match (existing, supplied) {
        (Some(left), Some(right)) if left != right => Err(ExperimentError::WorkloadConflict),
        (Some(value), _) | (None, Some(value)) => Ok(Some(value.clone())),
        (None, None) => Ok(None),
    }
}

fn merge_vec(existing: &[String], supplied: &[String]) -> Result<Vec<String>, ExperimentError> {
    if !existing.is_empty() && !supplied.is_empty() && existing != supplied {
        return Err(ExperimentError::WorkloadConflict);
    }
    Ok(if existing.is_empty() {
        supplied.to_vec()
    } else {
        existing.to_vec()
    })
}

fn merge_annotations(
    existing: &BTreeMap<String, String>,
    supplied: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ExperimentError> {
    let mut merged = existing.clone();
    for (key, value) in supplied {
        if merged.get(key).is_some_and(|current| current != value) {
            return Err(ExperimentError::WorkloadConflict);
        }
        merged.insert(key.clone(), value.clone());
    }
    Ok(merged)
}

fn validate_value(field: &str, value: &str, maximum_bytes: usize) -> Result<(), ExperimentError> {
    if value.len() > maximum_bytes {
        return Err(ExperimentError::ValueTooLong {
            field: field.to_owned(),
        });
    }
    let lower = value.to_ascii_lowercase();
    let looks_absolute = std::path::Path::new(value).is_absolute()
        || value.starts_with('/')
        || value.starts_with("\\\\")
        || value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'\\' | b'/');
    if value.chars().any(char::is_control)
        || lower.contains("$env:")
        || value.contains('%')
        || looks_absolute
    {
        return Err(ExperimentError::UnsafeValue {
            field: field.to_owned(),
        });
    }
    Ok(())
}
