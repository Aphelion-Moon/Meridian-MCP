use crate::{CapabilityMode, RiftBuildAccess};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolEffects {
    pub reads_files: bool,
    pub writes_files: bool,
    pub spawns_process: bool,
    pub network_loopback: bool,
    pub network_external: bool,
    pub destructive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportLevel {
    Verified,
    Provisional,
    Experimental,
    Unsupported,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolContract {
    pub name: &'static str,
    pub summary: &'static str,
    pub mode: CapabilityMode,
    pub effects: ToolEffects,
    pub support: SupportLevel,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: usize,
}

const READ: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: false,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: false,
};
const MEMORY: ToolEffects = ToolEffects {
    reads_files: false,
    writes_files: false,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: false,
};
const COMPILE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: true,
    network_loopback: false,
    network_external: false,
    destructive: true,
};
const RIFT_COMPILE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: true,
    network_loopback: false,
    network_external: true,
    destructive: true,
};
const RENDER: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: true,
};
const RUNTIME: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: true,
    network_loopback: true,
    network_external: false,
    destructive: false,
};
const RUNTIME_STATE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: false,
};
const RUNTIME_STOP: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: true,
};
const TOPIC: ToolEffects = ToolEffects {
    reads_files: false,
    writes_files: false,
    spawns_process: false,
    network_loopback: true,
    network_external: false,
    destructive: false,
};
const DEBUG: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: false,
    spawns_process: true,
    network_loopback: true,
    network_external: false,
    destructive: false,
};
const TRACY_PREPARE: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: false,
    network_external: false,
    destructive: true,
};
const TRACY_PROCESS: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: false,
    spawns_process: true,
    network_loopback: false,
    network_external: false,
    destructive: false,
};
const TRACY_STATUS: ToolEffects = ToolEffects {
    reads_files: false,
    writes_files: false,
    spawns_process: false,
    network_loopback: true,
    network_external: false,
    destructive: false,
};
const TRACY_STOP: ToolEffects = ToolEffects {
    reads_files: true,
    writes_files: true,
    spawns_process: false,
    network_loopback: true,
    network_external: false,
    destructive: true,
};

macro_rules! contract {
    ($name:literal, $summary:literal, $mode:ident, $effects:ident, $support:ident, $timeout:expr, $max:expr) => {
        ToolContract {
            name: $name,
            summary: $summary,
            mode: CapabilityMode::$mode,
            effects: $effects,
            support: SupportLevel::$support,
            timeout_ms: $timeout,
            max_output_bytes: $max,
        }
    };
}

static CONTRACTS: &[ToolContract] = &[
	contract!(
		"dm_server_status",
		"Report immutable startup policy, build identity, analysis generation, and owned runtime summary.",
		Analysis,
		MEMORY,
		Provisional,
		None,
		262_144
	),
	contract!(
        "dm_parse_environment",
        "Parse and atomically install DreamMaker analysis and lexical indexes.",
        Analysis,
        READ,
        Provisional,
        Some(1_800_000),
        1_048_576
    ),
    contract!(
        "dm_check_fixture_sync",
        "Validate declared fixture source contracts and build provenance.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!("dm_memory_summary", "Summarize sampled process memory over a selected time window.", Analysis, READ, Experimental, Some(30_000), 1_048_576),
    contract!("dm_memory_compare", "Compare process memory for matching recorded builds and workloads.", Analysis, READ, Experimental, Some(30_000), 1_048_576),
    contract!("dm_native_evidence_summary", "Summarize bounded redacted native runtime evidence.", Analysis, READ, Experimental, Some(120_000), 1_048_576),
    contract!("dm_native_evidence_compare", "Compare identity-compatible native evidence runs.", Analysis, READ, Experimental, Some(600_000), 1_048_576),
    contract!(
        "dm_get_type",
        "Inspect an exact DreamMaker type.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_get_proc",
        "Inspect exact proc implementations and source excerpts.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_get_var",
        "Inspect a DreamMaker variable through semantic inheritance, with value and declaration ownership.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_list_types",
        "List parsed types under an optional prefix.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_search_symbols",
        "Search parsed symbol names.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_search_context",
        "Rank lexical candidates from parsed symbols and source context.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_check_errors",
        "Read bounded cached parser and DreamChecker diagnostics.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_get_definition",
        "Locate an exact parsed definition.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_document_symbols",
        "List declarations in one parsed source file.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_references",
        "Find bounded exact member references.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_implementations",
        "Find type or member implementations.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_dmi_info",
        "Profile DMI metadata and frame pixels without altering art.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_compare_dmi_states",
        "Compare complete DMI states including common lazy changes.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_dmi_duplicates",
        "Find cross-file exact and lazy-change DMI duplicates.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_audit_icons",
        "Audit parsed icon evidence and duplicate DMI states.",
        Analysis,
        READ,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_extract_dmi",
        "Mechanically extract a selected DMI state without altering source art.",
        Development,
        RENDER,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_map_info",
        "Read DMM/TGM dimensions and atom statistics.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_diff_maps",
        "Compare coordinate models across two DMM/TGM maps.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_list_render_passes",
        "List pinned SpacemanDMM render-pass behavior.",
        Analysis,
        MEMORY,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_render_maps",
        "Render a bounded typed batch of map chunks.",
        Development,
        RENDER,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_find_on_map",
        "Find exact type instances in a DMM/TGM map.",
        Analysis,
        READ,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_compile",
        "Run an allowlisted DreamMaker compiler gate.",
        Development,
        COMPILE,
        Provisional,
        Some(1_800_000),
        1_048_576
    ),
    contract!(
        "dm_generate_docs",
        "Generate contained HTML through the verified exact dmdoc helper.",
        Development,
        COMPILE,
        Experimental,
        Some(300_000),
        262_144
    ),
    contract!(
        "dm_debug_launch",
        "Launch one owned interactive or headless auxtools session.",
        Development,
        DEBUG,
        Experimental,
        Some(60_000),
        262_144
    ),
    contract!(
        "dm_debug_stop",
        "Disconnect and terminate the owned debugger session.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_debug_set_breakpoints",
        "Replace source-oriented auxtools breakpoints.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_set_function_breakpoints",
        "Set canonical proc breakpoints.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_set_exception_breakpoints",
        "Toggle breaks on DreamMaker runtimes.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_debug_control",
        "Pause, continue, or step the active debuggee.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_debug_threads",
        "List auxtools debuggee stacks.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_stack_trace",
        "Read bounded debuggee stack frames.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_scopes",
        "Read variable scopes for a debug frame.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_variables",
        "Read a bounded variable-reference page.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_memory",
        "Control bounded, opt-in native allocation attribution.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_evaluate",
        "Evaluate an expression in the active debuggee.",
        Development,
        DEBUG,
        Experimental,
        Some(30_000),
        1_048_576
    ),
    contract!(
        "dm_debug_exception_info",
        "Read the last retained runtime exception.",
        Development,
        DEBUG,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_debug_source",
        "Read the retained auxtools standard-definition source.",
        Development,
        DEBUG,
        Experimental,
        None,
        1_048_576
    ),
    contract!(
        "dm_debug_wait_for_event",
        "Wait for a bounded debugger event.",
        Development,
        DEBUG,
        Experimental,
        Some(300_000),
        1_048_576
    ),
    contract!(
        "rift_compile",
        "Run Meridian-Rift's contained RIFT_BUILD.cmd full-build gate; reserve outer cleanup time and validate its versioned artifact result.",
        Development,
        RIFT_COMPILE,
        Provisional,
        Some(1_800_000),
        1_048_576
    ),
    contract!(
        "dm_render_map",
        "Render a contained DMM/TGM map output.",
        Development,
        RENDER,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_run",
        "Start a contained DreamDaemon program on loopback.",
        Development,
        RUNTIME,
        Provisional,
        Some(300_000),
        1_048_576
    ),
    contract!(
        "dm_wait_for_output",
        "Wait for bounded server-owned DreamDaemon output.",
        Development,
        RUNTIME_STATE,
        Provisional,
        Some(300_000),
        1_048_576
    ),
    contract!(
        "dm_stop",
        "Stop the server-owned DreamDaemon process.",
        Development,
        RUNTIME_STOP,
        Provisional,
        None,
        262_144
    ),
    contract!(
        "dm_status",
        "Inspect server-owned DreamDaemon state.",
        Development,
        RUNTIME_STATE,
        Provisional,
        None,
        1_048_576
    ),
    contract!(
        "dm_topic",
        "Call world.Topic on the loopback game server.",
        Development,
        TOPIC,
        Provisional,
        Some(60_000),
        262_144
    ),
    contract!(
        "dm_tracy_prepare",
        "Install the verified byond-tracy hook beside a contained DMB.",
        Development,
        TRACY_PREPARE,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_tracy_launch",
        "Launch an MCP-owned profiled DreamDaemon on loopback.",
        Development,
        RUNTIME,
        Experimental,
        Some(600_000),
        262_144
    ),
    contract!(
        "dm_tracy_capture",
        "Rotate the persistent collector for one validated window and publish an atomic `.tracy` plus schema-2 sidecar pair.",
        Development,
        RUNTIME,
        Experimental,
        Some(330_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_status",
        "Inspect profiled runtime and capture state.",
        Development,
        TRACY_STATUS,
        Experimental,
        None,
        262_144
    ),
    contract!(
        "dm_tracy_stop",
        "Stop capture and the profiled DreamDaemon.",
        Development,
        TRACY_STOP,
        Experimental,
        Some(30_000),
        262_144
    ),
    contract!(
        "dm_tracy_hotspots",
        "Return bounded deterministic trace hotspots.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(120_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_zone",
        "Inspect one profiled proc across source locations.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(120_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_frame_stats",
        "Summarize ServerTick frame durations.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(120_000),
        262_144
    ),
    contract!(
        "dm_tracy_compare",
        "Compare two traces by proc source identity.",
        Development,
        TRACY_PROCESS,
        Experimental,
        Some(180_000),
        1_048_576
    ),
    contract!(
        "dm_tracy_control_stats",
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
                && (contract.name != "rift_compile"
                    || (cfg!(windows)
                        && mode == CapabilityMode::Development
                        && rift_build != RiftBuildAccess::Disabled))
        })
        .collect()
}

pub fn render_tool_reference(contracts: &[ToolContract]) -> String {
    let mut contracts = contracts.to_vec();
    contracts.sort_by_key(|contract| contract.name);
    let mut output = String::from("# Tool contracts\n\nGenerated from `src/contracts.rs`; do not edit by hand.\n\n| Tool | Mode | Support | Effects | Timeout ms | Max output bytes | Summary |\n| --- | --- | --- | --- | ---: | ---: | --- |\n");
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
        if contract.effects.destructive {
            effects.push("destructive");
        }
        output.push_str(&format!(
            "| `{}` | {:?} | {:?} | {} | {} | {} | {} |\n",
            contract.name,
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
