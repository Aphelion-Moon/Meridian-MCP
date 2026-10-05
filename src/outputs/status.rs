use schemars::JsonSchema;
use serde::Serialize;
#[derive(Serialize, JsonSchema)]
pub struct OptionalCapabilities {
    pub rift_build: &'static str,
    pub documentation: bool,
    pub debugger: bool,
    pub tracy: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct PrivateStateStatus {
    pub ready: bool,
    pub contents_exposed: bool,
    pub runtime_integrity_recovery: Vec<crate::runtime_integrity::RuntimeIntegritySummary>,
}
#[derive(Serialize, JsonSchema)]
pub struct ParsedStatus {
    #[serde(flatten)]
    pub identity: crate::identity::AnalysisIdentity,
    pub parsed: bool,
    pub environment_path: std::path::PathBuf,
    pub project_root: Option<std::path::PathBuf>,
    pub spacemandmm_revision: &'static str,
    pub spacemandmm_local_patch: &'static str,
    pub spacemandmm_local_patch_sha256: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct UnparsedStatus {
    pub parsed: bool,
    pub state_generation: u64,
    pub environment_path: Option<std::path::PathBuf>,
    pub project_root: Option<std::path::PathBuf>,
    pub spacemandmm_revision: Option<String>,
}
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AnalysisStatus {
    Parsed(ParsedStatus),
    Unparsed(UnparsedStatus),
}
#[derive(Serialize, JsonSchema)]
pub struct ServerStatusOutput {
    pub mcp_build: crate::build_identity::BuildIdentity,
    pub mode: &'static str,
    pub tool_profile: crate::ToolProfile,
    pub optional_capabilities: OptionalCapabilities,
    pub containment: crate::path_policy::PathPolicyStatus,
    pub private_state: PrivateStateStatus,
    pub analysis: AnalysisStatus,
    pub runtime: crate::state::RuntimeStatus,
}
