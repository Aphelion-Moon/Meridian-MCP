use crate::artifact::FileIdentity;
use crate::build_identity::BuildIdentity;
use crate::{PathPolicy, PrivateStateStore};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectBuildIdentity {
    pub root: PathBuf,
    pub repository_identity: String,
    pub head_revision: Option<String>,
    pub dirty: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildInputIdentity {
    pub path: PathBuf,
    #[serde(default)]
    pub resolved_path: Option<PathBuf>,
    pub relative_path: String,
    pub role: String,
    pub size: u64,
    pub sha256: String,
}

impl BuildInputIdentity {
    pub fn capture(root: &Path, path: &Path, role: impl Into<String>) -> Result<Self> {
        let root = root.canonicalize()?;
        let identity = FileIdentity::capture(path)?;
        let relative_path =
            normalize_relative(identity.path.strip_prefix(&root).with_context(|| {
                format!("build input is outside project root: {}", path.display())
            })?);
        Ok(Self {
            path: identity.path,
            resolved_path: None,
            relative_path,
            role: role.into(),
            size: identity.size,
            sha256: identity.sha256,
        })
    }

    pub fn capture_authorized(
        policy: &PathPolicy,
        root: &Path,
        path: &Path,
        role: impl Into<String>,
    ) -> Result<Self> {
        let resolved = policy.read_path(path)?;
        let identity = FileIdentity::capture(&resolved)?;
        Ok(Self {
            path: path.to_owned(),
            resolved_path: Some(identity.path),
            relative_path: path
                .strip_prefix(root)
                .map(normalize_relative)
                .unwrap_or_else(|_| "<authorized-external>".to_owned()),
            role: role.into(),
            size: identity.size,
            sha256: identity.sha256,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BuildVerification {
    pub method: String,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub absent_inputs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BuildRecord {
    pub schema: u32,
    pub record_id: String,
    pub artifact_key: String,
    pub mcp_build: BuildIdentity,
    pub compiler: FileIdentity,
    pub project: ProjectBuildIdentity,
    pub inputs: Vec<BuildInputIdentity>,
    #[serde(default)]
    pub verification: Option<BuildVerification>,
    pub dmb: FileIdentity,
    pub rsc: Option<FileIdentity>,
    pub fixture_manifest_sha256: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BuildAttemptOutcome {
    Succeeded,
    Failed { code: String },
    Unverified { code: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BuildAttempt {
    pub schema: u32,
    pub attempt_id: String,
    pub artifact_key: String,
    pub outcome: BuildAttemptOutcome,
    pub observed_inputs: Vec<BuildInputIdentity>,
    pub retained_dmb_sha256: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceStatus {
    Verified,
    Unverified,
    Stale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProvenanceReason {
    pub code: String,
    pub message: String,
    pub role: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LaunchDecision {
    pub status: ProvenanceStatus,
    pub allowed: bool,
    pub record_id: Option<String>,
    pub reasons: Vec<ProvenanceReason>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LaunchProvenance {
    pub status: ProvenanceStatus,
    pub build_record_id: Option<String>,
    pub dmb_sha256: String,
    pub warnings: Vec<ProvenanceReason>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ArtifactLocation {
    schema: u32,
    artifact_key: String,
}

pub struct BuildProvenanceStore {
    state: Arc<PrivateStateStore>,
    policy: PathPolicy,
}

impl BuildProvenanceStore {
    pub fn new(state: Arc<PrivateStateStore>, policy: PathPolicy) -> Self {
        Self { state, policy }
    }

    pub fn artifact_key(&self, dmb_path: &Path) -> Result<String> {
        let (project, relative) = self.project_and_relative(dmb_path)?;
        Ok(format!(
            "{:x}",
            Sha256::digest(format!("{}\n{}", project.repository_identity, relative).as_bytes())
        ))
    }

    pub fn project_identity(&self, artifact_path: &Path) -> Result<ProjectBuildIdentity> {
        self.project_and_relative(artifact_path)
            .map(|(project, _)| project)
    }

    pub fn record_success(&self, record: &BuildRecord) -> Result<()> {
        if !matches!(record.schema, 1 | 2)
            || record.record_id.is_empty()
            || record.artifact_key.len() != 64
        {
            bail!("build record is invalid");
        }
        let current_key = self.artifact_key(&record.dmb.path)?;
        if current_key != record.artifact_key {
            bail!("build record artifact key does not match the current project identity");
        }
        self.state
            .write_json_atomic(&format!("builds/{}.json", record.artifact_key), record)?;
        self.state.write_json_atomic(
            &format!("locations/{}.json", location_key(&record.dmb.path)?),
            &ArtifactLocation {
                schema: 1,
                artifact_key: record.artifact_key.clone(),
            },
        )?;
        Ok(())
    }

    pub fn record_attempt(&self, attempt: &BuildAttempt) -> Result<()> {
        if attempt.schema != 1 || attempt.attempt_id.is_empty() || attempt.artifact_key.len() != 64
        {
            bail!("build attempt is invalid");
        }
        self.state
            .write_json_atomic(&format!("attempts/{}.json", attempt.artifact_key), attempt)?;
        Ok(())
    }

    pub fn evaluate_launch(
        &self,
        dmb_path: &Path,
        require_verified: bool,
    ) -> Result<LaunchDecision> {
        let dmb_path = if dmb_path.exists() {
            self.policy.read_path(dmb_path)?
        } else {
            let parent = dmb_path
                .parent()
                .ok_or_else(|| anyhow!("artifact path has no parent"))?;
            self.policy.read_path(parent)?.join(
                dmb_path
                    .file_name()
                    .ok_or_else(|| anyhow!("artifact path has no file name"))?,
            )
        };
        let requested_location = location_key(&dmb_path)?;
        let location_path = format!("locations/{requested_location}.json");
        let transaction = self.state.read_transaction()?;
        let mut location: Option<ArtifactLocation> =
            transaction.read_json_optional(&location_path)?;
        // Older Unix records folded case too. Retain their stale checks during
        // upgrade, but bind every recovered record to the requested artifact.
        if location.is_none() && !cfg!(windows) {
            let legacy_key = location_key_with_case(&dmb_path, true)?;
            if legacy_key != requested_location {
                location =
                    transaction.read_json_optional(&format!("locations/{legacy_key}.json"))?;
            }
        }
        let Some(location) = location else {
            return Ok(unverified(require_verified));
        };
        if location.schema != 1 || location.artifact_key.len() != 64 {
            bail!("managed artifact location record is invalid");
        }
        let record: BuildRecord =
            transaction.read_json(&format!("builds/{}.json", location.artifact_key))?;
        let attempt: Option<BuildAttempt> =
            transaction.read_json_optional(&format!("attempts/{}.json", record.artifact_key))?;
        drop(transaction);

        let mut reasons = Vec::new();
        if location_key(&record.dmb.path)? != requested_location {
            return Ok(LaunchDecision {
                status: ProvenanceStatus::Stale,
                allowed: false,
                record_id: Some(record.record_id),
                reasons: vec![reason(
                    "artifact_location_changed",
                    "the managed location points to a different artifact",
                    None,
                    Some(dmb_path),
                )],
            });
        }
        let current_project = self.project_identity(&dmb_path)?;
        if current_project != record.project {
            reasons.push(reason(
                "repository_identity_changed",
                "the current project identity differs from the recorded successful build",
                None,
                Some(current_project.root.clone()),
            ));
        }
        for input in &record.inputs {
            match FileIdentity::capture(&input.path) {
                Ok(current)
                    if current.size == input.size
                        && current.sha256 == input.sha256
                        && input
                            .resolved_path
                            .as_ref()
                            .is_none_or(|path| *path == current.path) => {}
                Ok(_) => reasons.push(reason(
                    "input_changed",
                    "a recorded build input changed",
                    Some(input.role.clone()),
                    Some(input.path.clone()),
                )),
                Err(_) => reasons.push(reason(
                    "input_missing",
                    "a recorded build input is missing or not a regular file",
                    Some(input.role.clone()),
                    Some(input.path.clone()),
                )),
            }
        }
        if let Some(verification) = &record.verification {
            for path in &verification.absent_inputs {
                if std::fs::symlink_metadata(path).is_ok() {
                    reasons.push(reason(
                        "input_appeared",
                        "an absent build configuration input appeared",
                        None,
                        Some(path.clone()),
                    ));
                }
            }
        }
        compare_output(
            &record.dmb,
            "dmb_changed",
            "the managed DMB changed",
            &mut reasons,
        );
        if let Some(rsc) = &record.rsc {
            compare_output(rsc, "rsc_changed", "the managed RSC changed", &mut reasons);
        }

        if let Some(attempt) = attempt {
            if attempt.created_at_unix_ms >= record.created_at_unix_ms
                && !matches!(attempt.outcome, BuildAttemptOutcome::Succeeded)
            {
                reasons.push(reason(
                    if matches!(attempt.outcome, BuildAttemptOutcome::Failed { .. }) {
                        "later_compile_failed"
                    } else {
                        "later_compile_unverified"
                    },
                    "a later compile attempt did not establish verified provenance",
                    None,
                    Some(dmb_path.clone()),
                ));
            }
        }

        if reasons.is_empty()
            && (record.schema != 2
                || record.verification.as_ref().is_none_or(|proof| {
                    proof.method != "literal_dm_closure_v1" || proof.arguments.is_empty()
                }))
        {
            Ok(LaunchDecision {
                status: ProvenanceStatus::Unverified,
                allowed: !require_verified,
                record_id: Some(record.record_id),
                reasons: vec![reason(
                    "unsupported_build_verification",
                    "legacy or unsupported build evidence cannot prove effective compiler inputs",
                    None,
                    None,
                )],
            })
        } else if reasons.is_empty() {
            Ok(LaunchDecision {
                status: ProvenanceStatus::Verified,
                allowed: true,
                record_id: Some(record.record_id),
                reasons,
            })
        } else {
            Ok(LaunchDecision {
                status: ProvenanceStatus::Stale,
                allowed: false,
                record_id: Some(record.record_id),
                reasons,
            })
        }
    }

    fn project_and_relative(&self, artifact_path: &Path) -> Result<(ProjectBuildIdentity, String)> {
        let artifact = if artifact_path.exists() {
            self.policy.read_path(artifact_path)?
        } else {
            let parent = artifact_path
                .parent()
                .ok_or_else(|| anyhow!("artifact path has no parent"))?;
            let parent = self.policy.read_path(parent)?;
            parent.join(
                artifact_path
                    .file_name()
                    .ok_or_else(|| anyhow!("artifact path has no file name"))?,
            )
        };
        let root = self
            .policy
            .effective_roots()
            .iter()
            .filter(|root| artifact.starts_with(&root.path))
            .max_by_key(|root| root.path.components().count())
            .ok_or_else(|| anyhow!("artifact is outside effective roots"))?;
        let relative = normalize_relative(artifact.strip_prefix(&root.path)?);
        let repository_identity = root
            .repository_identity
            .as_ref()
            .map(|identity| identity.digest.clone())
            .unwrap_or_else(|| {
                format!(
                    "{:x}",
                    Sha256::digest(root.path.to_string_lossy().as_bytes())
                )
            });
        Ok((
            ProjectBuildIdentity {
                root: root.path.clone(),
                repository_identity,
                head_revision: root.head_revision.clone(),
                dirty: root.dirty,
            },
            relative,
        ))
    }
}

fn compare_output(
    recorded: &FileIdentity,
    code: &str,
    message: &str,
    reasons: &mut Vec<ProvenanceReason>,
) {
    if !matches!(
        FileIdentity::capture(&recorded.path),
        Ok(current) if current.size == recorded.size && current.sha256 == recorded.sha256
    ) {
        reasons.push(reason(code, message, None, Some(recorded.path.clone())));
    }
}

fn unverified(require_verified: bool) -> LaunchDecision {
    LaunchDecision {
        status: ProvenanceStatus::Unverified,
        allowed: !require_verified,
        record_id: None,
        reasons: vec![reason(
            "no_build_record",
            "no managed successful build record exists for this artifact",
            None,
            None,
        )],
    }
}

fn reason(
    code: impl Into<String>,
    message: impl Into<String>,
    role: Option<String>,
    path: Option<PathBuf>,
) -> ProvenanceReason {
    ProvenanceReason {
        code: code.into(),
        message: message.into(),
        role,
        path,
    }
}

fn location_key(path: &Path) -> Result<String> {
    location_key_with_case(path, cfg!(windows))
}

fn location_key_with_case(path: &Path, fold_case: bool) -> Result<String> {
    let path = if path.exists() {
        path.canonicalize()?
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("artifact path has no parent"))?
            .canonicalize()?;
        parent.join(
            path.file_name()
                .ok_or_else(|| anyhow!("artifact path has no file name"))?,
        )
    };
    let path = path.to_string_lossy();
    let identity = if fold_case {
        path.to_ascii_lowercase()
    } else {
        path.into_owned()
    };
    Ok(format!("{:x}", Sha256::digest(identity.as_bytes())))
}

fn normalize_relative(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Pre-spawn evidence for the deliberately limited literal DM include grammar.
pub(crate) struct PreparedBuild {
    pub inputs: Vec<BuildInputIdentity>,
    pub compiler: FileIdentity,
    pub verification: BuildVerification,
    pub reason: Option<&'static str>,
}

impl PreparedBuild {
    pub fn capture(
        policy: &PathPolicy,
        snapshot: Option<&crate::analysis_snapshot::AnalysisSnapshot>,
        fixture: Option<&crate::fixture_manifest::VerifiedFixtureManifest>,
        dme: &Path,
        compiler: &Path,
        arguments: Vec<String>,
        working_directory: PathBuf,
    ) -> Result<Self> {
        let mut prepared = Self {
            inputs: Vec::new(),
            compiler: FileIdentity::capture(compiler)?,
            verification: BuildVerification {
                method: "literal_dm_closure_v1".to_owned(),
                arguments,
                working_directory,
                absent_inputs: Vec::new(),
            },
            reason: None,
        };
        let root = dme
            .parent()
            .ok_or_else(|| anyhow!("environment has no parent"))?;
        let matching = snapshot.filter(|snapshot| snapshot.environment_path == dme);
        if matching.is_none() {
            prepared.reason = Some("matching_snapshot_required");
        }
        if prepared.verification.arguments.len() != 1 {
            prepared.reason = Some("effective_defines_not_proved");
        }
        let mut pending = vec![dme.to_owned()];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(path) = pending.pop() {
            if visited.len() >= 10_000 {
                prepared.reason = Some("build_input_limit");
                break;
            }
            if !visited.insert(path.clone()) {
                continue;
            }
            let input = match BuildInputIdentity::capture_authorized(policy, root, &path, "source")
            {
                Ok(input) => input,
                Err(_) => {
                    prepared.reason = Some("build_input_unavailable");
                    continue;
                }
            };
            if matching.is_some_and(|snapshot| {
                !snapshot
                    .source_inputs()
                    .iter()
                    .any(|item| item == &path || Some(item) == input.resolved_path.as_ref())
            }) {
                prepared.reason = Some("parser_closure_changed");
            }
            let text = std::fs::read_to_string(&path);
            prepared.inputs.push(input.clone());
            let Ok(text) = text else {
                prepared.reason = Some("source_encoding_not_proved");
                continue;
            };
            if format!("{:x}", Sha256::digest(text.as_bytes())) != input.sha256 {
                prepared.reason = Some("build_inputs_changed");
            }
            // Reject ambiguous lexical forms instead of implementing a second DM preprocessor.
            if text.contains(['\\', '\'', '\0']) || !text.is_ascii() {
                prepared.reason = Some("compiler_resource_or_lexical_closure_not_proved");
                continue;
            }
            let mut block_comment = false;
            for line in text.lines() {
                let line = line.trim();
                if block_comment || line.starts_with("/*") {
                    let body = if block_comment { line } else { &line[2..] };
                    if body.contains(['#', '"'])
                        || body.contains("/*")
                        || body
                            .find("*/")
                            .is_some_and(|end| !body[end + 2..].trim().is_empty())
                    {
                        prepared.reason = Some("comment_lexical_closure_not_proved");
                    }
                    block_comment = !body.ends_with("*/");
                    continue;
                }
                if line.contains("/*") || line.contains("*/") {
                    prepared.reason = Some("comment_lexical_closure_not_proved");
                    continue;
                }
                if !line.contains('#') {
                    continue;
                }
                if let Some(define) = line.strip_prefix("#define ") {
                    let fields = define.split_whitespace().collect::<Vec<_>>();
                    if fields.len() == 2
                        && fields[0].bytes().enumerate().all(|(index, byte)| {
                            byte == b'_'
                                || byte.is_ascii_alphabetic()
                                || (index > 0 && byte.is_ascii_digit())
                        })
                        && !fields[1].is_empty()
                        && fields[1].bytes().all(|byte| byte.is_ascii_digit())
                    {
                        continue;
                    }
                }
                let include = line
                    .strip_prefix("#include \"")
                    .and_then(|value| value.strip_suffix('"'));
                let Some(include) =
                    include.filter(|value| !value.contains(['"', '#']) && !value.is_empty())
                else {
                    prepared.reason = Some("preprocessor_closure_not_proved");
                    continue;
                };
                let included = path.parent().unwrap_or(root).join(include);
                if !matches!(
                    included.extension().and_then(|value| value.to_str()),
                    Some("dm" | "dme")
                ) {
                    prepared.reason = Some("compiler_map_skin_script_closure_not_proved");
                    continue;
                }
                pending.push(included);
            }
            if block_comment {
                prepared.reason = Some("comment_lexical_closure_not_proved");
            }
        }
        if let Some(snapshot) = matching {
            for path in snapshot.source_inputs() {
                if prepared.inputs.iter().any(|input| &input.path == path) {
                    continue;
                }
                match BuildInputIdentity::capture_authorized(policy, root, path, "analysis_input") {
                    Ok(input) => prepared.inputs.push(input),
                    Err(_) => prepared.reason = Some("build_input_unavailable"),
                }
            }
            for path in snapshot.source_fingerprint.discovery_paths() {
                match std::fs::symlink_metadata(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        prepared.verification.absent_inputs.push(path.clone())
                    }
                    Ok(_) => match BuildInputIdentity::capture_authorized(
                        policy,
                        root,
                        path,
                        "configuration",
                    ) {
                        Ok(input) => prepared.inputs.push(input),
                        Err(_) => prepared.reason = Some("build_input_unavailable"),
                    },
                    Err(_) => prepared.reason = Some("build_input_unavailable"),
                }
            }
        }
        if let Some(fixture) = fixture {
            for (path, role) in std::iter::once((&fixture.manifest_path, "fixture_manifest")).chain(
                fixture
                    .inputs
                    .iter()
                    .map(|input| (&input.canonical_path, input.role.as_str())),
            ) {
                match BuildInputIdentity::capture_authorized(policy, root, path, role) {
                    Ok(input) => prepared.inputs.push(input),
                    Err(_) => prepared.reason = Some("build_input_unavailable"),
                }
            }
        }
        Ok(prepared)
    }

    pub fn finish_reason(&self) -> Option<&'static str> {
        if self.inputs.iter().any(|input| !matches!(FileIdentity::capture(&input.path), Ok(current) if current.size == input.size && current.sha256 == input.sha256 && input.resolved_path.as_ref().is_none_or(|path| *path == current.path)))
            || self.verification.absent_inputs.iter().any(|path| std::fs::symlink_metadata(path).is_ok())
            || !matches!(FileIdentity::capture(&self.compiler.path), Ok(current) if current.sha256 == self.compiler.sha256 && current.size == self.compiler.size)
        { return Some("build_inputs_changed"); }
        self.reason
    }
}
