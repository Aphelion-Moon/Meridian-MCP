use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Serialize, JsonSchema)]
pub struct SourceCorrelation {
    #[serde(rename = "match")]
    pub match_kind: String,
    pub path: std::path::PathBuf,
    pub line: u64,
    pub state_generation: u64,
    pub analysis: crate::identity::AnalysisIdentity,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyRangeCounts {
    pub raw: u64,
    pub intersecting: u64,
    pub complete: u64,
    pub partial_first: u64,
    pub partial_last: u64,
    pub spanning: u64,
    pub invalid: u64,
    pub excluded: u64,
    pub analyzed: u64,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyZoneStatistics {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub count: u64,
    pub inclusive_ns: i64,
    pub self_ns: i64,
    pub mean_ns: i64,
    pub min_ns: i64,
    pub max_ns: i64,
    pub p50_ns: i64,
    pub p95_ns: i64,
    pub p99_ns: i64,
    pub self_p50_ns: i64,
    pub self_p95_ns: i64,
    pub self_p99_ns: i64,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub source_correlation: Option<SourceCorrelation>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyZoneComparison {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub inclusive_delta_ns: i64,
    pub self_delta_ns: i64,
    pub count_delta: i64,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub source_correlation: Option<SourceCorrelation>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyHotspotsStatistics {
    pub items: Vec<TracyZoneStatistics>,
    pub truncated: bool,
    pub limit: u64,
    pub span_ns: i64,
    pub counts: TracyRangeCounts,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyZoneStatisticsResult {
    pub items: Vec<TracyZoneStatistics>,
    pub truncated: bool,
    pub counts: TracyRangeCounts,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyFrameStatistics {
    pub frame_count: u64,
    pub span_ns: i64,
    pub mean_ns: i64,
    pub min_ns: i64,
    pub max_ns: i64,
    pub p50_ns: i64,
    pub p95_ns: i64,
    pub p99_ns: i64,
    pub counts: TracyRangeCounts,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct TracyComparisonStatistics {
    pub items: Vec<TracyZoneComparison>,
    pub truncated: bool,
    pub limit: u64,
}
// Common metadata owns top-level truncation. Nested statistics retains the
// native helper flag, so the generated flattened object has one required key.
#[derive(Serialize, JsonSchema)]
pub struct TracyHotspotsFields {
    pub items: Vec<TracyZoneStatistics>,
    pub limit: u64,
    pub span_ns: i64,
    pub counts: TracyRangeCounts,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyZoneFields {
    pub items: Vec<TracyZoneStatistics>,
    pub counts: TracyRangeCounts,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyComparisonFields {
    pub items: Vec<TracyZoneComparison>,
    pub limit: u64,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct TracyAnalysisData<T, N = T> {
    #[serde(flatten)]
    pub native: N,
    pub helper_revision: String,
    pub schema: u32,
    pub protocol_version: Option<u32>,
    pub statistics: T,
    pub warnings: Vec<String>,
    pub identity_verification: String,
    pub window_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_iteration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<crate::tracy_artifact::RawRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<crate::tracy_artifact::ComparisonCompatibility>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct CollectorQueueHealth {
    pub capacity: u64,
    pub depth: u64,
    pub high_water: u64,
    pub tail_refresh_count: u64,
    pub saturation_count: u64,
    pub dropped_events: u64,
    pub produced_events: u64,
    pub consumed_events: u64,
    pub last_producer_progress_raw: u64,
    pub hook_installed: bool,
    pub prologue_validated: bool,
    pub byond_build: u32,
    pub offset_table_identity: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct CollectorStatus {
    pub state: String,
    pub worker_generation: u64,
    pub producer_progress: u64,
    pub capture_count: u64,
    pub worker_attached: bool,
    pub worker_purpose: Option<String>,
    pub transition_retry_count: u64,
    pub last_transition_error: Option<String>,
    pub recovery_required: bool,
    pub queue_health: Option<CollectorQueueHealth>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CollectorValidation {
    pub valid: bool,
    pub raw_begin: u64,
    pub raw_end: u64,
    pub trace_begin_ns: i64,
    pub trace_end_ns: i64,
    pub nanoseconds_per_tick: f64,
    pub wall_span_seconds: f64,
    pub requested_wall_seconds: f64,
    pub measured_wall_seconds: f64,
    pub wall_tolerance_seconds: f64,
    pub producer_progress_shortfall_seconds: f64,
    pub complete_frames: u64,
    pub partial_frames: u64,
    pub zones: u64,
    pub source_files: u64,
    pub queue: CollectorQueueHealth,
    pub error_codes: Vec<String>,
    pub warning_codes: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CollectorCapture {
    pub frame_count: u64,
    pub zone_count: u64,
    pub span_ns: i64,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
    pub validation: CollectorValidation,
    pub phase: String,
    pub phase_iteration: u32,
    pub session: CollectorStatus,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WakeAttempt {
    pub attempt: u32,
    pub topic_processed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<usize>,
    pub producer_progress_before: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer_progress_after_wake: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer_progress_sustained: Option<u64>,
    pub sustained: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WakeClientEvidence {
    pub pid: Option<u32>,
    pub sha256: Option<String>,
    pub producer_progress_before: u64,
    pub producer_progress_after_connect: Option<u64>,
    pub producer_progress_sustained: Option<u64>,
    pub sustained: bool,
    pub timeout_ms: u64,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RuntimeWake {
    pub strategy: String,
    pub initialization_marker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle_ms: Option<u64>,
    pub attempts: Vec<WakeAttempt>,
    #[serde(
        default,
        deserialize_with = "crate::outputs::build::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub wake_client: Option<Option<WakeClientEvidence>>,
    pub topic_processed: bool,
    pub sustained_producer_progress: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer_progress: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_client_error: Option<String>,
}
impl RuntimeWake {
    pub(crate) fn decode(value: serde_json::Value) -> anyhow::Result<Self> {
        let mut reply: Self = serde_json::from_value(value)?;
        reply.attempts.truncate(3);
        for attempt in &mut reply.attempts {
            if let Some(error) = &mut attempt.error {
                *error = crate::result::bounded_text(error, 4096, 4096, false).into();
            }
        }
        if let Some(error) = &mut reply.wake_client_error {
            *error = crate::result::bounded_text(error, 4096, 4096, false).into();
        }
        Ok(reply)
    }
}
#[derive(Serialize, JsonSchema)]
pub struct TracyPrepareData {
    pub state: &'static str,
    pub artifact: crate::atomic_output::OutputArtifact,
    pub source_revision: String,
    pub protocol_version: Option<u32>,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyLaunchData {
    pub lifecycle: &'static str,
    pub profiler_port: u16,
    pub collector: Option<CollectorStatus>,
    pub executable_identity: Option<crate::tracy_experiment::ExecutableIdentity>,
    pub runtime_configuration: Option<crate::tracy_runtime_config::RuntimeConfigurationIdentity>,
    pub runtime_wake: Option<RuntimeWake>,
    pub integrity_checkpoint: Option<crate::workspace_integrity::IntegrityCheckpoint>,
    pub integrity_journal: Option<crate::workspace_integrity::IntegrityJournalSummary>,
    pub launch_provenance: crate::LaunchProvenance,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyCaptureData {
    pub artifact: crate::atomic_output::OutputArtifact,
    pub sidecar: crate::atomic_output::OutputArtifact,
    pub capture: CollectorCapture,
    pub network_audit: crate::network_audit::NetworkEvidence,
    pub helper_revision: String,
    pub protocol_version: Option<u32>,
    pub integrity_checkpoint: Option<crate::workspace_integrity::IntegrityCheckpoint>,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyStatusData {
    pub running: bool,
    pub runtime_kind: Option<crate::state::RuntimeKind>,
    pub game_port: Option<u16>,
    pub profiler_port: Option<u16>,
    pub pid: Option<u32>,
    pub last_exit_code: Option<i32>,
    pub recent_output: Vec<String>,
    pub capture_active: bool,
    pub capture_output_path: Option<std::path::PathBuf>,
    pub last_capture_error: Option<String>,
    pub collector_phase: Option<crate::tracy_collector::TracySessionPhase>,
    pub collector_status: Option<CollectorStatus>,
    pub runtime_wake: Option<RuntimeWake>,
    pub integrity_journal: Option<crate::workspace_integrity::IntegrityJournalSummary>,
    pub collector_stderr_tail: Vec<String>,
    pub collector_exit_code: Option<i32>,
    pub launch_provenance: Option<crate::LaunchProvenance>,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyStopData {
    pub lifecycle: &'static str,
    pub runtime_was_running: bool,
    pub experiment_manifest: Option<crate::atomic_output::OutputArtifact>,
    pub pre_stop_integrity_checkpoint: Option<crate::workspace_integrity::IntegrityCheckpoint>,
    pub post_stop_integrity_checkpoint: Option<crate::workspace_integrity::IntegrityCheckpoint>,
    pub integrity_journal: Option<crate::workspace_integrity::IntegrityJournalSummary>,
    pub launch_provenance: Option<crate::LaunchProvenance>,
}
#[derive(Serialize, JsonSchema)]
pub struct ZoneDistribution {
    pub distribution: crate::tracy_statistics::DistributionSummary,
    pub noise: crate::tracy_statistics::NoiseEnvelope,
}
#[derive(Serialize, JsonSchema)]
pub struct TracyControlStatsData {
    pub schema: u32,
    pub input_count: usize,
    pub valid_count: usize,
    pub incomplete_count: usize,
    pub establishes_control_baseline: bool,
    pub compatibility: crate::tracy_artifact::ComparisonCompatibility,
    pub frame_percentile: String,
    pub frame_time: crate::tracy_statistics::DistributionSummary,
    pub zones: BTreeMap<String, ZoneDistribution>,
    pub noise: crate::tracy_statistics::NoiseEnvelope,
}
pub type TracyHotspotsOutput =
    crate::result::Success<TracyAnalysisData<TracyHotspotsStatistics, TracyHotspotsFields>>;
pub type TracyZoneOutput =
    crate::result::Success<TracyAnalysisData<TracyZoneStatisticsResult, TracyZoneFields>>;
pub type TracyFrameStatsOutput = crate::result::Success<TracyAnalysisData<TracyFrameStatistics>>;
pub type TracyCompareOutput =
    crate::result::Success<TracyAnalysisData<TracyComparisonStatistics, TracyComparisonFields>>;
pub type TracyPrepareOutput = crate::result::Success<TracyPrepareData>;
pub type TracyLaunchOutput = crate::result::Success<TracyLaunchData>;
pub type TracyCaptureOutput = crate::result::Success<TracyCaptureData>;
pub type TracyStatusOutput = crate::result::Success<TracyStatusData>;
pub type TracyStopOutput = crate::result::Success<TracyStopData>;
pub type TracyControlStatsOutput = crate::result::Success<TracyControlStatsData>;

impl<T> TracyAnalysisData<T> {
    pub(crate) fn into_projection(self) -> TracyAnalysisData<T, T::Fields>
    where
        T: NativeStatistics,
    {
        TracyAnalysisData {
            native: self.native.into_fields(),
            statistics: self.statistics,
            helper_revision: self.helper_revision,
            schema: self.schema,
            protocol_version: self.protocol_version,
            warnings: self.warnings,
            identity_verification: self.identity_verification,
            window_source: self.window_source,
            experiment_id: self.experiment_id,
            capture_id: self.capture_id,
            phase: self.phase,
            phase_iteration: self.phase_iteration,
            range: self.range,
            compatibility: self.compatibility,
        }
    }
}
pub(crate) trait NativeStatistics {
    type Fields: Serialize;
    fn into_fields(self) -> Self::Fields;
    fn bound(&mut self, budget: &mut crate::outputs::budget::Budget);
    fn correlate(&mut self, snapshot: Option<&crate::analysis_snapshot::AnalysisSnapshot>);
    fn truncated(&self) -> bool;
}
fn correlation(
    snapshot: Option<&crate::analysis_snapshot::AnalysisSnapshot>,
    file: &str,
    line: u32,
) -> Option<SourceCorrelation> {
    let snapshot = snapshot?;
    let root = snapshot.environment_path.parent()?;
    let reported = std::path::Path::new(file);
    let candidate = if reported.is_absolute() {
        reported.to_owned()
    } else {
        root.join(reported)
    };
    let path = candidate.canonicalize().ok()?;
    if !path.starts_with(root) {
        return None;
    }
    Some(SourceCorrelation {
        match_kind: "file_line".into(),
        path,
        line: line as u64,
        state_generation: snapshot.generation,
        analysis: snapshot.identity(),
    })
}
macro_rules! statistics_rows {
    ($type:ty,$fields:ident,$($field:ident),+) => {
        impl NativeStatistics for $type {
            type Fields=$fields;
            fn into_fields(self)->Self::Fields {$fields {$($field:self.$field,)+}}
            fn bound(&mut self, budget: &mut crate::outputs::budget::Budget) {
                let mut retained = 0;
                let mut omitted = crate::outputs::Omissions::default();
                for row in &mut self.items {
                    if retained > 0 && budget.bytes < 512 {
                        break;
                    }
                    row.name = budget.text(&row.name, 4096, "items.name", &mut omitted);
                    row.file = budget.text(&row.file, 4096, "items.file", &mut omitted);
                    budget.bytes = budget.bytes.saturating_sub(
                        crate::result::encoded_bytes(row, usize::MAX).unwrap_or(usize::MAX),
                    );
                    retained += 1;
                }
                if retained < self.items.len() || !omitted.is_empty() {
                    self.truncated = true;
                }
                self.items.truncate(retained);
            }
            fn truncated(&self) -> bool {
                self.truncated
            }
            fn correlate(&mut self, snapshot: Option<&crate::analysis_snapshot::AnalysisSnapshot>) {
                for row in &mut self.items {
                    row.source_correlation = correlation(snapshot, &row.file, row.line);
                }
            }
        }
    };
}
statistics_rows!(
    TracyHotspotsStatistics,
    TracyHotspotsFields,
    items,
    limit,
    span_ns,
    counts
);
statistics_rows!(TracyZoneStatisticsResult, TracyZoneFields, items, counts);
statistics_rows!(
    TracyComparisonStatistics,
    TracyComparisonFields,
    items,
    limit
);
impl NativeStatistics for TracyFrameStatistics {
    type Fields = Self;
    fn into_fields(self) -> Self::Fields {
        self
    }
    fn bound(&mut self, _: &mut crate::outputs::budget::Budget) {}
    fn correlate(&mut self, _: Option<&crate::analysis_snapshot::AnalysisSnapshot>) {}
    fn truncated(&self) -> bool {
        false
    }
}
