use schemars::JsonSchema;
use serde::Serialize;
#[derive(Serialize, JsonSchema)]
pub struct WaitObservation {
    pub matched: bool,
    pub pattern: String,
    pub regex: bool,
    pub timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_exited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit_code: Option<Option<i32>>,
    pub recent_output: Vec<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct RunData {
    pub runtime_id: String,
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    pub success: bool,
    pub pid: Option<u32>,
    pub port: u16,
    pub dmb_path: String,
    pub working_directory: String,
    pub runtime_kind: &'static str,
    pub profiler_port: Option<u16>,
    pub launch_provenance: crate::LaunchProvenance,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness: Option<WaitObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_stopped: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<Option<crate::runtime_integrity::RuntimeIntegritySummary>>,
}
#[derive(Serialize, JsonSchema)]
pub struct RuntimeFailureData {
    pub message: &'static str,
    pub last_exit_code: Option<i32>,
    pub recent_output: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<Option<crate::runtime_integrity::RuntimeIntegritySummary>>,
}
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub enum RunOutput {
    Run(Box<RunData>),
    Failed(RuntimeFailureData),
}
#[derive(Serialize, JsonSchema)]
pub struct WaitData {
    #[serde(flatten)]
    pub observation: WaitObservation,
    pub runtime_id: Option<String>,
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_replaced: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<Option<crate::runtime_integrity::RuntimeIntegritySummary>>,
    pub launch_provenance: Option<crate::LaunchProvenance>,
    pub recent_output_entries: Vec<crate::state::RuntimeOutputEntry>,
}
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub enum WaitForOutputOutput {
    Wait(Box<WaitData>),
    Failed(RuntimeFailureData),
}
#[derive(Serialize, JsonSchema)]
pub struct StopData {
    pub success: bool,
    pub process_stopped: bool,
    pub message: &'static str,
    pub runtime_id: Option<String>,
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    pub launch_provenance: Option<crate::LaunchProvenance>,
    pub integrity: Option<crate::runtime_integrity::RuntimeIntegritySummary>,
    pub warnings: Vec<crate::runtime_integrity::RuntimeIntegrityEvent>,
}
#[derive(Serialize, JsonSchema)]
pub struct StatusData {
    pub running: bool,
    pub runtime_kind: Option<crate::state::RuntimeKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit_code: Option<Option<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiler_port: Option<Option<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<Option<Option<u32>>>,
    pub recent_output: Vec<String>,
    pub recent_output_entries: Vec<crate::state::RuntimeOutputEntry>,
    pub runtime_id: Option<String>,
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    pub launch_provenance: Option<crate::LaunchProvenance>,
    pub integrity: Option<crate::runtime_integrity::RuntimeIntegritySummary>,
}
#[derive(Serialize, JsonSchema)]
pub struct TopicData {
    pub success: bool,
    pub runtime_id: Option<String>,
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    pub response: String,
}
pub type StopOutput = StopData;
pub type StatusOutput = StatusData;
pub type TopicOutput = TopicData;
pub(crate) fn recent(rows: Vec<String>) -> Vec<String> {
    let mut budget = crate::outputs::budget::Budget {
        bytes: 24 * 1024,
        source_lines: 0,
    };
    let mut omissions = crate::outputs::Omissions::default();
    rows.iter()
        .take(50)
        .map(|text| budget.text(text, 8192, "recent_output", &mut omissions))
        .collect()
}
pub(crate) fn entries(
    mut rows: Vec<crate::state::RuntimeOutputEntry>,
) -> Vec<crate::state::RuntimeOutputEntry> {
    let mut budget = crate::outputs::budget::Budget {
        bytes: 24 * 1024,
        source_lines: 0,
    };
    let mut omissions = crate::outputs::Omissions::default();
    for row in &mut rows {
        row.text = budget.text(&row.text, 8192, "recent_output_entries", &mut omissions);
    }
    rows
}
