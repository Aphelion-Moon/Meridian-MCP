use crate::process::TerminationReason;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ReplyOmission {
    Field {
        field_omitted: bool,
        json_bytes: usize,
    },
    Rows {
        total: usize,
        returned: usize,
        omitted: usize,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct StreamReplySummary {
    pub included: bool,
    pub available_utf8_bytes: usize,
    pub returned_utf8_bytes: usize,
    pub omitted_utf8_bytes: usize,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct OutputSummary {
    pub stdout: StreamReplySummary,
    pub stderr: StreamReplySummary,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DiagnosticDetailSummary {
    pub returned: usize,
    pub omitted: u64,
    pub truncated_messages: usize,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct BuildDiagnosticSummary {
    pub scope: String,
    pub errors: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<u64>,
    pub analysis_complete: bool,
    pub output_complete: bool,
    pub capture_complete: bool,
    pub oversized_lines: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors_detail: Option<DiagnosticDetailSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings_detail: Option<DiagnosticDetailSummary>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CompilerDiagnosticData {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_utf8_bytes: Option<usize>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReplyArtifact {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<std::path::PathBuf>,
    pub exists: bool,
    pub size: Option<u64>,
    pub modified_unix_ms: Option<u128>,
    pub sha256: Option<String>,
}
impl From<crate::artifact::ArtifactSnapshot> for ReplyArtifact {
    fn from(a: crate::artifact::ArtifactSnapshot) -> Self {
        Self {
            path: Some(a.path),
            exists: a.exists,
            size: a.size,
            modified_unix_ms: a.modified_unix_ms,
            sha256: a.sha256,
        }
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReplyArtifactPair {
    pub dmb: ReplyArtifact,
    pub rsc: ReplyArtifact,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ControllerTimeout {
    pub inner_wall_seconds: u64,
    pub inner_idle_seconds: u64,
    pub outer_idle_timeout_ms: u128,
}
pub(crate) fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CompileData {
    pub success: bool,
    pub compiler_succeeded: bool,
    pub diagnostic_analysis_error: Option<String>,
    pub artifact_error: Option<String>,
    pub timed_out: bool,
    pub idle: bool,
    pub termination: TerminationReason,
    pub process_termination: TerminationReason,
    pub finalization_interruption: Option<String>,
    pub duration_ms: u128,
    pub timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dme_argument: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_working_directory: Option<String>,
    pub dmb_exists: bool,
    pub dmb_updated: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub dmb_path: Option<Option<String>>,
    pub artifact_before: ReplyArtifact,
    pub artifact_after: ReplyArtifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub working_directory: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defines: Option<Vec<String>>,
    pub errors: Vec<CompilerDiagnosticData>,
    pub warnings: Vec<CompilerDiagnosticData>,
    pub diagnostic_summary: BuildDiagnosticSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub stdout_truncated_bytes: u64,
    pub stderr_truncated_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_audit: Option<crate::network_audit::NetworkAuditReport>,
    pub provenance_status: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub build_record_id: Option<Option<String>>,
    pub attempt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance_reasons: Option<Vec<BuildProvenanceReason>>,
    pub retained_dmb_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<OutputSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_omissions: Option<BTreeMap<String, ReplyOmission>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RiftCompileData<D = String> {
    pub success: bool,
    pub code: Option<String>,
    pub evidence: crate::tools::rift::BuildEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_build_entrypoint: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rift_build_entrypoint: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dme_path: Option<std::path::PathBuf>,
    pub state_generation: Option<u64>,
    pub byond_version: Option<String>,
    pub startup_ceiling: String,
    pub network_mode: String,
    pub force_rebuild: bool,
    pub capture_network: bool,
    pub timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub controller_timeout: ControllerTimeout,
    pub duration_ms: u128,
    pub termination: TerminationReason,
    pub process_termination: TerminationReason,
    pub finalization_interruption: Option<String>,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub stdout_truncated_bytes: u64,
    pub stderr_truncated_bytes: u64,
    pub diagnostics: Vec<D>,
    pub diagnostic_summary: BuildDiagnosticSummary,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_evidence: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub rift_result: Option<Option<crate::tools::rift::RiftResultRecord>>,
    pub artifact_before: ReplyArtifactPair,
    pub artifact_after: ReplyArtifactPair,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_audit: Option<crate::network_audit::NetworkAuditReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    pub recovery: String,
    pub provenance_status: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub build_record_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance_reasons: Option<Vec<BuildProvenanceReason>>,
    pub retained_dmb_sha256: Option<String>,
    pub attempt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<OutputSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_omissions: Option<BTreeMap<String, ReplyOmission>>,
}
#[derive(Serialize, Deserialize, JsonSchema, Default)]
pub struct DocsData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_directory: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper: Option<std::path::PathBuf>,
    pub source_revision: String,
    pub installed: bool,
    pub success: bool,
    pub cleanup_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub stdout_truncated_bytes: u64,
    pub stderr_truncated_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination: Option<TerminationReason>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub exit_code: Option<Option<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_restored: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_directory: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staging_directory: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staging_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staging_cleanup_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_error_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staging_cleanup_error_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<OutputSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_omissions: Option<BTreeMap<String, ReplyOmission>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct HelperProjection {
    #[serde(flatten)]
    pub data: DocsData,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
}
pub type CompileOutput = CompileData;
pub type RiftCompileOutput = RiftCompileData;
pub type GenerateDocsOutput = crate::result::Success<DocsData>;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct BuildProvenanceReason {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub role: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub path: Option<Option<std::path::PathBuf>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RiftDiagnosticData {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_utf8_bytes: Option<usize>,
}

#[derive(Default, Serialize, JsonSchema)]
pub struct DocsInstallation {
    pub installed: bool,
    pub success: bool,
    pub cleanup_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_restored: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_directory: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}
impl DocsData {
    pub(crate) fn installation(&mut self, installation: DocsInstallation) {
        self.installed = installation.installed;
        self.success = installation.success;
        self.cleanup_complete = installation.cleanup_complete;
        self.code = installation.code;
        self.message = installation.message;
        self.previous_restored = installation.previous_restored;
        self.backup_directory = installation.backup_directory;
        self.backup_name = installation.backup_name;
        self.cleanup_error = installation.cleanup_error;
        self.recovery = installation.recovery;
    }
}
