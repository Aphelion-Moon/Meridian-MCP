#[derive(Clone, Debug)]
pub struct ServerLimits {
    pub max_result_bytes: usize,
    pub max_blocking_jobs: usize,
    pub max_reference_results: usize,
    pub max_document_symbols: usize,
    pub max_fixture_text_file_bytes: u64,
    pub max_fixture_binary_file_bytes: u64,
    pub max_fixture_input_bytes: u64,
    pub max_dmi_files: usize,
    pub max_dmi_input_bytes: u64,
    pub max_dmi_file_bytes: u64,
    pub max_dmi_decoded_pixels: u64,
    pub max_dmi_metadata_bytes: usize,
    pub max_dmi_decoder_bytes: usize,
    pub max_dmi_scan_decoded_bytes: usize,
    pub max_dmi_scan_metadata_bytes: usize,
    pub max_dmi_scan_states: usize,
    pub max_dmi_scan_frames: usize,
    pub max_dmi_states: usize,
    pub max_dmi_frames: usize,
    pub max_dmi_cache_entries: usize,
    pub max_dmi_cache_bytes: usize,
    pub max_dmi_matches: usize,
    pub max_dmi_candidates: usize,
    pub max_map_differences: usize,
    pub max_map_cells: usize,
    pub max_render_pixels: u64,
    pub max_render_output_bytes: u64,
    pub max_render_files: usize,
    pub max_render_chunks: usize,
    pub max_docs_files: usize,
    pub max_docs_output_bytes: u64,
    pub max_docs_duration_ms: u64,
    pub max_debug_message_bytes: usize,
    pub max_debug_events: usize,
    pub max_debug_output_bytes: usize,
    pub max_debug_startup_ms: u64,
    pub max_debug_request_ms: u64,
    pub max_debug_frames: usize,
    pub max_debug_variables: usize,
}

pub const MAX_EVIDENCE_ARTIFACTS: usize = 32;
pub const MAX_EVIDENCE_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_EVIDENCE_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_EVIDENCE_ROWS: usize = 5_000_000;
pub const MAX_EVIDENCE_COLUMNS: usize = 512;
pub const MAX_EVIDENCE_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_EVIDENCE_STRING_BYTES: usize = 64 * 1024;
pub const MAX_EVIDENCE_GROUPS: usize = 100_000;
pub const MAX_EVIDENCE_PHASES: usize = 64;
pub const MAX_EVIDENCE_SELECTED_METRICS: usize = 64;
pub const MAX_EVIDENCE_RETURNED_GROUPS: usize = 1_000;
pub const MAX_EVIDENCE_COMPARISON_RUNS: usize = 20;

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_result_bytes: 1_048_576,
            max_blocking_jobs: 4,
            max_reference_results: 10_000,
            max_document_symbols: 20_000,
            max_fixture_text_file_bytes: 64 * 1024 * 1024,
            max_fixture_binary_file_bytes: 256 * 1024 * 1024,
            max_fixture_input_bytes: 1024 * 1024 * 1024,
            max_dmi_files: 20_000,
            max_dmi_input_bytes: 2 * 1024 * 1024 * 1024,
            max_dmi_file_bytes: 64 * 1024 * 1024,
            max_dmi_decoded_pixels: 64_000_000,
            max_dmi_metadata_bytes: 4 * 1024 * 1024,
            max_dmi_decoder_bytes: 256 * 1024 * 1024,
            max_dmi_scan_decoded_bytes: 512 * 1024 * 1024,
            max_dmi_scan_metadata_bytes: 32 * 1024 * 1024,
            max_dmi_scan_states: 100_000,
            max_dmi_scan_frames: 1_000_000,
            max_dmi_states: 100_000,
            max_dmi_frames: 1_000_000,
            max_dmi_cache_entries: 128,
            max_dmi_cache_bytes: 512 * 1024 * 1024,
            max_dmi_matches: 10_000,
            max_dmi_candidates: 2_000_000,
            max_map_differences: 100_000,
            max_map_cells: 16_777_216,
            max_render_pixels: 268_435_456,
            max_render_output_bytes: 512 * 1024 * 1024,
            max_render_files: 128,
            max_render_chunks: 512,
            max_docs_files: 100_000,
            max_docs_output_bytes: 1024 * 1024 * 1024,
            max_docs_duration_ms: 600_000,
            max_debug_message_bytes: 8 * 1024 * 1024,
            max_debug_events: 1_000,
            max_debug_output_bytes: 1024 * 1024,
            max_debug_startup_ms: 60_000,
            max_debug_request_ms: 30_000,
            max_debug_frames: 1_000,
            max_debug_variables: 10_000,
        }
    }
}

pub const MAX_EVIDENCE_GROUP_FIELDS: usize = 64;
pub const MAX_EVIDENCE_GROUP_FIELD_BYTES: usize = 256;
pub const MAX_EVIDENCE_PHASE_ID_BYTES: usize = 256;
