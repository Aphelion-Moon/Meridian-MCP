use crate::spaceman::debugger::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Serialize, JsonSchema)]
pub struct DebugLaunchData {
    pub lifecycle: &'static str,
    pub host_mode: String,
    pub port: u16,
    pub dmb_path: std::path::PathBuf,
    pub dll_sha256: String,
    pub memory_profile: bool,
    pub launch_provenance: crate::LaunchProvenance,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugStopData {
    pub lifecycle: &'static str,
    pub launch_provenance: crate::LaunchProvenance,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugThreadsData {
    pub threads: Vec<Stack>,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugStackTraceData {
    pub frames: Vec<StackFrame>,
    pub total_count: u32,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugScopesData {
    pub arguments: Option<VariablesRef>,
    pub locals: Option<VariablesRef>,
    pub globals: Option<VariablesRef>,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugVariablesData {
    pub variables: Vec<Variable>,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugEvaluateData {
    pub result: EvalResponse,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugExceptionBreakpointsData {
    pub break_on_runtimes: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugControlData {
    pub action: String,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugBreakpoint {
    pub instruction: InstructionRef,
    pub verified: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugBreakpointsData {
    pub breakpoints: Vec<DebugBreakpoint>,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugExceptionInfoData {
    pub message: Option<String>,
    pub sequence: u64,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugSourceData {
    pub source_reference: u32,
    pub name: &'static str,
    pub content: String,
    pub source_origin: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugEventData {
    pub event: Option<DebuggerEventRecord>,
    pub timed_out: bool,
    pub dropped_events: u64,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NativeMemoryEnvelope {
    pub protocol_version: u32,
    pub allocator: String,
    pub scope: String,
    pub excludes: Vec<String>,
    pub evidence: NativeMemoryEvidence,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NativeMemoryEvidence {
    pub ok: bool,
    pub result: NativeMemoryResult,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum NativeMemoryResult {
    Stop(NativeMemoryStop),
    Start(NativeMemoryStart),
    Status(NativeMemoryStatus),
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NativeMemoryStatus {
    pub available: bool,
    pub pending_capture: bool,
    pub recording: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NativeMemoryStart {
    pub recording: bool,
    pub duration_ms: u64,
    pub max_records: usize,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NativeMemoryStop {
    pub recording: bool,
    pub stop_reason: String,
    pub elapsed_ms: u64,
    pub capacity_exceeded: bool,
    pub total_procedures: usize,
    pub rows_truncated: bool,
    pub attributed_allocation_calls: u64,
    pub unattributed_allocation_calls: u64,
    pub outstanding_requested_bytes: u64,
    pub peak_outstanding_requested_bytes: u64,
    pub procedures: Vec<NativeMemoryProcedure>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NativeMemoryProcedure {
    pub proc_id: u32,
    pub proc_path: Option<String>,
    pub proc_path_truncated: bool,
    pub outstanding_requested_bytes: u64,
    pub allocation_count: u64,
}
#[derive(Serialize, JsonSchema)]
pub struct DebugMemoryData {
    pub native_memory: NativeMemoryEnvelope,
    pub helper_sha256: String,
    pub helper_source_revision: &'static str,
    pub launch_provenance: crate::LaunchProvenance,
    pub warning: &'static str,
}

pub type DebugLaunchOutput = crate::result::Success<DebugLaunchData>;
pub type DebugStopOutput = crate::result::Success<DebugStopData>;
pub type DebugSetBreakpointsOutput = crate::result::Success<DebugBreakpointsData>;
pub type DebugSetFunctionBreakpointsOutput = crate::result::Success<DebugBreakpointsData>;
pub type DebugSetExceptionBreakpointsOutput = crate::result::Success<DebugExceptionBreakpointsData>;
pub type DebugControlOutput = crate::result::Success<DebugControlData>;
pub type DebugThreadsOutput = crate::result::Success<DebugThreadsData>;
pub type DebugStackTraceOutput = crate::result::Success<DebugStackTraceData>;
pub type DebugScopesOutput = crate::result::Success<DebugScopesData>;
pub type DebugVariablesOutput = crate::result::Success<DebugVariablesData>;
pub type DebugMemoryOutput = crate::result::Success<DebugMemoryData>;
pub type DebugEvaluateOutput = crate::result::Success<DebugEvaluateData>;
pub type DebugExceptionInfoOutput = crate::result::Success<DebugExceptionInfoData>;
pub type DebugSourceOutput = crate::result::Success<DebugSourceData>;
pub type DebugWaitForEventOutput = crate::result::Success<DebugEventData>;
