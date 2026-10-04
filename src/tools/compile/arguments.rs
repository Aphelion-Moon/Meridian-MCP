use super::response::ResponseOptions;
use crate::parameters::CompileParams;
use std::path::PathBuf;
pub(super) struct CompileOptions {
    pub dme_path: String,
    pub compiler_path: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub fixture_manifest_path: Option<PathBuf>,
    pub defines: Vec<String>,
    pub timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub capture_network: bool,
    pub response: ResponseOptions,
}
impl From<CompileParams> for CompileOptions {
    fn from(args: CompileParams) -> Self {
        Self {
            response: ResponseOptions::new(
                args.include_output,
                args.output_max_bytes,
                args.diagnostic_limit,
            ),
            dme_path: args.dme_path,
            compiler_path: args.compiler_path,
            working_directory: args.working_directory.map(PathBuf::from),
            fixture_manifest_path: args.fixture_manifest_path.map(PathBuf::from),
            defines: args.defines.unwrap_or_default(),
            timeout_ms: args.timeout_ms.unwrap_or(600_000),
            idle_timeout_ms: args
                .idle_timeout_ms
                .unwrap_or(super::DEFAULT_IDLE_TIMEOUT_MS),
            capture_network: args.capture_network.unwrap_or(false),
        }
    }
}
