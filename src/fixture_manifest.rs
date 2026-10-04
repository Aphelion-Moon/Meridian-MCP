use crate::PathPolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FIXTURE_ID_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_INPUTS: usize = 10_000;
const MAX_REQUIRED_PROCS: usize = 1_000;
const MAX_REQUIRED_TOKENS: usize = 1_000;
const MAX_TOKEN_BYTES: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureManifestDocument {
    pub schema: u32,
    pub fixture_id: String,
    pub dme_path: String,
    pub dmb_path: String,
    #[serde(default)]
    pub rsc_path: Option<String>,
    pub inputs: Vec<FixtureInputDocument>,
    #[serde(default)]
    pub required_procs: Vec<RequiredProcDocument>,
    #[serde(default)]
    pub required_tokens: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureInputDocument {
    pub path: String,
    pub role: FixtureInputRole,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureInputRole {
    Source,
    GeneratedBinding,
    NativeModule,
    ServiceExecutable,
    Configuration,
}

impl FixtureInputRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::GeneratedBinding => "generated_binding",
            Self::NativeModule => "native_module",
            Self::ServiceExecutable => "service_executable",
            Self::Configuration => "configuration",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredProcDocument {
    pub path: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifiedFixtureInput {
    pub relative_path: String,
    pub canonical_path: PathBuf,
    pub role: FixtureInputRole,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifiedFixtureManifest {
    pub manifest_path: PathBuf,
    pub fixture_root: PathBuf,
    pub fixture_id: String,
    pub dme_path: PathBuf,
    pub dmb_path: PathBuf,
    pub rsc_path: Option<PathBuf>,
    pub inputs: Vec<VerifiedFixtureInput>,
    pub required_procs: Vec<RequiredProcDocument>,
    pub required_tokens: Vec<String>,
    pub identity_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum FixtureManifestError {
    #[error("fixture manifest policy rejected the path: {0}")]
    Policy(#[from] crate::PolicyError),
    #[error("fixture manifest I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("fixture manifest JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("fixture manifest is invalid: {0}")]
    Invalid(String),
}

pub struct FixtureManifest;

impl FixtureManifest {
    pub fn load(
        policy: &PathPolicy,
        path: &Path,
    ) -> Result<VerifiedFixtureManifest, FixtureManifestError> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(invalid("manifest must be a regular non-symlink file"));
        }
        if metadata.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("manifest exceeds the 4 MiB limit"));
        }
        let manifest_path = policy.read_path(path)?;
        let document: FixtureManifestDocument =
            serde_json::from_reader(File::open(&manifest_path)?)?;
        validate_document(&document)?;

        let fixture_root = manifest_path
            .parent()
            .expect("a canonical manifest file has a parent")
            .to_owned();
        let dme_path = resolve_existing(policy, &fixture_root, &document.dme_path)?;
        let dmb_path = resolve_output(&fixture_root, &document.dmb_path)?;
        let rsc_path = document
            .rsc_path
            .as_deref()
            .map(|path| resolve_output(&fixture_root, path))
            .transpose()?;

        let mut normalized_paths = BTreeSet::new();
        let mut canonical_paths = BTreeSet::new();
        let mut inputs = Vec::with_capacity(document.inputs.len());
        for input in &document.inputs {
            let normalized = validate_relative_path(&input.path)?;
            if !normalized_paths.insert(path_identity(&normalized)) {
                return Err(invalid("input paths are duplicated after normalization"));
            }
            let requested = fixture_root.join(&normalized);
            let metadata = std::fs::symlink_metadata(&requested)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid(format!(
                    "input must be a regular non-symlink file: {}",
                    input.path
                )));
            }
            let canonical_path = policy.read_path(&requested)?;
            let canonical_key = path_identity(&canonical_path.to_string_lossy());
            if !canonical_paths.insert(canonical_key) {
                return Err(invalid(
                    "multiple inputs resolve to the same canonical file",
                ));
            }
            inputs.push(VerifiedFixtureInput {
                relative_path: normalized,
                canonical_path: canonical_path.clone(),
                role: input.role,
                size: metadata.len(),
                sha256: hash_file(&canonical_path)?,
            });
        }
        inputs.sort_by(|left, right| {
            (left.role.as_str(), &left.relative_path)
                .cmp(&(right.role.as_str(), &right.relative_path))
        });

        let identity_sha256 = manifest_identity(&document, &inputs)?;
        Ok(VerifiedFixtureManifest {
            manifest_path,
            fixture_root,
            fixture_id: document.fixture_id,
            dme_path,
            dmb_path,
            rsc_path,
            inputs,
            required_procs: document.required_procs,
            required_tokens: document.required_tokens,
            identity_sha256,
        })
    }
}

fn validate_document(document: &FixtureManifestDocument) -> Result<(), FixtureManifestError> {
    if document.schema != 1 {
        return Err(invalid("schema must be 1"));
    }
    if document.fixture_id.is_empty() || document.fixture_id.len() > MAX_FIXTURE_ID_BYTES {
        return Err(invalid("fixture_id must contain 1-128 bytes"));
    }
    validate_relative_path(&document.dme_path)?;
    validate_relative_path(&document.dmb_path)?;
    if let Some(path) = &document.rsc_path {
        validate_relative_path(path)?;
    }
    if document.inputs.is_empty() || document.inputs.len() > MAX_INPUTS {
        return Err(invalid("inputs must contain 1-10000 entries"));
    }
    if document.required_procs.len() > MAX_REQUIRED_PROCS {
        return Err(invalid("required_procs exceeds 1000 entries"));
    }
    if document.required_tokens.len() > MAX_REQUIRED_TOKENS {
        return Err(invalid("required_tokens exceeds 1000 entries"));
    }
    for required in &document.required_procs {
        if !required.path.starts_with('/')
            || required.path.len() > MAX_PATH_BYTES
            || required.arguments.len() > 1_000
            || required.arguments.iter().any(|argument| {
                argument.is_empty()
                    || argument.len() > MAX_PATH_BYTES
                    || !argument
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
        {
            return Err(invalid("required proc contract is invalid"));
        }
    }
    if document
        .required_tokens
        .iter()
        .any(|token| token.is_empty() || token.len() > MAX_TOKEN_BYTES)
    {
        return Err(invalid("required token must contain 1-4096 bytes"));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<String, FixtureManifestError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.contains('\\')
        || path.contains("://")
        || path.contains(['*', '?', '[', ']'])
        || path.starts_with('/')
        || path.contains(':')
    {
        return Err(invalid(
            "paths must be bounded forward-slash relative paths",
        ));
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(invalid(
            "paths cannot contain empty, dot, or parent components",
        ));
    }
    Ok(components.join("/"))
}

fn resolve_existing(
    policy: &PathPolicy,
    root: &Path,
    relative: &str,
) -> Result<PathBuf, FixtureManifestError> {
    let normalized = validate_relative_path(relative)?;
    let requested = root.join(normalized);
    let metadata = std::fs::symlink_metadata(&requested)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("fixture DME must be a regular non-symlink file"));
    }
    Ok(policy.read_path(requested)?)
}

fn resolve_output(root: &Path, relative: &str) -> Result<PathBuf, FixtureManifestError> {
    let normalized = validate_relative_path(relative)?;
    Ok(root.join(normalized))
}

fn hash_file(path: &Path) -> Result<String, FixtureManifestError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn manifest_identity(
    document: &FixtureManifestDocument,
    inputs: &[VerifiedFixtureInput],
) -> Result<String, FixtureManifestError> {
    let portable_inputs = inputs
        .iter()
        .map(|input| {
            serde_json::json!({
                "path": input.relative_path,
                "role": input.role,
                "size": input.size,
                "sha256": input.sha256,
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema": document.schema,
        "fixture_id": document.fixture_id,
        "dme_path": document.dme_path,
        "dmb_path": document.dmb_path,
        "rsc_path": document.rsc_path,
        "inputs": portable_inputs,
        "required_procs": document.required_procs,
        "required_tokens": document.required_tokens,
    }))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn invalid(message: impl Into<String>) -> FixtureManifestError {
    FixtureManifestError::Invalid(message.into())
}

fn path_identity(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}
