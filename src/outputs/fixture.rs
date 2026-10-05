use schemars::JsonSchema;
use serde::Serialize;
#[derive(Serialize, JsonSchema)]
pub struct FixtureIssue {
    pub code: &'static str,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_arguments: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_arguments: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_arguments_omitted: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_arguments_omitted: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_truncated: Option<bool>,
}
#[derive(Default, Serialize, JsonSchema)]
pub struct IssuesSummary {
    pub total: usize,
    pub returned: usize,
    pub omitted: usize,
}
#[derive(Default, Serialize, JsonSchema)]
pub struct FixtureSyncOutput {
    pub classification: &'static str,
    pub validation_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture_manifest_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_path: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmb_path: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance_status: Option<crate::build_provenance::ProvenanceStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_record_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance_reasons: Option<Vec<crate::build_provenance::ProvenanceReason>>,
    pub truncated: bool,
    pub issues_summary: IssuesSummary,
    pub issues: Vec<FixtureIssue>,
}
