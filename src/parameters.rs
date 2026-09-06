use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;

pub const RIFT_DEFAULT_TIMEOUT_MS: u64 = 1_800_000;
pub const RIFT_MAX_TIMEOUT_MS: u64 = 1_800_000;
pub const RIFT_DEFAULT_IDLE_TIMEOUT_MS: u64 = 120_000;
pub const RIFT_MAX_IDLE_TIMEOUT_MS: u64 = 900_000;
pub const RIFT_MIN_TIMEOUT_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RiftNetworkMode {
    Offline,
    Allow,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RiftCompileParams {
    pub network_mode: Option<RiftNetworkMode>,
    pub timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub capture_network: bool,
    #[serde(default)]
    pub force_rebuild: bool,
    pub fixture_manifest_path: Option<PathBuf>,
}

impl RiftCompileParams {
    pub fn network_mode(&self) -> RiftNetworkMode {
        self.network_mode.unwrap_or(RiftNetworkMode::Offline)
    }

    pub fn validated_timeouts(&self) -> Result<(u64, u64), &'static str> {
        let timeout_ms = self.timeout_ms.unwrap_or(RIFT_DEFAULT_TIMEOUT_MS);
        let idle_timeout_ms = self.idle_timeout_ms.unwrap_or(RIFT_DEFAULT_IDLE_TIMEOUT_MS);
        if !(RIFT_MIN_TIMEOUT_MS..=RIFT_MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err("timeout_ms must be between 1000 and 1800000");
        }
        if !(RIFT_MIN_TIMEOUT_MS..=RIFT_MAX_IDLE_TIMEOUT_MS).contains(&idle_timeout_ms) {
            return Err("idle_timeout_ms must be between 1000 and 900000");
        }
        Ok((timeout_ms, idle_timeout_ms))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ParseEnvironmentParams {
    pub dme_path: PathBuf,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTypeParams {
    pub type_path: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProcParams {
    pub type_path: String,
    pub proc_name: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVarParams {
    pub type_path: String,
    pub var_name: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTypesParams {
    pub prefix: Option<String>,
    pub max_depth: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchSymbolsParams {
    pub query: String,
    pub kind: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchContextParams {
    pub query: String,
    pub kind: Option<String>,
    pub type_prefix: Option<String>,
    pub file_filter: Option<String>,
    pub limit: Option<usize>,
    pub include_source: Option<bool>,
    pub max_source_lines: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckErrorsParams {
    pub file_path: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDefinitionParams {
    pub type_path: String,
    pub member_name: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentSymbolsParams {
    pub file_path: PathBuf,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub detail: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindReferencesParams {
    pub type_path: String,
    pub member_name: Option<String>,
    pub kind: Option<String>,
    pub include_declaration: Option<bool>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub detail: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindImplementationsParams {
    pub type_path: String,
    pub member_name: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub detail: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompileParams {
    pub dme_path: PathBuf,
    pub compiler_path: Option<PathBuf>,
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub defines: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub capture_network: bool,
    pub fixture_manifest_path: Option<PathBuf>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenderMapParams {
    pub dmm_path: PathBuf,
    pub z_level: Option<usize>,
    pub output_path: Option<PathBuf>,
    #[serde(default)]
    pub overwrite: bool,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MapInfoParams {
    pub dmm_path: PathBuf,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindOnMapParams {
    pub dmm_path: PathBuf,
    pub type_path: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunParams {
    pub dmb_path: PathBuf,
    pub port: Option<u16>,
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub daemon_args: Vec<String>,
    pub wait_for: Option<String>,
    #[serde(default)]
    pub wait_regex: bool,
    pub startup_timeout_ms: Option<u64>,
    #[serde(default)]
    pub require_verified_provenance: bool,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureSyncParams {
    pub fixture_manifest_path: PathBuf,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitForOutputParams {
    pub pattern: String,
    #[serde(default)]
    pub regex: bool,
    pub timeout_ms: Option<u64>,
}
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EmptyParams {}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TopicParams {
    pub topic: String,
    pub timeout_ms: Option<u64>,
}
