use crate::{CapabilityMode, RiftBuildAccess, ToolProfile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolEffects {
    pub reads_files: bool,
    pub writes_files: bool,
    pub spawns_process: bool,
    pub network_loopback: bool,
    pub network_external: bool,
    pub destructive: bool,
    /// Advisory only: project code may perform effects beyond the adapter.
    pub project_behavior: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportLevel {
    Verified,
    Provisional,
    Experimental,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupGate {
    Base,
    Rift,
    Docs,
    Debugger,
    Tracy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileMembership {
    Shared,
    Code,
    Assets,
    Runtime,
    CodeAndRuntime,
}

impl ProfileMembership {
    pub fn includes(self, profile: ToolProfile) -> bool {
        match profile {
            ToolProfile::All => true,
            ToolProfile::Code => matches!(self, Self::Shared | Self::Code | Self::CodeAndRuntime),
            ToolProfile::Assets => matches!(self, Self::Shared | Self::Assets),
            ToolProfile::Runtime => {
                matches!(self, Self::Shared | Self::Runtime | Self::CodeAndRuntime)
            }
        }
    }

    fn names(self) -> &'static str {
        match self {
            Self::Shared => "all, code, assets, runtime",
            Self::Code => "all, code",
            Self::Assets => "all, assets",
            Self::Runtime => "all, runtime",
            Self::CodeAndRuntime => "all, code, runtime",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ToolContract {
    pub name: &'static str,
    pub summary: &'static str,
    pub mode: CapabilityMode,
    pub profiles: ProfileMembership,
    pub effects: ToolEffects,
    pub support: SupportLevel,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: usize,
    pub(crate) gate: StartupGate,
    pub(crate) schema: fn() -> serde_json::Value,
    pub(crate) output_schema: fn() -> serde_json::Value,
    pub(crate) decode: fn(
        serde_json::Value,
    )
        -> Result<crate::parameters::ToolRequest, crate::parameters::InputError>,
}

const READ: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: false,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const MEMORY: ToolEffects = ToolEffects {
    reads_files: false,
    writes_files: false,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const COMPILE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: true,
    network_loopback: false,
    network_external: false,
    destructive: true,
    project_behavior: false,
};
const RIFT_COMPILE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: true,
    network_loopback: false,
    network_external: true,
    destructive: true,
    project_behavior: false,
};
const RENDER: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: true,
    project_behavior: false,
};
const RUNTIME: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: true,
    network_loopback: true,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const RUNTIME_STATE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const RUNTIME_STOP: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: true,
    project_behavior: false,
};
const TOPIC: ToolEffects = ToolEffects {
    reads_files: false,
    writes_files: false,
    spawns_process: false,
    network_loopback: true,
    network_external: false,
    destructive: true,
    project_behavior: true,
};
const DEBUG: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: false,
    spawns_process: true,
    network_loopback: true,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const DEBUG_EXPRESSION: ToolEffects = ToolEffects {
    destructive: true,
    project_behavior: true,
    ..DEBUG
};
const TRACY_PREPARE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: true,
    project_behavior: false,
};
const TRACY_PROCESS: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: false,
    spawns_process: true,
    network_loopback: false,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const TRACY_STATUS: ToolEffects = ToolEffects {
    reads_files: false,
    writes_files: false,
    spawns_process: false,
    network_loopback: true,
    network_external: false,
    destructive: false,
    project_behavior: false,
};
const TRACY_STOP: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: true,
    network_external: false,
    destructive: true,
    project_behavior: false,
};

macro_rules! contract {
    ($name:literal, $request:ty, $variant:ident, $output:ty, $gate:ident, $profiles:ident, $summary:literal, $mode:ident, $effects:ident, $support:ident, $timeout:expr, $max:expr) => {
        ToolContract {
            name: $name,
            gate: StartupGate::$gate,
            summary: $summary,
            mode: CapabilityMode::$mode,
            profiles: ProfileMembership::$profiles,
            effects: $effects,
            support: SupportLevel::$support,
            timeout_ms: $timeout,
            max_output_bytes: $max,
            schema: crate::parameters::schema::<$request>,
            output_schema: crate::result::output_schema::<$output>,
            decode: |value| {
                crate::parameters::decode::<$request>(value)
                    .map(crate::parameters::ToolRequest::$variant)
            },
        }
    };
}

static CONTRACTS: &[ToolContract] = &[
	contract!(
		"dm_server_status", crate::parameters::ServerStatusParams, ServerStatus, crate::outputs::ServerStatusOutput, Base, Shared,
		"Report immutable startup policy, build identity, analysis generation, and owned runtime summary.",
		Analysis,
		MEMORY,
		Provisional,
		None,
		262_144
	),
	contract!(
        "dm_parse_environment", crate::parameters::ParseEnvironmentParams, ParseEnvironment, crate::outputs::ParseEnvironmentOutput, Base, Shared,
        "Parse and atomically install DreamMaker analysis and lexical indexes.",
        Analysis,
        READ,
        Provisional,
        Some(1_800_000),
        1_048_576
    ),
    contract!(
        "dm_check_fixture_sync", crate::parameters::CheckFixtureSyncParams, CheckFixtureSync, crate::outputs::FixtureSyncOutput, Base, Code,
        "Validate declared fixture source contracts and build provenance.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!("dm_memory_summary", crate::memory_evidence::MemoryRequest, MemorySummary, crate::memory_evidence::MemorySummary, Base, Runtime, "Summarize sampled process memory over a selected time window.", Analysis, READ, Experimental, Some(30_000), 1_048_576),
    contract!("dm_memory_compare", crate::memory_evidence::MemoryCompareRequest, MemoryCompare, crate::memory_evidence::MemoryComparison, Base, Runtime, "Compare process memory for matching recorded builds and workloads.", Analysis, READ, Experimental, Some(30_000), 1_048_576),
    contract!("dm_native_evidence_summary", crate::native_evidence::model::NativeEvidenceRequest, NativeEvidenceSummary, crate::native_evidence::model::NormalizedRun, Base, Runtime, "Summarize bounded redacted native runtime evidence.", Analysis, READ, Experimental, Some(120_000), 1_048_576),
    contract!("dm_native_evidence_compare", crate::parameters::NativeEvidenceCompareParams, NativeEvidenceCompare, crate::native_evidence::ComparisonResult, Base, Runtime, "Compare identity-compatible native evidence runs.", Analysis, READ, Experimental, Some(600_000), 1_048_576),
    contract!(
        "dm_get_type", crate::parameters::GetTypeParams, GetType, crate::outputs::GetTypeOutput, Base, Code,
        "Inspect an exact DreamMaker type.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_get_proc", crate::parameters::GetProcParams, GetProc, crate::outputs::GetProcOutput, Base, Code,
        "Inspect exact proc implementations and source excerpts.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_get_var", crate::parameters::GetVarParams, GetVar, crate::outputs::GetVarOutput, Base, Code,
        "Inspect a DreamMaker variable through semantic inheritance, with value and declaration ownership.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_list_types", crate::parameters::ListTypesParams, ListTypes, crate::outputs::ListTypesOutput, Base, Code,
        "List parsed types under an optional prefix.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_search_symbols", crate::parameters::SearchSymbolsParams, SearchSymbols, crate::outputs::SearchSymbolsOutput, Base, Code,
        "Search parsed symbol names.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_search_context", crate::parameters::SearchContextParams, SearchContext, crate::outputs::SearchContextOutput, Base, Code,
        "Rank lexical candidates from parsed symbols and source context.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_check_errors", crate::parameters::CheckErrorsParams, CheckErrors, crate::outputs::CheckErrorsOutput, Base, Code,
        "Read bounded cached parser and DreamChecker diagnostics.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_get_definition", crate::parameters::GetDefinitionParams, GetDefinition, crate::outputs::GetDefinitionOutput, Base, Code,
        "Locate an exact parsed definition.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_document_symbols", crate::parameters::DocumentSymbolsParams, DocumentSymbols, crate::outputs::DocumentSymbolsOutput, Base, Code,
        "List declarations in one parsed source file.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_references", crate::parameters::FindReferencesParams, FindReferences, crate::outputs::FindReferencesOutput, Base, Code,
        "Find bounded exact member references.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_implementations", crate::parameters::FindImplementationsParams, FindImplementations, crate::outputs::FindImplementationsOutput, Base, Code,
        "Find type or member implementations.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_dmi_info", crate::parameters::DmiInfoParams, DmiInfo, crate::outputs::DmiInfoOutput, Base, Assets,
        "Profile DMI metadata and frame pixels without altering art.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_compare_dmi_states", crate::parameters::CompareDmiStatesParams, CompareDmiStates, crate::outputs::CompareDmiStatesOutput, Base, Assets,
        "Compare complete DMI states including common lazy changes.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_dmi_duplicates", crate::parameters::FindDmiDuplicatesParams, FindDmiDuplicates, crate::outputs::FindDmiDuplicatesOutput, Base, Assets,
        "Find cross-file exact and lazy-change DMI duplicates.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_audit_icons", crate::parameters::AuditIconsParams, AuditIcons, crate::outputs::AuditIconsOutput, Base, Assets,
        "Audit parsed icon evidence and duplicate DMI states.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_extract_dmi", crate::parameters::ExtractDmiParams, ExtractDmi, crate::outputs::ExtractDmiOutput, Base, Assets,
        "Mechanically extract a selected DMI state without altering source art.",
        Development,
        RENDER,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_map_info", crate::parameters::MapInfoParams, MapInfo, crate::outputs::MapInfoOutput, Base, Assets,
        "Read DMM/TGM dimensions and atom statistics.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_diff_maps", crate::parameters::DiffMapsParams, DiffMaps, crate::outputs::DiffMapsOutput, Base, Assets,
        "Compare coordinate models across two DMM/TGM maps.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_list_render_passes", crate::parameters::ListRenderPassesParams, ListRenderPasses, crate::outputs::ListRenderPassesOutput, Base, Assets,
        "List pinned SpacemanDMM render-pass behavior.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_render_maps", crate::parameters::RenderMapsParams, RenderMaps, crate::outputs::RenderMapsOutput, Base, Assets,
        "Render a bounded typed batch of map chunks.",
        Development,
        RENDER,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_on_map", crate::parameters::FindOnMapParams, FindOnMap, crate::outputs::FindOnMapOutput, Base, Assets,
        "Find exact type instances in a DMM/TGM map.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_compile", crate::parameters::CompileParams, Compile, crate::outputs::CompileOutput, Base, CodeAndRuntime,
        "Run an allowlisted DreamMaker compiler gate.",
        Development,
        COMPILE,
        Provisional,
        Some(1_800_000),
        1_048_576
    ),
    contract!(
        "dm_generate_docs", crate::parameters::GenerateDocsParams, GenerateDocs, crate::outputs::GenerateDocsOutput, Docs, Code,
        "Generate contained HTML through the verified exact dmdoc helper.",
        Development,
        COMPILE,
        Experimental,
        Some(600_000),
        262_144
    ),
    contract!(
        "dm_debug_launch", crate::parameters::DebugLaunchParams, DebugLaunch, crate::outputs::DebugLaunchOutput, Debugger, Runtime,
        "Launch one owned interactive or headless auxtools session.",
        Development,
        DEBUG,
        Experimental,
        Some(60_000),
        262_144
    ),
    contract!(
        "dm_debug_stop", crate::parameters::DebugStopParams, DebugStop, crate::outputs::DebugStopOutput, Debugger, Runtime,
        "Disconnect and terminate the owned debugger session.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_debug_set_breakpoints", crate::parameters::DebugSetBreakpointsParams, DebugSetBreakpoints, crate::outputs::DebugSetBreakpointsOutput, Debugger, Runtime,
        "Replace source-oriented auxtools breakpoints.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_set_function_breakpoints", crate::parameters::DebugSetFunctionBreakpointsParams, DebugSetFunctionBreakpoints, crate::outputs::DebugSetFunctionBreakpointsOutput, Debugger, Runtime,
        "Set canonical proc breakpoints.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_set_exception_breakpoints", crate::parameters::DebugSetExceptionBreakpointsParams, DebugSetExceptionBreakpoints, crate::outputs::DebugSetExceptionBreakpointsOutput, Debugger, Runtime,
        "Toggle breaks on DreamMaker runtimes.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_debug_control", crate::parameters::DebugControlParams, DebugControl, crate::outputs::DebugControlOutput, Debugger, Runtime,
        "Pause, continue, or step the active debuggee.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_debug_threads", crate::parameters::DebugThreadsParams, DebugThreads, crate::outputs::DebugThreadsOutput, Debugger, Runtime,
        "List auxtools debuggee stacks.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_stack_trace", crate::parameters::DebugStackTraceParams, DebugStackTrace, crate::outputs::DebugStackTraceOutput, Debugger, Runtime,
        "Read bounded debuggee stack frames.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_scopes", crate::parameters::DebugScopesParams, DebugScopes, crate::outputs::DebugScopesOutput, Debugger, Runtime,
        "Read variable scopes for a debug frame.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_variables", crate::parameters::DebugVariablesParams, DebugVariables, crate::outputs::DebugVariablesOutput, Debugger, Runtime,
        "Read a bounded variable-reference page.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_memory", crate::parameters::DebugMemoryParams, DebugMemory, crate::outputs::DebugMemoryOutput, Debugger, Runtime,
        "Control bounded, opt-in native allocation attribution.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_evaluate", crate::parameters::DebugEvaluateParams, DebugEvaluate, crate::outputs::DebugEvaluateOutput, Debugger, Runtime,
        "Evaluate an expression in the active debuggee.",
        Development,
        DEBUG_EXPRESSION,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_exception_info", crate::parameters::DebugExceptionInfoParams, DebugExceptionInfo, crate::outputs::DebugExceptionInfoOutput, Debugger, Runtime,
        "Read the last retained runtime exception.",
        Development,
        DEBUG,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_debug_source", crate::parameters::DebugSourceParams, DebugSource, crate::outputs::DebugSourceOutput, Debugger, Runtime,
        "Read the retained auxtools standard-definition source.",
        Development,
        DEBUG,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_debug_wait_for_event", crate::parameters::DebugWaitForEventParams, DebugWaitForEvent, crate::outputs::DebugWaitForEventOutput, Debugger, Runtime,
        "Wait for a bounded debugger event.",
        Development,
        DEBUG,
        Experimental,
        Some(300_000),
        1_048_576
    ),
    contract!(
        "rift_compile", crate::parameters::RiftCompileParams, RiftCompile, crate::outputs::RiftCompileOutput, Rift, CodeAndRuntime,
        "Run Meridian-Rift's contained RIFT_BUILD.cmd full-build gate; reserve outer cleanup time and validate its versioned artifact result.",
        Development,
        RIFT_COMPILE,
        Provisional,
        Some(1_800_000),
        1_048_576
    ),
    contract!(
        "dm_render_map", crate::parameters::RenderMapParams, RenderMap, crate::outputs::RenderMapOutput, Base, Assets,
        "Render a contained DMM/TGM map output.",
        Development,
        RENDER,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_run", crate::parameters::RunParams, Run, crate::outputs::RunOutput, Base, Runtime,
        "Start a contained DreamDaemon program on loopback.",
        Development,
        RUNTIME,
        Provisional,
        Some(300_000),
        1_048_576
    ),
    contract!(
        "dm_wait_for_output", crate::parameters::WaitForOutputParams, WaitForOutput, crate::outputs::WaitForOutputOutput, Base, Runtime,
        "Wait for bounded server-owned DreamDaemon output.",
        Development,
        RUNTIME_STATE,
        Provisional,
        Some(300_000),
        1_048_576
    ),
    contract!(
        "dm_stop", crate::parameters::StopParams, Stop, crate::outputs::StopOutput, Base, Runtime,
        "Stop the server-owned DreamDaemon process.",
        Development,
        RUNTIME_STOP,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_status", crate::parameters::StatusParams, Status, crate::outputs::StatusOutput, Base, Runtime,
        "Inspect server-owned DreamDaemon state.",
        Development,
        RUNTIME_STATE,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_topic", crate::parameters::TopicParams, Topic, crate::outputs::TopicOutput, Base, Runtime,
        "Call world.Topic on the loopback game server.",
        Development,
        TOPIC,
        Provisional,
        Some(60_000),
        262_144
    ),
    contract!(
        "dm_tracy_prepare", crate::parameters::TracyPrepareParams, TracyPrepare, crate::outputs::TracyPrepareOutput, Tracy, Runtime,
        "Install the verified byond-tracy hook beside a contained DMB.",
        Development,
        TRACY_PREPARE,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_tracy_launch", crate::parameters::TracyLaunchParams, TracyLaunch, crate::outputs::TracyLaunchOutput, Tracy, Runtime,
        "Launch an MCP-owned profiled DreamDaemon on loopback.",
        Development,
        RUNTIME,
        Experimental,
        Some(600_000),
        262_144
    ),
    contract!(
        "dm_tracy_capture", crate::parameters::TracyCaptureParams, TracyCapture, crate::outputs::TracyCaptureOutput, Tracy, Runtime,
        "Rotate the persistent collector for one validated window and publish an atomic `.tracy` plus schema-2 sidecar pair.",
        Development,
        RUNTIME,
        Experimental,
        Some(330_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_status", crate::parameters::TracyStatusParams, TracyStatus, crate::outputs::TracyStatusOutput, Tracy, Runtime,
        "Inspect profiled runtime and capture state.",
        Development,
        TRACY_STATUS,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_tracy_stop", crate::parameters::TracyStopParams, TracyStop, crate::outputs::TracyStopOutput, Tracy, Runtime,
        "Stop capture and the profiled DreamDaemon.",
        Development,
        TRACY_STOP,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_tracy_hotspots", crate::parameters::TracyHotspotsParams, TracyHotspots, crate::outputs::TracyHotspotsOutput, Tracy, Runtime,
        "Return bounded deterministic trace hotspots.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(120_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_zone", crate::parameters::TracyZoneParams, TracyZone, crate::outputs::TracyZoneOutput, Tracy, Runtime,
        "Inspect one profiled proc across source locations.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(120_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_frame_stats", crate::parameters::TracyFrameStatsParams, TracyFrameStats, crate::outputs::TracyFrameStatsOutput, Tracy, Runtime,
        "Summarize ServerTick frame durations.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(120_000),
        262_144
    ),
    contract!(
        "dm_tracy_compare", crate::parameters::TracyCompareParams, TracyCompare, crate::outputs::TracyCompareOutput, Tracy, Runtime,
        "Compare two traces by proc source identity.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(180_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_control_stats", crate::parameters::TracyControlStatsParams, TracyControlStats, crate::outputs::TracyControlStatsOutput, Tracy, Runtime,
        "Validate 3-20 repeated Tracy controls and calculate fixed noise statistics.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(2_400_000),
        1_048_576
    ),
];

pub fn all_contracts() -> &'static [ToolContract] {
    CONTRACTS
}

pub fn contracts_for(mode: CapabilityMode) -> Vec<&'static ToolContract> {
    contracts_for_configuration(mode, RiftBuildAccess::Disabled)
}

pub fn contracts_for_configuration(
    mode: CapabilityMode,
    rift_build: RiftBuildAccess,
) -> Vec<&'static ToolContract> {
    CONTRACTS
        .iter()
        .filter(|contract| {
            contract.support != SupportLevel::Unsupported
                && (contract.mode == CapabilityMode::Analysis
                    || mode == CapabilityMode::Development)
                && (contract.gate != StartupGate::Rift
                    || (cfg!(windows)
                        && mode == CapabilityMode::Development
                        && rift_build != RiftBuildAccess::Disabled))
        })
        .collect()
}

pub fn render_tool_reference(contracts: &[ToolContract]) -> String {
    let mut contracts = contracts.to_vec();
    contracts.sort_by_key(|contract| contract.name);
    let mut output = String::from("# Tool contracts\n\nGenerated from `src/contracts.rs`; do not edit by hand.\n\nStartup `MERIDIAN_MCP_TOOL_PROFILE` selects `all` (default), `code`, `assets` or `runtime`. Profile membership only narrows the existing mode, helper and authorization gates; it never enables unavailable capabilities. Status and parsing are shared by every profile.\n\nMax output bytes bounds the serialized SDK `CallToolResult` body, including text/structured duplication, JSON escaping, metadata and `resultType`. The client-owned JSON-RPC request ID is outside this cap. Output schemas and native structured content are advertised for MCP 2025-06-18 and newer; all versions retain compatibility text. Mutation outcomes survive optional-detail reduction.\n\n| Tool | Profiles | Mode | Support | Effects | Timeout ms | Max output bytes | Summary |\n| --- | --- | --- | --- | --- | ---: | ---: | --- |\n");
    for contract in contracts {
        let mut effects = Vec::new();
        if contract.effects.reads_files {
            effects.push("read");
        }
        if contract.effects.writes_files {
            effects.push("write");
        }
        if contract.effects.spawns_process {
            effects.push("process");
        }
        if contract.effects.network_loopback {
            effects.push("loopback");
        }
        if contract.effects.network_external {
            effects.push("network");
        }
        if contract.effects.project_behavior {
            effects.push("project behavior");
        }
        if contract.effects.destructive {
            effects.push("destructive");
        }
        output.push_str(&format!(
            "| `{}` | {} | {:?} | {:?} | {} | {} | {} | {} |\n",
            contract.name,
            contract.profiles.names(),
            contract.mode,
            contract.support,
            if effects.is_empty() {
                "memory".to_owned()
            } else {
                effects.join(", ")
            },
            contract
                .timeout_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            contract.max_output_bytes,
            contract.summary
        ));
    }
    output
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_family_requests_reject_nested_types_bounds_and_names() {
        for (name, value, field) in [
            (
                "dm_compile",
                json!({"dme_path":"fixture.dme","timeout_ms":u64::MAX}),
                "timeout_ms",
            ),
            (
                "dm_tracy_launch",
                json!({"dmb_path":"fixture.dmb","experiment_directory":"evidence","experiment_name":"%PRIVATE%"}),
                "experiment_name",
            ),
            (
                "dm_native_evidence_summary",
                json!({"artifacts":[{"kind":{"performance_csv":null},"path":"fixture.csv"}]}),
                "artifacts[0].kind",
            ),
            (
                "dm_memory_compare",
                json!({"baseline":["evidence.json",null,null,0],"current":{"evidence_path":"evidence.json"}}),
                "baseline",
            ),
            (
                "dm_native_evidence_summary",
                json!({"artifacts":[["performance_csv","fixture.csv"]]}),
                "artifacts[0]",
            ),
            (
                "dm_native_evidence_summary",
                json!({"artifacts":[{"kind":"performance_csv","path":"fixture.csv","options":[]}]}),
                "artifacts[0].options",
            ),
            ("dm_server_status", json!([]), "."),
            (
                "dm_render_map",
                json!({"dmm_path":"fixture.dmm","min":[2,1,1],"max":[1,1,1]}),
                "max",
            ),
            (
                "dm_render_maps",
                json!({"files":[{"dmm_path":"fixture.dmm","chunks":[{"output_path":"out.png","min":[1,1,1],"max":[1,1,2]}]}]}),
                "files[0].chunks[0].max",
            ),
            (
                "dm_native_evidence_summary",
                json!({"artifacts":[{"kind":"performance_csv","path":"fixture.csv"},{"kind":"performance_csv","path":"fixture.csv"}]}),
                "artifacts[1].path",
            ),
            (
                "dm_render_maps",
                json!({"files":[{"dmm_path":"fixture.dmm","chunks":[{"output_path":"out.png","min":[1,"2",1]}]}]}),
                "files[0].chunks[0].min[1]",
            ),
            (
                "dm_debug_set_breakpoints",
                json!({"source_path":"fixture.dm","breakpoints":[{"line":1,"condition":false}]}),
                "breakpoints[0].condition",
            ),
            (
                "dm_native_evidence_summary",
                json!({"artifacts":[{"kind":"performance_csv","path":"fixture.csv","options":{"group_fields":[7]}}]}),
                "artifacts[0].options.group_fields[0]",
            ),
            (
                "dm_tracy_launch",
                json!({"dmb_path":"fixture.dmb","experiment_directory":"evidence","annotations":{"note":3}}),
                "annotations.note",
            ),
            (
                "dm_debug_evaluate",
                json!({"expression":"#hidden_control"}),
                "expression",
            ),
            (
                "dm_run",
                json!({"dmb_path":"fixture.dmb","require_verified_provenance":"false"}),
                "require_verified_provenance",
            ),
            (
                "dm_extract_dmi",
                json!({"dmi_path":"fixture.dmi","state":"a","output_path":"out.png","overwrite":null}),
                "overwrite",
            ),
            (
                "dm_debug_variables",
                json!({"variables_reference":2147483648_i64}),
                "variables_reference",
            ),
        ] {
            let descriptor = all_contracts()
                .iter()
                .find(|tool| tool.name == name)
                .unwrap();
            let error = match (descriptor.decode)(value) {
                Err(error) => error,
                Ok(_) => panic!("accepted malformed {name}"),
            };
            assert_eq!(error.field, field, "{name}: {error}");
        }
    }

    #[test]
    fn intentional_nullable_windows_and_zero_summary_or_wait_values_remain_valid() {
        for (name, value) in [
            (
                "dm_memory_summary",
                json!({"evidence_path":"fixture.json","begin_ms":null,"end_ms":null,"sample_limit":0}),
            ),
            (
                "dm_check_fixture_sync",
                json!({"fixture_manifest_path":"fixture.json","issue_limit":0}),
            ),
            (
                "dm_wait_for_output",
                json!({"pattern":"ready","timeout_ms":0}),
            ),
            (
                "dm_debug_wait_for_event",
                json!({"timeout_ms":0,"after_sequence":0}),
            ),
            (
                "dm_debug_control",
                json!({"action":"step_in","thread_id":0}),
            ),
            (
                "dm_diff_maps",
                json!({"left_dmm_path":"a.dmm","right_dmm_path":"b.dmm","limit":0}),
            ),
        ] {
            let descriptor = all_contracts()
                .iter()
                .find(|tool| tool.name == name)
                .unwrap();
            assert!(
                (descriptor.decode)(value).is_ok(),
                "rejected valid compatibility case {name}"
            );
        }
    }

    #[test]
    fn error_paths_and_messages_do_not_echo_unbounded_unknown_keys_or_enum_values() {
        let descriptor = all_contracts()
            .iter()
            .find(|tool| tool.name == "dm_tracy_hotspots")
            .unwrap();
        let marker = "private_argument_".repeat(4096);
        for value in [
            json!({"trace_path":"fixture.tracy","sort":marker}),
            json!({"trace_path":"fixture.tracy",marker.clone():true}),
        ] {
            let error = match (descriptor.decode)(value) {
                Err(error) => error,
                Ok(_) => panic!("accepted malformed request"),
            };
            assert!(error.field.len() <= 256);
            assert!(!error.to_string().contains(&marker));
        }
    }
}
