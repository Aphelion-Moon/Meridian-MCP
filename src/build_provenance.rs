use crate::artifact::FileIdentity;
use crate::artifact_location::{canonical_location, legacy_folded_location_key, location_key};
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
    InProgress,
    Interrupted { code: String },
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

// A single replacement publishes the managed marker, attempt, and last build.
// Legacy location/build/attempt records remain read-only during a quiesced upgrade.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ArtifactState {
    schema: u32,
    artifact_key: String,
    record: Option<BuildRecord>,
    attempt: BuildAttempt,
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
        self.authorized_location(dmb_path)
            .and_then(|path| location_key(&path))
    }

    pub fn project_identity(&self, artifact_path: &Path) -> Result<ProjectBuildIdentity> {
        let (mut project, _) = self.project_and_relative(artifact_path)?;
        (project.head_revision, project.dirty) =
            crate::repository_roots::git_observation(&project.root);
        Ok(project)
    }

    fn validate_record(&self, record: &BuildRecord) -> Result<()> {
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
        Ok(())
    }

    /// Persist before spawning a writer. Dropping the caller leaves a durable
    /// non-success state; a completion must name this exact attempt.
    pub fn begin_attempt(
        &self,
        dmb_path: &Path,
        inputs: Vec<BuildInputIdentity>,
    ) -> Result<BuildAttempt> {
        let dmb_path = self.authorized_location(dmb_path)?;
        let key = location_key(&dmb_path)?;
        let transaction = self.state.transaction()?;
        let previous = self.load_state(&transaction, &dmb_path, &key)?;
        let attempt = BuildAttempt {
            schema: 1,
            attempt_id: random_id()?,
            artifact_key: key.clone(),
            outcome: BuildAttemptOutcome::InProgress,
            observed_inputs: inputs,
            retained_dmb_sha256: None,
            created_at_unix_ms: unix_ms(),
        };
        transaction.write_json_atomic(
            &state_path(&key),
            &ArtifactState {
                schema: 2,
                artifact_key: key,
                record: previous.and_then(|state| state.record),
                attempt: attempt.clone(),
            },
        )?;
        Ok(attempt)
    }

    pub fn finish_attempt(
        &self,
        attempt: &BuildAttempt,
        record: Option<&BuildRecord>,
    ) -> Result<()> {
        self.finish_attempt_checked(attempt, record, || None)
            .map(|_| ())
    }

    /// Check cancellation/deadline after acquiring the publication transaction.
    /// Interrupted finalization retains evidence and the previous successful
    /// record, but cannot install a newly verified completion.
    pub(crate) fn finish_attempt_checked(
        &self,
        attempt: &BuildAttempt,
        record: Option<&BuildRecord>,
        checkpoint: impl FnOnce() -> Option<&'static str>,
    ) -> Result<Option<&'static str>> {
        if attempt.schema != 1 || attempt.attempt_id.is_empty() || attempt.artifact_key.len() != 64
        {
            bail!("build attempt is invalid");
        }
        if matches!(attempt.outcome, BuildAttemptOutcome::InProgress) {
            bail!("attempt completion must have a terminal outcome");
        }
        if let Some(record) = record {
            self.validate_record(record)?;
            if record.artifact_key != attempt.artifact_key
                || !matches!(attempt.outcome, BuildAttemptOutcome::Succeeded)
            {
                bail!("build completion does not match its attempt");
            }
        } else if matches!(attempt.outcome, BuildAttemptOutcome::Succeeded) {
            bail!("verified completion requires a build record");
        }
        let transaction = self.state.transaction()?;
        let path = state_path(&attempt.artifact_key);
        let mut current: ArtifactState = transaction.read_json(&path)?;
        if current.schema != 2
            || current.artifact_key != attempt.artifact_key
            || current.attempt.attempt_id != attempt.attempt_id
            || !matches!(current.attempt.outcome, BuildAttemptOutcome::InProgress)
        {
            bail!("build attempt is no longer the current in-progress attempt");
        }
        let interruption = checkpoint();
        current.attempt = attempt.clone();
        if let Some(code) = interruption {
            if !matches!(attempt.outcome, BuildAttemptOutcome::Failed { .. }) {
                current.attempt.outcome = BuildAttemptOutcome::Unverified {
                    code: code.to_owned(),
                };
            }
        } else if let Some(record) = record {
            current.record = Some(record.clone());
        }
        transaction.write_json_atomic(&path, &current)?;
        Ok(interruption)
    }

    fn authorized_location(&self, path: &Path) -> Result<PathBuf> {
        let location = canonical_location(path)?;
        if location.exists() {
            Ok(self.policy.read_path(location)?)
        } else {
            self.policy.read_path(
                location
                    .parent()
                    .ok_or_else(|| anyhow!("artifact has no parent"))?,
            )?;
            Ok(location)
        }
    }

    fn load_state(
        &self,
        transaction: &crate::private_state::PrivateStateTransaction<'_>,
        dmb_path: &Path,
        key: &str,
    ) -> Result<Option<ArtifactState>> {
        if let Some(state) = transaction.read_json_optional::<ArtifactState>(&state_path(key))? {
            if state.schema != 2
                || state.artifact_key != key
                || state.attempt.artifact_key != key
                || state.attempt.schema != 1
                || state.attempt.attempt_id.is_empty()
            {
                bail!("managed artifact state is invalid; recovery or rebuild is required");
            }
            return Ok(Some(state));
        }
        if transaction.namespace_exists(&format!("artifacts-v2/{key}"))? {
            return Ok(Some(ArtifactState {
                schema: 2,
                artifact_key: key.to_owned(),
                record: None,
                attempt: BuildAttempt {
                    schema: 1,
                    attempt_id: "missing-state".to_owned(),
                    artifact_key: key.to_owned(),
                    outcome: BuildAttemptOutcome::Interrupted {
                        code: "managed_state_missing".to_owned(),
                    },
                    observed_inputs: Vec::new(),
                    retained_dmb_sha256: None,
                    created_at_unix_ms: unix_ms(),
                },
            }));
        }
        let mut location: Option<ArtifactLocation> =
            transaction.read_json_optional(&format!("locations/{key}.json"))?;
        if location.is_none() {
            let legacy_key = legacy_folded_location_key(dmb_path)?;
            if legacy_key != key {
                location =
                    transaction.read_json_optional(&format!("locations/{legacy_key}.json"))?;
            }
        }
        let (project, relative) = self.project_and_relative(dmb_path)?;
        let old_key = format!(
            "{:x}",
            Sha256::digest(format!("{}\n{}", project.repository_identity, relative).as_bytes())
        );
        let legacy_key = match &location {
            Some(location) if location.schema == 1 && location.artifact_key.len() == 64 => {
                &location.artifact_key
            }
            Some(_) => bail!(
                "managed artifact location record is invalid; recovery or rebuild is required"
            ),
            None => &old_key,
        };
        let mut record: Option<BuildRecord> =
            transaction.read_json_optional(&format!("builds/{legacy_key}.json"))?;
        if record.as_ref().is_some_and(|record| {
            !matches!(record.schema, 1 | 2)
                || record.record_id.is_empty()
                || record.artifact_key != *legacy_key
        }) {
            bail!("legacy build record is invalid; recovery or rebuild is required");
        }
        let mut attempt: Option<BuildAttempt> =
            transaction.read_json_optional(&format!("attempts/{legacy_key}.json"))?;
        if location.is_none() && record.is_none() && attempt.is_none() {
            return Ok(None);
        }
        if location.is_none() || record.is_none() {
            // An incomplete legacy publication is evidence of management, never
            // permission to use the unmanaged launch path.
            attempt = Some(BuildAttempt {
                schema: 1,
                attempt_id: "legacy-recovery-required".to_owned(),
                artifact_key: key.to_owned(),
                outcome: BuildAttemptOutcome::Interrupted {
                    code: "legacy_state_incomplete".to_owned(),
                },
                observed_inputs: Vec::new(),
                retained_dmb_sha256: None,
                created_at_unix_ms: unix_ms(),
            });
        }
        if let Some(record) = &mut record {
            record.artifact_key = key.to_owned();
        }
        let mut attempt = attempt.unwrap_or_else(|| BuildAttempt {
            schema: 1,
            attempt_id: "legacy-success".to_owned(),
            artifact_key: key.to_owned(),
            outcome: BuildAttemptOutcome::Succeeded,
            observed_inputs: Vec::new(),
            retained_dmb_sha256: None,
            created_at_unix_ms: record
                .as_ref()
                .map_or(0, |record| record.created_at_unix_ms),
        });
        if attempt.schema != 1 || attempt.attempt_id.is_empty() {
            bail!("legacy attempt is invalid; recovery is required");
        }
        if record
            .as_ref()
            .is_some_and(|record| attempt.created_at_unix_ms < record.created_at_unix_ms)
            && !matches!(attempt.outcome, BuildAttemptOutcome::Interrupted { .. })
        {
            attempt.outcome = BuildAttemptOutcome::Succeeded;
        }
        attempt.artifact_key = key.to_owned();
        Ok(Some(ArtifactState {
            schema: 2,
            artifact_key: key.to_owned(),
            record,
            attempt,
        }))
    }

    pub fn evaluate_launch(
        &self,
        dmb_path: &Path,
        require_verified: bool,
    ) -> Result<LaunchDecision> {
        let dmb_path = self.authorized_location(dmb_path)?;
        let requested_location = location_key(&dmb_path)?;
        let transaction = self.state.transaction()?;
        let Some(state) = self.load_state(&transaction, &dmb_path, &requested_location)? else {
            return Ok(unverified(require_verified));
        };
        drop(transaction);
        let attempt = state.attempt;
        let Some(record) = state.record else {
            let unverified = matches!(attempt.outcome, BuildAttemptOutcome::Unverified { .. });
            return Ok(LaunchDecision {
                status: if unverified {
                    ProvenanceStatus::Unverified
                } else {
                    ProvenanceStatus::Stale
                },
                allowed: unverified && !require_verified,
                record_id: None,
                reasons: vec![attempt_reason(&attempt, &dmb_path)],
            });
        };
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
        let (current_project, _) = self.project_and_relative(&dmb_path)?;
        if current_project.root != record.project.root
            || current_project.repository_identity != record.project.repository_identity
        {
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

        {
            if !matches!(attempt.outcome, BuildAttemptOutcome::Succeeded) {
                reasons.push(attempt_reason(&attempt, &dmb_path));
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

fn state_path(key: &str) -> String {
    format!("artifacts-v2/{key}/state.json")
}

fn attempt_reason(attempt: &BuildAttempt, path: &Path) -> ProvenanceReason {
    reason(
        match attempt.outcome {
            BuildAttemptOutcome::InProgress => "build_in_progress_or_interrupted",
            BuildAttemptOutcome::Interrupted { .. } => "build_recovery_required",
            BuildAttemptOutcome::Failed { .. } => "later_compile_failed",
            _ => "later_compile_unverified",
        },
        "the latest managed attempt did not establish verified completion",
        None,
        Some(path.to_owned()),
    )
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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

    pub fn finish_reason_checked(
        &self,
        checkpoint: impl Fn() -> Option<&'static str>,
    ) -> Option<&'static str> {
        for input in &self.inputs {
            if let Some(reason) = checkpoint() {
                return Some(reason);
            }
            let current = FileIdentity::capture(&input.path);
            if let Some(reason) = checkpoint() {
                return Some(reason);
            }
            if !matches!(current, Ok(current) if current.size == input.size
                && current.sha256 == input.sha256
                && input.resolved_path.as_ref().is_none_or(|path| *path == current.path))
            {
                return Some("build_inputs_changed");
            }
        }
        for path in &self.verification.absent_inputs {
            if let Some(reason) = checkpoint() {
                return Some(reason);
            }
            if std::fs::symlink_metadata(path).is_ok() {
                return Some("build_inputs_changed");
            }
        }
        if let Some(reason) = checkpoint() {
            return Some(reason);
        }
        let compiler = FileIdentity::capture(&self.compiler.path);
        if let Some(reason) = checkpoint() {
            return Some(reason);
        }
        if !matches!(compiler, Ok(current) if current.sha256 == self.compiler.sha256
            && current.size == self.compiler.size)
        {
            return Some("build_inputs_changed");
        }
        self.reason
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::tools::ToolExecutionContext;

    #[test]
    fn cancelled_publication_retains_outputs_and_previous_record_without_verified_completion() {
        let base = std::env::temp_dir().join(format!(
            "meridian-publication-{}-{}",
            std::process::id(),
            random_id().unwrap(),
        ));
        let root = base.join("workspace");
        let state_dir = base.join("state");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        let input = root.join("source.dm");
        let dmb = root.join("world.dmb");
        std::fs::write(&input, "/world\n").unwrap();
        std::fs::write(&dmb, "previous-output").unwrap();
        let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
        let state =
            Arc::new(PrivateStateStore::open(&state_dir, policy.effective_roots()).unwrap());
        let store = Arc::new(BuildProvenanceStore::new(state.clone(), policy.clone()));
        let mut context = ToolExecutionContext::with_features_and_state(
            crate::CapabilityMode::Development,
            policy,
            crate::RiftBuildAccess::Disabled,
            None,
            None,
            None,
            Some(state.clone()),
        );
        let (cancel, cancellation) = tokio::sync::watch::channel(false);
        context.cancellation = Some(cancellation);
        let inputs = vec![BuildInputIdentity::capture(&root, &input, "source").unwrap()];
        let mut record = BuildRecord {
            schema: 2,
            record_id: "previous-success".to_owned(),
            artifact_key: store.artifact_key(&dmb).unwrap(),
            mcp_build: crate::build_identity::current().clone(),
            compiler: FileIdentity::capture(&input).unwrap(),
            project: store.project_identity(&dmb).unwrap(),
            inputs: inputs.clone(),
            verification: None,
            dmb: FileIdentity::capture(&dmb).unwrap(),
            rsc: None,
            fixture_manifest_sha256: None,
            created_at_unix_ms: unix_ms(),
        };
        let mut previous = store.begin_attempt(&dmb, inputs.clone()).unwrap();
        previous.outcome = BuildAttemptOutcome::Succeeded;
        store.finish_attempt(&previous, Some(&record)).unwrap();
        std::fs::write(&dmb, "produced-output").unwrap();
        record.record_id = "candidate-success".to_owned();
        record.dmb = FileIdentity::capture(&dmb).unwrap();
        let mut attempt = store.begin_attempt(&dmb, inputs).unwrap();
        attempt.outcome = BuildAttemptOutcome::Succeeded;
        attempt.retained_dmb_sha256 = Some(record.dmb.sha256.clone());
        let key = attempt.artifact_key.clone();
        let publication = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(state_dir.join(".meridian-mcp.lock"))
            .unwrap();
        publication.lock().unwrap();
        let candidate_store = store.clone();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let publisher = std::thread::spawn(move || {
            candidate_store
                .finish_attempt_checked(&attempt, Some(&record), || {
                    context.finalization_reason(deadline)
                })
                .unwrap()
        });
        cancel.send(true).unwrap();
        drop(publication);
        assert_eq!(publisher.join().unwrap(), Some("request_cancelled"));
        let current: ArtifactState = state.read_json(&state_path(&key)).unwrap();
        assert_eq!(
            current.attempt.outcome,
            BuildAttemptOutcome::Unverified {
                code: "request_cancelled".to_owned(),
            }
        );
        assert!(current.attempt.retained_dmb_sha256.is_some());
        assert!(!current.attempt.observed_inputs.is_empty());
        assert_eq!(current.record.unwrap().record_id, "previous-success");
        std::fs::remove_dir_all(base).unwrap();
    }
}
