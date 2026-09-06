mod analysis;
mod compile;
mod debugger;
mod diagnostics;
mod dmi;
mod docs;
mod fixture;
mod language;
mod map;
mod memory;
mod native_evidence;
mod parse;
pub mod rift;
pub(crate) mod runtime;
mod search;
mod server_status;
mod tracy;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::mcp::{ToolDefinition, ToolResult};
use crate::result::{structured_error, ToolErrorCode};
use crate::spaceman::debugger::DebuggerInstallation;
use crate::state::ServerState;
use crate::tracy::TracyInstallation;
use crate::{contracts_for_configuration, CapabilityMode, PathPolicy, RiftBuildAccess};

pub(crate) fn bounded_u64(
    args: &Value,
    name: &str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64> {
    let value = match args.get(name) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow!("{name} must be an integer"))?,
        None => default,
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(anyhow!("{name} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

#[derive(Clone)]
pub struct ToolExecutionContext {
    mode: CapabilityMode,
    policy: PathPolicy,
    rift_build: RiftBuildAccess,
    dmdoc_helper: Option<std::path::PathBuf>,
    debugger: Option<DebuggerInstallation>,
    tracy: Option<TracyInstallation>,
    private_state: Option<std::sync::Arc<crate::PrivateStateStore>>,
    build_provenance: Option<std::sync::Arc<crate::BuildProvenanceStore>>,
    integrity_recovery: std::sync::Arc<[crate::runtime_integrity::RuntimeIntegritySummary]>,
}

impl ToolExecutionContext {
    pub fn new(mode: CapabilityMode, policy: PathPolicy) -> Self {
        Self::with_rift_build(mode, policy, RiftBuildAccess::Disabled)
    }

    pub fn with_rift_build(
        mode: CapabilityMode,
        policy: PathPolicy,
        rift_build: RiftBuildAccess,
    ) -> Self {
        Self {
            mode,
            policy,
            rift_build,
            dmdoc_helper: None,
            debugger: None,
            tracy: None,
            private_state: None,
            build_provenance: None,
            integrity_recovery: std::sync::Arc::from([]),
        }
    }

    pub fn with_features(
        mode: CapabilityMode,
        policy: PathPolicy,
        rift_build: RiftBuildAccess,
        dmdoc_helper: Option<std::path::PathBuf>,
        debugger: Option<DebuggerInstallation>,
        tracy: Option<TracyInstallation>,
    ) -> Self {
        Self::with_features_and_state(
            mode,
            policy,
            rift_build,
            dmdoc_helper,
            debugger,
            tracy,
            None,
        )
    }

    pub fn with_features_and_state(
        mode: CapabilityMode,
        policy: PathPolicy,
        rift_build: RiftBuildAccess,
        dmdoc_helper: Option<std::path::PathBuf>,
        debugger: Option<DebuggerInstallation>,
        tracy: Option<TracyInstallation>,
        private_state: Option<std::sync::Arc<crate::PrivateStateStore>>,
    ) -> Self {
        let build_provenance = private_state.as_ref().map(|state| {
            std::sync::Arc::new(crate::BuildProvenanceStore::new(
                std::sync::Arc::clone(state),
                policy.clone(),
            ))
        });
        let integrity_recovery = private_state
            .as_ref()
            .and_then(|state| {
                crate::runtime_integrity::recover_unfinished(state, policy.effective_roots()).ok()
            })
            .unwrap_or_default();
        Self {
            mode,
            policy,
            rift_build,
            dmdoc_helper,
            debugger,
            tracy,
            private_state,
            build_provenance,
            integrity_recovery: std::sync::Arc::from(integrity_recovery),
        }
    }

    pub fn mode(&self) -> CapabilityMode {
        self.mode
    }

    pub fn rift_build_access(&self) -> RiftBuildAccess {
        self.rift_build
    }

    pub(crate) fn policy(&self) -> &PathPolicy {
        &self.policy
    }
    pub(crate) fn dmdoc_helper(&self) -> Option<&std::path::Path> {
        self.dmdoc_helper.as_deref()
    }
    pub(crate) fn debugger(&self) -> Option<&DebuggerInstallation> {
        self.debugger.as_ref()
    }
    pub(crate) fn tracy(&self) -> Option<&TracyInstallation> {
        self.tracy.as_ref()
    }
    pub(crate) fn private_state(&self) -> Option<&crate::PrivateStateStore> {
        self.private_state.as_deref()
    }
    pub(crate) fn private_state_arc(&self) -> Option<std::sync::Arc<crate::PrivateStateStore>> {
        self.private_state.as_ref().map(std::sync::Arc::clone)
    }
    pub(crate) fn build_provenance(&self) -> Option<&crate::BuildProvenanceStore> {
        self.build_provenance.as_deref()
    }
    pub(crate) fn build_provenance_arc(
        &self,
    ) -> Option<std::sync::Arc<crate::BuildProvenanceStore>> {
        self.build_provenance.as_ref().map(std::sync::Arc::clone)
    }
    pub(crate) fn integrity_recovery(
        &self,
    ) -> &[crate::runtime_integrity::RuntimeIntegritySummary] {
        &self.integrity_recovery
    }
}

/// Get all available tool definitions
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    let mut tools = Vec::new();

    // Parsing tools
    tools.push(ToolDefinition {
		name: "dm_server_status".to_string(),
		description: "Report immutable startup policy, build identity, analysis generation, and owned runtime summary.".to_string(),
		input_schema: json!({
			"type": "object",
			"properties": {},
			"additionalProperties": false
		}),
	});

    tools.push(ToolDefinition {
		name: "dm_parse_environment".to_string(),
        description: "Parse a DreamMaker environment and atomically install its analysis and lexical BM25 indexes. Reuse compares metadata for every parser-registered input; results report stage timings and explicit dense-not-configured readiness. Call this before other analysis tools.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dme_path": {
                    "type": "string",
                    "description": "Path to the .dme environment file"
                },
                "force": {
                    "type": "boolean",
                    "description": "Reparse even when the active snapshot already matches this environment on disk (default false)."
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1800000,
                    "description": "Total request budget including admission, blocking scheduling, reuse validation, parsing, and installation (default 600000). Unfinished workers retain exclusive admission after timeout or cancellation."
                }
            },
            "required": ["dme_path"],
            "additionalProperties": false
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_check_fixture_sync".to_string(),
        description: "Validate a contained fixture manifest against parsed DreamMaker proc contracts, required text tokens, and managed build provenance.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "fixture_manifest_path": {"type": "string"}
            },
            "required": ["fixture_manifest_path"],
            "additionalProperties": false
        }),
    });
    let evidence_request_schema = json!({
        "type":"object",
        "properties":{
            "artifacts":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"object","properties":{"kind":{"type":"string","enum":["byond_proc_profile_json","byond_sendmaps_json","performance_csv","runtime_jsonl","event_jsonl"]},"path":{"type":"string"},"options":{"type":"object"}},"required":["kind","path"],"additionalProperties":false}},
            "dmb_path":{"type":"string"},
            "workload":{"type":"object"},
            "phases":{"type":"array","maxItems":64,"items":{"type":"object"}}
        },
        "required":["artifacts"],
        "additionalProperties":false
    });
    tools.push(ToolDefinition { name: "dm_memory_summary".into(), description: "Read schema-2 Tracy memory evidence and summarize a half-open millisecond window. Reports separate process metrics, sampled peaks, net growth and gaps; does not identify allocations or prove leaks. No parse or native helper required.".into(), input_schema: serde_json::to_value(schemars::schema_for!(crate::memory_evidence::MemoryRequest)).unwrap() });
    tools.push(ToolDefinition { name: "dm_memory_compare".into(), description: "Compare two memory evidence windows with matching recorded build/workload identities, OS, roles and metrics. Reports descriptive deltas, not verified leak or performance conclusions. No parse or native helper required.".into(), input_schema: serde_json::to_value(schemars::schema_for!(crate::memory_evidence::MemoryCompareRequest)).unwrap() });
    tools.push(ToolDefinition { name:"dm_native_evidence_summary".into(), description:"Validate, redact, phase-align, and summarize bounded BYOND and application-native evidence without modifying raw artifacts.".into(), input_schema:evidence_request_schema.clone() });
    tools.push(ToolDefinition { name:"dm_native_evidence_compare".into(), description:"Recompute and identity-check 2-20 complete native-evidence runs before calculating deterministic metric deltas and distributions.".into(), input_schema:json!({"type":"object","properties":{"runs":{"type":"array","minItems":2,"maxItems":20,"items":evidence_request_schema}},"required":["runs"],"additionalProperties":false}) });

    tools.push(ToolDefinition {
        name: "dm_get_type".to_string(),
        description: "Get information about a type including its variables, procs, parent type, and children.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "type_path": {
                    "type": "string",
                    "description": "The type path (e.g., '/obj/item', '/mob/living')"
                }
            },
            "required": ["type_path"]
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_get_proc".to_string(),
        description: "Inspect a procedure's parameters, ownership, documentation and bounded snapshot source. Set include_source=false for metadata only; excerpts report their physical boundaries and truncation.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "type_path": {
                    "type": "string",
                    "description": "The type path containing the proc"
                },
                "proc_name": {
                    "type": "string",
                    "description": "Name of the procedure"
                },
                "include_source": {
                    "type": "boolean",
                    "default": true
                },
                "max_source_lines": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": crate::source::MAX_SOURCE_LINES,
                    "default": crate::source::DEFAULT_PROC_SOURCE_LINES
                }
            },
            "required": ["type_path", "proc_name"]
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_get_var".to_string(),
        description: "Inspect a variable through semantic inheritance, including its declared type, initial value, documentation, and separate value/declaration owners.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "type_path": {
                    "type": "string",
                    "description": "The type path containing the variable"
                },
                "var_name": {
                    "type": "string",
                    "description": "Name of the variable"
                }
            },
            "required": ["type_path", "var_name"]
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_list_types".to_string(),
        description: "List all types in the object tree, optionally filtered by a path prefix."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "prefix": {
                    "type": "string",
                    "description": "Optional path prefix to filter types (e.g., '/obj')"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum depth to traverse (default: unlimited)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 500,
                    "default": 100,
                    "description": "Maximum types returned in one page"
                },
                "cursor": {
                    "type": "string",
                    "pattern": "^[0-9]+$",
                    "description": "Opaque next_cursor from a previous response"
                }
            }
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_search_symbols".to_string(),
        description: "Search for types, procs, variables, or macros by name pattern.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (supports partial matches)"
                },
                "kind": {
                    "type": "string",
                    "enum": ["type", "proc", "var", "macro", "all"],
                    "description": "Kind of symbol to search for (default: all)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "default": 50,
                    "description": "Maximum number of results (default: 50)"
                }
            },
            "required": ["query"]
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_search_context".to_string(),
        description: "Search parsed DreamMaker types, procs, variables, documentation, and source using deterministic candidate-bounded lexical BM25 retrieval. Reports candidate and scoring counts; call dm_parse_environment first and verify results with exact inspection tools.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language behavior, exact symbol, or identifier terms to find"
                },
                "kind": {
                    "type": "string",
                    "enum": ["all", "type", "proc", "var"],
                    "description": "Optional symbol kind filter (default: all)"
                },
                "type_prefix": {
                    "type": "string",
                    "description": "Optional canonical type-path prefix, such as /turf/open"
                },
                "file_filter": {
                    "type": "string",
                    "description": "Optional case-insensitive source-path substring"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Maximum ranked results (default: 10)"
                },
                "include_source": {
                    "type": "boolean",
                    "description": "Include physical snapshot excerpts with boundaries and truncation (default: true); false returns metadata only"
                },
                "max_source_lines": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Maximum source lines per result, up to the 200-line snapshot budget (default: 40)"
                }
            },
            "required": ["query"]
        }),
    });

    // Analysis tools
    tools.push(ToolDefinition {
        name: "dm_check_errors".to_string(),
        description: "Read a bounded page of parser and DreamChecker diagnostics cached by the latest dm_parse_environment call. Returns a summary across all matching diagnostics; it does not rerun analysis or replace DreamMaker compilation."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Only include diagnostics whose normalized source path contains this value. Both slash styles are accepted."
                },
                "severity": {
                    "type": "string",
                    "enum": ["error", "warning", "info", "hint"],
                    "description": "Only include diagnostics with this severity."
                },
                "component": {
                    "type": "string",
                    "enum": ["parser", "dreamchecker"],
                    "description": "Only include diagnostics emitted by this SpacemanDMM component."
                },
                "rule": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Only include diagnostics with this exact rule identifier."
                },
                "configured": {
                    "type": "boolean",
                    "description": "Only include diagnostics whose rule is or is not explicitly configured in SpacemanDMM.toml."
                },
                "cursor": {
                    "type": "string",
                    "pattern": "^[0-9]+$",
                    "description": "Opaque continuation cursor returned by a previous call using the same filters and analysis generation."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 50,
                    "description": "Maximum diagnostics returned in this page. Summary counts cover every matching diagnostic."
                }
            }
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_get_definition".to_string(),
        description: "Get the source location where a symbol is defined.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "type_path": {
                    "type": "string",
                    "description": "The type path"
                },
                "member_name": {
                    "type": "string",
                    "description": "Optional: name of var or proc to find definition of"
                }
            },
            "required": ["type_path"]
        }),
    });

    tools.push(ToolDefinition {
		name: "dm_document_symbols".to_string(),
		description: "Page through parsed declarations and variable assignments in one contained source file. In compact detail, overlay each row on shared to reconstruct full fields.".to_string(),
		input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20000,"default":100},"cursor":{"type":"string","description":"Opaque next_cursor from the same query and parsed snapshot."},"detail":{"type":"string","enum":["full","compact"],"default":"full"}},"required":["file_path"]}),
	});
    tools.push(ToolDefinition {
		name: "dm_find_references".to_string(),
		description: "Page through resolved references to a type or exact member. Dynamic skips are environment-wide, not a per-symbol completeness count. In compact detail, overlay each row on shared.".to_string(),
		input_schema: json!({"type":"object","properties":{"type_path":{"type":"string"},"member_name":{"type":"string","minLength":1},"kind":{"type":"string","enum":["call","read","write","type_path","macro_expansion","declaration"]},"include_declaration":{"type":"boolean","default":false},"limit":{"type":"integer","minimum":1,"maximum":10000,"default":100},"cursor":{"type":"string","description":"Opaque next_cursor from the same query and parsed snapshot."},"detail":{"type":"string","enum":["full","compact"],"default":"full"}},"required":["type_path"]}),
	});
    tools.push(ToolDefinition {
		name: "dm_find_implementations".to_string(),
		description: "Page through semantic type descendants, proc implementations or variable assignments, including explicit parent_type. In compact detail, overlay each row on shared.".to_string(),
		input_schema: json!({"type":"object","properties":{"type_path":{"type":"string"},"member_name":{"type":"string","minLength":1},"limit":{"type":"integer","minimum":1,"maximum":10000,"default":100},"cursor":{"type":"string","description":"Opaque next_cursor from the same query and parsed snapshot."},"detail":{"type":"string","enum":["full","compact"],"default":"full"}},"required":["type_path"]}),
	});

    // Compile tool
    tools.push(ToolDefinition { name:"dm_generate_docs".to_string(), description:"Generate contained DreamMaker HTML documentation with the hash-verified exact-revision dmdoc helper.".to_string(), input_schema:json!({"type":"object","properties":{"output_directory":{"type":"string"},"overwrite":{"type":"boolean"}},"required":["output_directory"]}) });

    tools.push(ToolDefinition {
        name: "dm_compile".to_string(),
        description: "Compile the DreamMaker environment using the DM compiler. Returns compiler output and any errors.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dme_path": {
                    "type": "string",
                    "description": "Path to the .dme file to compile"
                },
                "compiler_path": {
                    "type": "string",
                    "description": "Optional path to the DreamMaker executable"
                },
                "working_directory": {
                    "type": "string",
                    "description": "Optional working directory for the compiler process"
                },
                "defines": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional preprocessor defines, with or without the -D prefix"
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Compiler timeout in milliseconds (default: 600000, capped at 1800000)"
                },
                "idle_timeout_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "description": "Fail if DreamMaker produces no output and consumes no CPU for this long (default: 45000, capped at 900000)"
                },
                "capture_network": {
                    "type": "boolean",
                    "description": "Request best-effort endpoint observation (default: false)"
                },
                "fixture_manifest_path": {
                    "type": "string",
                    "description": "Optional contained declarative fixture manifest"
                }
            },
            "required": ["dme_path"]
        }),
    });

    tools.push(ToolDefinition {
        name: "rift_compile".to_string(),
        description: "Run Meridian-Rift's contained RIFT_BUILD.cmd full-build gate.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "network_mode": {
                    "type": "string",
                    "enum": ["offline", "allow"],
                    "description": "Dependency network mode (default: offline)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "maximum": 1800000,
                    "description": "Wall timeout in milliseconds (default: 1800000)"
                },
                "idle_timeout_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "maximum": 900000,
                    "description": "No-output and no-CPU timeout in milliseconds (default: 120000)"
                },
                "capture_network": {
                    "type": "boolean",
                    "description": "Request best-effort endpoint observation (default: false)"
                },
                "force_rebuild": {
                    "type": "boolean",
                    "description": "Remove only canonical root build artifacts before building (default: false)"
                },
                "fixture_manifest_path": {
                    "type": "string",
                    "description": "Optional contained declarative fixture manifest"
                }
            }
        }),
    });

    // DMI analysis and mechanical extraction tools
    tools.push(ToolDefinition { name:"dm_dmi_info".to_string(), description:"Profile DMI metadata, frames, alpha bounds, pixel counts, and content hashes without changing the asset.".to_string(), input_schema:json!({"type":"object","properties":{"dmi_path":{"type":"string"}},"required":["dmi_path"]}) });
    tools.push(ToolDefinition { name:"dm_compare_dmi_states".to_string(), description:"Compare two complete DMI states for exact, mirrored, rotated, padded, palette, metadata-only, or near-copy changes.".to_string(), input_schema:json!({"type":"object","properties":{"left_dmi_path":{"type":"string"},"left_state":{"type":"string"},"left_duplicate_index":{"type":"integer","minimum":0},"right_dmi_path":{"type":"string"},"right_state":{"type":"string"},"right_duplicate_index":{"type":"integer","minimum":0},"minimum_similarity":{"type":"number","minimum":0.9,"maximum":1.0}},"required":["left_dmi_path","left_state","right_dmi_path","right_state"]}) });
    tools.push(ToolDefinition { name:"dm_find_dmi_duplicates".to_string(), description:"Scan a contained scope for cross-DMI duplicate states, including common mirrored, padded, palette-swapped, and near-copy changes.".to_string(), input_schema:json!({"type":"object","properties":{"scope_path":{"type":"string"},"include_glob":{"type":"string"},"minimum_similarity":{"type":"number","minimum":0.9,"maximum":1.0},"include_frame_matches":{"type":"boolean"},"max_matches":{"type":"integer","minimum":1}}}) });
    tools.push(ToolDefinition { name:"dm_audit_icons".to_string(), description:"Correlate parsed icon evidence with bounded DMI duplicate scanning; dynamic references make unused-state evidence best-effort.".to_string(), input_schema:json!({"type":"object","properties":{"scope_path":{"type":"string"},"include_glob":{"type":"string"},"minimum_similarity":{"type":"number","minimum":0.9,"maximum":1.0},"include_unused":{"type":"boolean"},"max_matches":{"type":"integer","minimum":1}}}) });
    tools.push(ToolDefinition { name:"dm_extract_dmi".to_string(), description:"Mechanically extract one user-selected DMI state, contact sheet, or exact frame without altering the source art.".to_string(), input_schema:json!({"type":"object","properties":{"dmi_path":{"type":"string"},"state":{"type":"string"},"duplicate_index":{"type":"integer","minimum":0},"kind":{"type":"string","enum":["auto","png","gif","contact_sheet","frame"]},"direction":{"type":"string","enum":["north","south","east","west","northeast","northwest","southeast","southwest"]},"frame":{"type":"integer","minimum":0},"output_path":{"type":"string"},"overwrite":{"type":"boolean"}},"required":["dmi_path","state","output_path"]}) });

    // Map tools
    tools.push(ToolDefinition {
        name: "dm_render_map".to_string(),
        description: "Render a map file (.dmm) to a PNG image.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dmm_path": {
                    "type": "string",
                    "description": "Path to the .dmm map file"
                },
                "z_level": {
                    "type": "integer",
                    "description": "Z-level to render (default: 1)"
                },
                "output_path": {
                    "type": "string",
                    "description": "Path to save the PNG (default: same as dmm with .png extension)"
                }
                ,
                "overwrite": {
                    "type": "boolean",
                    "description": "Replace an existing output only when explicitly true (default: false)"
                }
				,"min":{"type":"array","items":{"type":"integer","minimum":1},"minItems":3,"maxItems":3},
				"max":{"type":"array","items":{"type":"integer","minimum":1},"minItems":3,"maxItems":3},
				"enable_passes":{"type":"array","items":{"type":"string"}},
				"disable_passes":{"type":"array","items":{"type":"string"}}
            },
            "required": ["dmm_path"]
        }),
    });
    tools.push(ToolDefinition { name:"dm_diff_maps".to_string(), description:"Compare coordinate models between two contained DMM/TGM maps independent of dictionary keys.".to_string(), input_schema:json!({"type":"object","properties":{"left_dmm_path":{"type":"string"},"right_dmm_path":{"type":"string"},"limit":{"type":"integer","minimum":1}},"required":["left_dmm_path","right_dmm_path"]}) });
    tools.push(ToolDefinition {
        name: "dm_list_render_passes".to_string(),
        description:
            "List every pinned SpacemanDMM render pass with its description and default state."
                .to_string(),
        input_schema: json!({"type":"object","properties":{}}),
    });
    tools.push(ToolDefinition {
		name: "dm_render_maps".to_string(),
		description: "Render a validated, bounded batch of typed map chunks without exposing raw RenderMany commands.".to_string(),
		input_schema: json!({
			"type": "object",
			"properties": {
				"files": {"type":"array", "items": {
					"type":"object",
					"properties": {
						"dmm_path":{"type":"string"},
						"chunks":{"type":"array","items":{
							"type":"object",
							"properties":{
								"output_path":{"type":"string"},
								"z_level":{"type":"integer","minimum":1},
								"min":{"type":"array","items":{"type":"integer","minimum":1},"minItems":3,"maxItems":3},
								"max":{"type":"array","items":{"type":"integer","minimum":1},"minItems":3,"maxItems":3}
							},
							"required":["output_path"]
						}}
					},
					"required":["dmm_path","chunks"]
				}},
				"enable_passes": {"type":"array", "items":{"type":"string"}},
				"disable_passes": {"type":"array", "items":{"type":"string"}},
				"overwrite": {"type":"boolean"}
			},
			"required": ["files"]
		}),
	});

    tools.push(ToolDefinition {
        name: "dm_map_info".to_string(),
        description:
            "Get information about a map file including dimensions, z-levels, and area statistics."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dmm_path": {
                    "type": "string",
                    "description": "Path to the .dmm map file"
                }
            },
            "required": ["dmm_path"]
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_find_on_map".to_string(),
        description: "Find all instances of a type on a map.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dmm_path": {
                    "type": "string",
                    "description": "Path to the .dmm map file"
                },
                "type_path": {
                    "type": "string",
                    "description": "Type path to search for (e.g., '/obj/machinery/door')"
                }
            },
            "required": ["dmm_path", "type_path"]
        }),
    });

    tools.extend([
        ToolDefinition { name:"dm_debug_launch".into(), description:"Launch one MCP-owned interactive DreamSeeker or headless DreamDaemon session with a hash-verified auxtools debugger. Opt into the separately installed native memory helper with memory_profile.".into(), input_schema:json!({"type":"object","properties":{"dmb_path":{"type":"string"},"host_mode":{"type":"string","enum":["interactive","headless"],"default":"interactive"},"memory_profile":{"type":"boolean","default":false},"startup_timeout_ms":{"type":"integer","minimum":1,"maximum":60000},"require_verified_provenance":{"type":"boolean","default":false}},"required":["dmb_path"],"additionalProperties":false}) },
        ToolDefinition { name: "dm_debug_memory".into(), description: "Check, start or stop bounded native allocation attribution in a debugger launched with memory_profile: true. Windows BYOND 516.1687 only; excludes other threads and object retaining references.".into(), input_schema: serde_json::to_value(schemars::schema_for!(crate::native_memory::MemoryControl)).expect("native memory schema") },
        ToolDefinition { name:"dm_debug_stop".into(), description:"Disconnect and terminate the active MCP-owned debugger process tree.".into(), input_schema:json!({"type":"object","properties":{}}) },
        ToolDefinition { name:"dm_debug_set_breakpoints".into(), description:"Replace all source breakpoints for one contained parsed DreamMaker file.".into(), input_schema:debug_source_breakpoint_schema() },
        ToolDefinition { name:"dm_debug_set_function_breakpoints".into(), description:"Set a bounded complete list of canonical proc breakpoints.".into(), input_schema:debug_breakpoint_schema() },
        ToolDefinition { name:"dm_debug_set_exception_breakpoints".into(), description:"Enable or disable breaks on DreamMaker runtime exceptions.".into(), input_schema:json!({"type":"object","properties":{"break_on_runtimes":{"type":"boolean"}},"required":["break_on_runtimes"]}) },
        ToolDefinition { name:"dm_debug_control".into(), description:"Pause, continue, step into, step over, or step out of the active debuggee.".into(), input_schema:json!({"type":"object","properties":{"action":{"type":"string","enum":["pause","continue","step_in","step_over","step_out"]},"thread_id":{"type":"integer","minimum":0}},"required":["action"]}) },
        ToolDefinition { name:"dm_debug_threads".into(), description:"List the active debuggee's bounded thread/stack inventory.".into(), input_schema:json!({"type":"object","properties":{}}) },
        ToolDefinition { name:"dm_debug_stack_trace".into(), description:"Read a bounded page of stack frames for a debuggee thread.".into(), input_schema:json!({"type":"object","properties":{"thread_id":{"type":"integer","minimum":0},"start_frame":{"type":"integer","minimum":0},"count":{"type":"integer","minimum":1}},"required":["thread_id"]}) },
        ToolDefinition { name:"dm_debug_scopes".into(), description:"Read arguments, locals, and globals references for a stack frame.".into(), input_schema:json!({"type":"object","properties":{"frame_id":{"type":"integer","minimum":0}},"required":["frame_id"]}) },
        ToolDefinition { name:"dm_debug_variables".into(), description:"Read bounded variables for an auxtools variable reference.".into(), input_schema:json!({"type":"object","properties":{"variables_reference":{"type":"integer"}},"required":["variables_reference"]}) },
        ToolDefinition { name:"dm_debug_evaluate".into(), description:"Execute a bounded DreamMaker expression in the active debuggee.".into(), input_schema:json!({"type":"object","properties":{"expression":{"type":"string","maxLength":16384},"frame_id":{"type":"integer","minimum":0},"context":{"type":"string","enum":["watch","repl","hover"]}},"required":["expression"]}) },
        ToolDefinition { name:"dm_debug_exception_info".into(), description:"Read the most recent retained runtime exception from the active session.".into(), input_schema:json!({"type":"object","properties":{}}) },
        ToolDefinition { name:"dm_debug_source".into(), description:"Read the retained auxtools stddef source by issued source reference only.".into(), input_schema:json!({"type":"object","properties":{"source_reference":{"type":"integer","enum":[1]}},"required":["source_reference"]}) },
        ToolDefinition { name:"dm_debug_wait_for_event".into(), description:"Wait for the next bounded debugger event after an optional sequence.".into(), input_schema:json!({"type":"object","properties":{"kinds":{"type":"array","items":{"type":"string","enum":["breakpoint","step","pause","runtime","output","terminated"]}},"after_sequence":{"type":"integer","minimum":0},"timeout_ms":{"type":"integer","minimum":1,"maximum":300000}}}) },
    ]);

    // Runtime tools
    tools.push(ToolDefinition {
        name: "dm_run".to_string(),
        description:
            "Start DreamDaemon with a compiled .dmb file. Returns the process ID and port."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dmb_path": {
                    "type": "string",
                    "description": "Path to the compiled .dmb file"
                },
                "port": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 65535,
                    "description": "Port to run the server on (default: 1337)"
                },
                "working_directory": {
                    "type": "string",
                    "description": "Optional working directory used to resolve a relative DMB path and run DreamDaemon"
                },
                "daemon_args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional additional DreamDaemon arguments, appended after the standard trusted/logging arguments"
                },
                "wait_for": {
                    "type": "string",
                    "description": "Optional output marker to wait for before returning"
                },
                "wait_regex": {
                    "type": "boolean",
                    "description": "Interpret wait_for as a regular expression (default: false)"
                },
                "startup_timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 300000,
                    "description": "Maximum wait for wait_for in milliseconds (default: 30000)"
                },
                "require_verified_provenance": {
                    "type": "boolean",
                    "default": false,
                    "description": "Reject an unmanaged DMB unless it has a current Meridian-MCP build record"
                }
            },
            "required": ["dmb_path"],
            "additionalProperties": false
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_wait_for_output".to_string(),
        description:
            "Wait for a literal or regular-expression marker in captured DreamDaemon output."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Output text or regular expression to wait for"
                },
                "regex": {
                    "type": "boolean",
                    "description": "Interpret pattern as a regular expression (default: false)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Maximum wait in milliseconds (default: 30000, capped at 300000)"
                }
            },
            "required": ["pattern"]
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_stop".to_string(),
        description: "Stop the currently running DreamDaemon instance.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_status".to_string(),
        description: "Get the status of the running game (port, PID, whether it's running)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    });

    tools.push(ToolDefinition {
        name: "dm_topic".to_string(),
        description: "Send a Topic() call to the running game server. Use this to communicate with debug handlers in the game.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "description": "The topic string to send (e.g., '?debug_screenshot' or '?debug_click=10,20')"
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 60000,
                    "description": "Total request timeout in milliseconds (default: 5000)"
                }
            },
            "required": ["topic"]
        }),
    });

    tools.push(ToolDefinition { name: "dm_tracy_prepare".into(), description: "Place the verified pinned byond-tracy hook beside a contained DMB using atomic replacement rules.".into(), input_schema: json!({"type":"object","properties":{"dmb_path":{"type":"string"},"overwrite":{"type":"boolean","default":false}},"required":["dmb_path"]}) });
    let workload_properties = json!({
        "experiment_name":{"type":"string","maxLength":512},
        "map":{"type":"string","maxLength":512},
        "seed":{"type":"string","maxLength":512},
        "configuration_profile":{"type":"string","maxLength":512},
        "feature_set":{"type":"array","maxItems":64,"items":{"type":"string","maxLength":512}},
        "scenario":{"type":"string","maxLength":512},
        "external_run_id":{"type":"string","maxLength":512},
        "annotations":{"type":"object","maxProperties":32,"additionalProperties":{"type":"string","maxLength":512}}
    });
    let mut launch_properties = workload_properties.as_object().cloned().unwrap_or_default();
    launch_properties.extend(serde_json::Map::from_iter([
        ("dmb_path".into(), json!({"type":"string"})),
        (
            "game_port".into(),
            json!({"type":"integer","minimum":1,"maximum":65535}),
        ),
        (
            "startup_timeout_ms".into(),
            json!({"type":"integer","minimum":1000,"maximum":60000,"default":60000,"description":"Maximum time for both the Tracy client connection and producer-health readiness."}),
        ),
        (
            "experiment_directory".into(),
            json!({"type":"string","description":"Existing contained directory for immutable experiment manifests, recovery journals, and diagnostics."}),
        ),
        (
            "config_directory".into(),
            json!({"type":"string","description":"Optional contained profiling configuration directory. It must contain config.txt and explicitly enable RESUME_AFTER_INITIALIZATIONS in config.txt or dev_overrides.txt; its full contents are hash-bound into launch identity. The flag requires a post-initialization wake before it can take effect on a headless world."}),
        ),
        (
            "wake_sleeping_world".into(),
            json!({"type":"boolean","default":true,"description":"When config_directory is supplied, wait for Meridian-Rift initialization, try bounded loopback Topic wakes, and if necessary hold one fixed MCP-owned loopback DreamSeeker guest connection until stop. Set false for a deliberately sleeping control run."}),
        ),
        (
            "initialization_timeout_ms".into(),
            json!({"type":"integer","minimum":1,"maximum":300000,"default":180000,"description":"Maximum wait for the fixed Meridian-Rift initialization-complete marker before the post-initialization wake request."}),
        ),
        (
            "experiment_name".into(),
            json!({"type":"string","maxLength":512}),
        ),
        (
            "require_verified_provenance".into(),
            json!({"type":"boolean","default":false}),
        ),
    ]));
    tools.push(ToolDefinition { name: "dm_tracy_launch".into(), description: "Launch an MCP-owned DreamDaemon with fixed Tracy parameters, an optional hash-bound post-initialization wake configuration, an immutable executable/workload draft, and a private loopback profiler endpoint.".into(), input_schema: json!({"type":"object","properties":launch_properties,"required":["dmb_path","experiment_directory"]}) });
    let mut capture_properties = workload_properties.as_object().cloned().unwrap_or_default();
    capture_properties.remove("experiment_name");
    capture_properties.extend(serde_json::Map::from_iter([
        ("output_path".into(), json!({"type":"string"})),
        (
            "duration_ms".into(),
            json!({"type":"integer","minimum":1,"maximum":300000}),
        ),
        (
            "memory_limit_mb".into(),
            json!({"type":"integer","minimum":16,"maximum":4096}),
        ),
        (
            "overwrite".into(),
            json!({"type":"boolean","default":false}),
        ),
        (
            "capture_network".into(),
            json!({"type":"boolean","default":false}),
        ),
        (
            "phase".into(),
            json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[a-z0-9_-]+$"}),
        ),
        (
            "phase_iteration".into(),
            json!({"type":"integer","minimum":1,"maximum":4294967295_u64}),
        ),
        (
            "capture_annotations".into(),
            json!({"type":"object","maxProperties":32,"additionalProperties":{"type":"string","maxLength":512}}),
        ),
    ]));
    tools.push(ToolDefinition { name: "dm_tracy_capture".into(), description: "Lock or verify immutable workload identity, then capture one uniquely named phase iteration into an atomic trace/sidecar pair.".into(), input_schema: json!({"type":"object","properties":capture_properties,"required":["output_path","duration_ms","memory_limit_mb","phase","phase_iteration"]}) });
    tools.push(ToolDefinition { name: "dm_tracy_status".into(), description: "Report profiled DreamDaemon, capture, endpoint, hook, helper, protocol, and last-error state.".into(), input_schema: json!({"type":"object","properties":{}}) });
    tools.push(ToolDefinition {
        name: "dm_tracy_stop".into(),
        description:
            "Stop the active Tracy capture and then terminate the MCP-owned profiled DreamDaemon."
                .into(),
        input_schema: json!({"type":"object","properties":{}}),
    });
    tools.push(ToolDefinition { name: "dm_tracy_hotspots".into(), description: "Read a contained Tracy trace and return bounded deterministic DreamMaker hotspot statistics.".into(), input_schema: json!({"type":"object","properties":{"trace_path":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100},"sort":{"type":"string","enum":["inclusive","self","count","max"],"default":"inclusive"}},"required":["trace_path"]}) });
    tools.push(ToolDefinition { name: "dm_tracy_zone".into(), description: "Inspect bounded statistics for one DreamMaker proc name across source locations in a contained Tracy trace.".into(), input_schema: json!({"type":"object","properties":{"trace_path":{"type":"string"},"name":{"type":"string","maxLength":4096},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100}},"required":["trace_path","name"]}) });
    tools.push(ToolDefinition { name: "dm_tracy_frame_stats".into(), description: "Summarize ServerTick frame count, span, mean, extrema, and p50/p95/p99 from a contained Tracy trace.".into(), input_schema: json!({"type":"object","properties":{"trace_path":{"type":"string"}},"required":["trace_path"]}) });
    tools.push(ToolDefinition { name: "dm_tracy_compare".into(), description: "Verify trace identity compatibility, then compare two contained Tracy traces by proc, file, and line.".into(), input_schema: json!({"type":"object","properties":{"baseline_path":{"type":"string"},"current_path":{"type":"string"},"comparison_mode":{"type":"string","enum":["same_experiment_same_phase","cross_experiment"],"default":"same_experiment_same_phase"},"minimum_delta_ns":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100}},"required":["baseline_path","current_path"]}) });
    tools.push(ToolDefinition { name: "dm_tracy_control_stats".into(), description: "Validate 3-20 identity-compatible controls and calculate fixed frame and exact-zone noise statistics.".into(), input_schema: json!({"type":"object","properties":{"trace_paths":{"type":"array","minItems":3,"maxItems":20,"uniqueItems":true,"items":{"type":"string"}},"frame_percentile":{"type":"string","enum":["p50","p95","p99"],"default":"p95"},"zone_keys":{"type":"array","maxItems":32,"uniqueItems":true,"items":{"type":"string","maxLength":4096,"description":"file|line|name|inclusive_or_self|p50_p95_or_p99"}},"comparison_mode":{"type":"string","enum":["same_experiment_same_phase","cross_experiment"],"default":"same_experiment_same_phase"}},"required":["trace_paths"]}) });

    tools
}

fn debug_breakpoint_schema() -> Value {
    json!({"type":"object","properties":{"breakpoints":{"type":"array","maxItems":10000,"items":{"type":"object","properties":{"proc_path":{"type":"string"},"override_id":{"type":"integer","minimum":0},"offset":{"type":"integer","minimum":0},"condition":{"type":"string","maxLength":4096}},"required":["proc_path"]}}},"required":["breakpoints"]})
}

fn debug_source_breakpoint_schema() -> Value {
    json!({"type":"object","properties":{"source_path":{"type":"string"},"breakpoints":{"type":"array","maxItems":10000,"items":{"type":"object","properties":{"line":{"type":"integer","minimum":1},"condition":{"type":"string","maxLength":4096}},"required":["line"]}}},"required":["source_path","breakpoints"]})
}

pub fn get_tool_definitions_for(
    mode: CapabilityMode,
    rift_build: RiftBuildAccess,
) -> Vec<ToolDefinition> {
    let active: std::collections::HashSet<_> = contracts_for_configuration(mode, rift_build)
        .into_iter()
        .map(|contract| contract.name)
        .collect();
    get_tool_definitions()
        .into_iter()
        .filter(|definition| {
            definition.name != "dm_generate_docs" && active.contains(definition.name.as_str())
        })
        .collect()
}

pub fn get_tool_definitions_for_active(
    mode: CapabilityMode,
    rift_build: RiftBuildAccess,
    dmdoc: bool,
) -> Vec<ToolDefinition> {
    get_tool_definitions_for_runtime(mode, rift_build, dmdoc, false, false)
}

pub fn get_tool_definitions_for_runtime(
    mode: CapabilityMode,
    rift_build: RiftBuildAccess,
    dmdoc: bool,
    debugger: bool,
    tracy: bool,
) -> Vec<ToolDefinition> {
    let mut tools = get_tool_definitions_for(mode, rift_build);
    tools.retain(|tool| !tool.name.starts_with("dm_debug_"));
    tools.retain(|tool| !tool.name.starts_with("dm_tracy_"));
    if dmdoc && mode == CapabilityMode::Development {
        if let Some(tool) = get_tool_definitions()
            .into_iter()
            .find(|tool| tool.name == "dm_generate_docs")
        {
            tools.push(tool)
        }
    }
    if debugger && mode == CapabilityMode::Development {
        tools.extend(
            get_tool_definitions()
                .into_iter()
                .filter(|tool| tool.name.starts_with("dm_debug_")),
        );
    }
    if tracy && mode == CapabilityMode::Development {
        tools.extend(
            get_tool_definitions()
                .into_iter()
                .filter(|tool| tool.name.starts_with("dm_tracy_")),
        );
    }
    tools
}

/// Call a tool by name with the given arguments
pub async fn call_tool(
    context: &ToolExecutionContext,
    state: &ServerState,
    name: &str,
    mut args: Value,
) -> Result<ToolResult> {
    if name == "rift_compile" && !cfg!(windows) {
        return Ok(policy_error(
            "unsupported_platform",
            "rift_compile is supported only on Windows".to_string(),
            None,
            "Run rift_compile from an approved Windows Meridian-MCP installation.",
            json!({ "tool": name, "platform": std::env::consts::OS }),
        ));
    }
    if name == "dm_generate_docs" && context.dmdoc_helper.is_none() {
        return Ok(policy_error(
            "tool_not_available",
            "dm_generate_docs requires a verified startup helper".into(),
            None,
            "Set MERIDIAN_MCP_HELPER_MANIFEST to a valid exact-revision helper manifest.",
            json!({"tool":name}),
        ));
    }
    if name.starts_with("dm_debug_") && context.debugger.is_none() {
        return Ok(policy_error(
            "tool_not_available",
            "debugger tools require a verified auxtools startup configuration".into(),
            None,
            "Start Meridian-MCP in development mode with MERIDIAN_MCP_DEBUGGER=auxtools.",
            json!({"tool":name}),
        ));
    }
    if name.starts_with("dm_tracy_") && context.tracy.is_none() {
        return Ok(policy_error(
            "tool_not_available",
            "Tracy tools require a verified startup helper configuration".into(),
            None,
            "Start Meridian-MCP in development mode with MERIDIAN_MCP_TRACY=byond.",
            json!({"tool":name}),
        ));
    }
    if !contracts_for_configuration(context.mode, context.rift_build)
        .iter()
        .any(|contract| contract.name == name)
    {
        return Ok(policy_error(
            "tool_not_available",
            format!("{name} is not available in {:?} mode", context.mode),
            None,
            "Use a tool advertised by tools/list for the immutable startup configuration.",
            json!({
                "tool": name,
                "mode": match context.mode {
                    CapabilityMode::Analysis => "analysis",
                    CapabilityMode::Development => "development",
                },
            }),
        ));
    }
    if let Err(error) = contain_arguments(&context.policy, name, &mut args) {
        return Ok(policy_error(
            error.code(),
            error.to_string(),
            Some(error.path()),
            "Use a contained path and only startup-allowlisted executables.",
            json!({
                "path": error.path().display().to_string(),
                "policy_code": error.code(),
                "containment_mode": error.context().containment_mode,
                "policy_source": error.context().policy_source,
                "effective_roots": error.context().effective_roots,
            }),
        ));
    }
    match name {
        // Parsing tools
        "dm_server_status" => server_status::status(context, state).await,
        "dm_parse_environment" => {
            parse::parse_environment_with_policy(state, args, context.policy()).await
        }
        "dm_check_fixture_sync" => fixture::check_sync(context, state, args).await,
        "dm_memory_summary" => memory::run(context, args, false).await,
        "dm_memory_compare" => memory::run(context, args, true).await,
        "dm_native_evidence_summary" => native_evidence::summary(context, args).await,
        "dm_native_evidence_compare" => native_evidence::compare(context, args).await,
        "dm_get_type" => parse::get_type(state, args).await,
        "dm_get_proc" => parse::get_proc(state, args).await,
        "dm_get_var" => parse::get_var(state, args).await,
        "dm_list_types" => parse::list_types(state, args).await,
        "dm_search_symbols" => parse::search_symbols(state, args).await,
        "dm_search_context" => search::search_context(state, args).await,

        // Analysis tools
        "dm_check_errors" => diagnostics::check_errors(state, args).await,
        "dm_get_definition" => analysis::get_definition(state, args).await,
        "dm_generate_docs" => docs::generate(context, state, args).await,
        "dm_document_symbols" => language::document_symbols(state, args).await,
        "dm_find_references" => language::find_references(state, args).await,
        "dm_find_implementations" => language::find_implementations(state, args).await,
        "dm_dmi_info" => dmi::info(context, state, args).await,
        "dm_compare_dmi_states" => dmi::compare(context, state, args).await,
        "dm_find_dmi_duplicates" => dmi::find_duplicates(context, state, args).await,
        "dm_audit_icons" => dmi::audit_icons(context, state, args).await,
        "dm_extract_dmi" => dmi::extract(context, state, args).await,

        // Compile tool
        "dm_compile" => compile::compile(context, state, args).await,
        "rift_compile" => rift::compile(context, state, args).await,

        // Map tools
        "dm_render_map" => map::render_map(context, state, args).await,
        "dm_map_info" => map::map_info(args).await,
        "dm_find_on_map" => map::find_on_map(args).await,
        "dm_diff_maps" => map::diff_maps(args).await,
        "dm_list_render_passes" => map::list_render_passes().await,
        "dm_render_maps" => map::render_maps(context, state, args).await,

        "dm_debug_launch" => debugger::launch(context, state, args).await,
        "dm_debug_stop" => debugger::stop(state).await,
        "dm_debug_set_breakpoints" => debugger::set_breakpoints(state, args).await,
        "dm_debug_set_function_breakpoints" => {
            debugger::set_function_breakpoints(state, args).await
        }
        "dm_debug_set_exception_breakpoints" => {
            debugger::set_exception_breakpoints(state, args).await
        }
        "dm_debug_control" => debugger::control(state, args).await,
        "dm_debug_threads" => debugger::threads(state).await,
        "dm_debug_stack_trace" => debugger::stack_trace(state, args).await,
        "dm_debug_scopes" => debugger::scopes(state, args).await,
        "dm_debug_variables" => debugger::variables(state, args).await,
        "dm_debug_evaluate" => debugger::evaluate(state, args).await,
        "dm_debug_memory" => debugger::memory(state, args).await,
        "dm_debug_exception_info" => debugger::exception_info(state).await,
        "dm_debug_source" => debugger::source(state, args).await,
        "dm_debug_wait_for_event" => debugger::wait_for_event(state, args).await,

        // Runtime tools
        "dm_run" => runtime::run(context, state, args).await,
        "dm_wait_for_output" => runtime::wait_for_output(state, args).await,
        "dm_stop" => runtime::stop(state, args).await,
        "dm_status" => runtime::status(state, args).await,
        "dm_topic" => runtime::topic(state, args).await,
        "dm_tracy_prepare" => tracy::prepare(context, args).await,
        "dm_tracy_launch" => tracy::launch(context, state, args).await,
        "dm_tracy_capture" => tracy::capture(context, state, args).await,
        "dm_tracy_status" => tracy::status(state).await,
        "dm_tracy_stop" => tracy::stop(context, state).await,
        "dm_tracy_hotspots" => tracy::hotspots(context, state, args).await,
        "dm_tracy_zone" => tracy::zone(context, state, args).await,
        "dm_tracy_frame_stats" => tracy::frame_stats(context, state, args).await,
        "dm_tracy_compare" => tracy::compare(context, state, args).await,
        "dm_tracy_control_stats" => tracy::control_stats(context, state, args).await,
        _ => Err(anyhow!("Unknown tool: {name}")),
    }
}

fn contain_arguments(
    policy: &PathPolicy,
    name: &str,
    args: &mut Value,
) -> std::result::Result<(), crate::PolicyError> {
    match name {
        "dm_parse_environment" | "dm_compile" => {
            canonical_argument(policy, args, "dme_path", false)?
        }
        "dm_check_fixture_sync" => {
            canonical_argument(policy, args, "fixture_manifest_path", false)?
        }
        "dm_memory_summary" => canonical_argument(policy, args, "evidence_path", false)?,
        "dm_memory_compare" => {
            for name in ["baseline", "current"] {
                if let Some(request) = args.get_mut(name) {
                    canonical_argument(policy, request, "evidence_path", false)?;
                }
            }
        }
        "dm_native_evidence_summary" => contain_evidence_request(policy, args)?,
        "dm_native_evidence_compare" => {
            if let Some(runs) = args.get_mut("runs").and_then(Value::as_array_mut) {
                for run in runs {
                    contain_evidence_request(policy, run)?;
                }
            }
        }
        "dm_document_symbols" => canonical_argument(policy, args, "file_path", false)?,
        "dm_dmi_info" => canonical_argument(policy, args, "dmi_path", false)?,
        "dm_compare_dmi_states" => {
            canonical_argument(policy, args, "left_dmi_path", false)?;
            canonical_argument(policy, args, "right_dmi_path", false)?;
        }
        "dm_find_dmi_duplicates" | "dm_audit_icons" if args.get("scope_path").is_some() => {
            canonical_argument(policy, args, "scope_path", false)?
        }
        "dm_extract_dmi" => {
            canonical_argument(policy, args, "dmi_path", false)?;
        }
        "dm_generate_docs" => {}
        "dm_render_map" | "dm_map_info" | "dm_find_on_map" => {
            canonical_argument(policy, args, "dmm_path", false)?
        }
        "dm_diff_maps" => {
            canonical_argument(policy, args, "left_dmm_path", false)?;
            canonical_argument(policy, args, "right_dmm_path", false)?;
        }
        "dm_run" | "dm_tracy_prepare" | "dm_tracy_launch" => {
            canonical_argument(policy, args, "dmb_path", true)?
        }
        "dm_debug_launch" => canonical_argument(policy, args, "dmb_path", true)?,
        "dm_debug_set_breakpoints" => canonical_argument(policy, args, "source_path", false)?,
        "dm_tracy_hotspots" | "dm_tracy_zone" | "dm_tracy_frame_stats" => {
            canonical_argument(policy, args, "trace_path", false)?
        }
        "dm_tracy_compare" => {
            canonical_argument(policy, args, "baseline_path", false)?;
            canonical_argument(policy, args, "current_path", false)?;
        }
        "dm_tracy_capture" => {}
        _ => {}
    }
    if name == "dm_compile" {
        canonical_optional_argument(policy, args, "working_directory")?;
        canonical_optional_argument(policy, args, "fixture_manifest_path")?;
        if let Some(path) = args.get("compiler_path").and_then(Value::as_str) {
            let path = policy.executable(path)?;
            args["compiler_path"] = Value::String(path.display().to_string());
        }
    }
    if name == "rift_compile" {
        canonical_optional_argument(policy, args, "fixture_manifest_path")?;
    }
    if name == "dm_run" {
        canonical_optional_argument(policy, args, "working_directory")?;
    }
    if name == "dm_render_map" {
        let output = args
            .get("output_path")
            .and_then(Value::as_str)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(args["dmm_path"].as_str().unwrap()).with_extension("png")
            });
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let output = policy.output_path(output, overwrite)?;
        args["output_path"] = Value::String(output.display().to_string());
    }
    if name == "dm_extract_dmi" {
        if let Some(output) = args.get("output_path").and_then(Value::as_str) {
            let overwrite = args
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let output = policy.output_path(output, overwrite)?;
            args["output_path"] = Value::String(output.display().to_string());
        }
    }
    if name == "dm_generate_docs" {
        if let Some(output) = args.get("output_directory").and_then(Value::as_str) {
            let overwrite = args
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let output = policy.output_path(output, overwrite)?;
            args["output_directory"] = Value::String(output.display().to_string());
        }
    }
    Ok(())
}

fn contain_evidence_request(
    policy: &PathPolicy,
    request: &mut Value,
) -> std::result::Result<(), crate::PolicyError> {
    if let Some(artifacts) = request.get_mut("artifacts").and_then(Value::as_array_mut) {
        for artifact in artifacts {
            canonical_argument(policy, artifact, "path", false)?;
        }
    }
    canonical_optional_argument(policy, request, "dmb_path")?;
    Ok(())
}

pub(crate) fn require_launchable_artifact(
    context: &ToolExecutionContext,
    dmb_path: &std::path::Path,
    require_verified: bool,
) -> std::result::Result<crate::LaunchProvenance, ToolResult> {
    let dmb = match crate::FileIdentity::capture(dmb_path) {
        Ok(dmb) => dmb,
        Err(error) => {
            return Err(structured_error(
                ToolErrorCode::InvalidInput,
                "could not identify the DMB immediately before launch",
                Some("Use an existing contained regular DMB file.".to_owned()),
                json!({"dmb_path":dmb_path,"error":error.to_string()}),
            ));
        }
    };
    let decision = match context.build_provenance() {
        Some(store) => match store.evaluate_launch(dmb_path, require_verified) {
            Ok(decision) => decision,
            Err(error) => {
                return Err(structured_error(
                    ToolErrorCode::WorkspaceIntegrityViolation,
                    "managed build provenance could not be validated",
                    Some(
                        "Inspect the private state store and compile the artifact again."
                            .to_owned(),
                    ),
                    json!({"dmb_path":dmb_path,"error":error.to_string()}),
                ));
            }
        },
        None => crate::LaunchDecision {
            status: crate::ProvenanceStatus::Unverified,
            allowed: !require_verified,
            record_id: None,
            reasons: vec![crate::ProvenanceReason {
                code: "no_build_record".to_owned(),
                message: "no managed successful build record exists for this artifact".to_owned(),
                role: None,
                path: Some(dmb.path.clone()),
            }],
        },
    };
    let launch = crate::LaunchProvenance {
        status: decision.status,
        build_record_id: decision.record_id,
        dmb_sha256: dmb.sha256,
        warnings: decision.reasons,
    };
    if decision.allowed {
        Ok(launch)
    } else {
        let code = if launch.status == crate::ProvenanceStatus::Stale {
            "stale_build_artifact"
        } else {
            "build_provenance_unavailable"
        };
        Err(structured_error(
            ToolErrorCode::WorkspaceIntegrityViolation,
            code,
            Some("Compile the DMB through Meridian-MCP and retry the launch.".to_owned()),
            json!({"provenance":launch}),
        ))
    }
}

fn canonical_argument(
    policy: &PathPolicy,
    args: &mut Value,
    key: &str,
    runtime: bool,
) -> std::result::Result<(), crate::PolicyError> {
    if let Some(path) = args.get(key).and_then(Value::as_str) {
        let path = if runtime {
            policy.runtime_dmb(path)?
        } else {
            policy.read_path(path)?
        };
        args[key] = Value::String(path.display().to_string());
    }
    Ok(())
}

fn canonical_optional_argument(
    policy: &PathPolicy,
    args: &mut Value,
    key: &str,
) -> std::result::Result<(), crate::PolicyError> {
    canonical_argument(policy, args, key, false)
}

fn policy_error(
    code: &str,
    message: String,
    path: Option<&std::path::Path>,
    recovery: &str,
    details: Value,
) -> ToolResult {
    ToolResult::error(
        json!({
            "code": code,
            "message": message,
            "recovery": recovery,
            "details": details,
            "path": path.map(|path| path.display().to_string())
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::{bounded_u64, get_tool_definitions};
    use serde_json::json;

    #[test]
    fn tool_definitions_use_supported_name_prefixes() {
        let definitions = get_tool_definitions();

        assert!(!definitions.is_empty());
        assert!(definitions
            .iter()
            .all(|tool| tool.name.starts_with("dm_") || tool.name == "rift_compile"));
    }

    #[test]
    fn debugger_launch_schema_exposes_interactive_and_headless_hosts() {
        let definitions = get_tool_definitions();
        let launch = definitions
            .iter()
            .find(|tool| tool.name == "dm_debug_launch")
            .expect("dm_debug_launch should be defined");

        assert_eq!(
            launch.input_schema["properties"]["host_mode"]["enum"],
            json!(["interactive", "headless"])
        );
        assert_eq!(
            launch.input_schema["properties"]["host_mode"]["default"],
            "interactive"
        );
    }

    #[test]
    fn context_search_schema_requires_a_query_and_exposes_filters() {
        let definitions = get_tool_definitions();
        let search = definitions
            .iter()
            .find(|tool| tool.name == "dm_search_context")
            .expect("context search tool should be registered");

        assert_eq!(
            search.input_schema["required"],
            serde_json::json!(["query"])
        );
        assert_eq!(
            search.input_schema["properties"]["kind"]["enum"],
            serde_json::json!(["all", "type", "proc", "var"])
        );
        assert_eq!(search.input_schema["properties"]["limit"]["maximum"], 50);
        assert_eq!(
            search.input_schema["properties"]["max_source_lines"]["maximum"],
            200
        );
    }

    #[test]
    fn diagnostic_schema_exposes_bounded_snapshot_filters() {
        let definitions = get_tool_definitions();
        let diagnostics = definitions
            .iter()
            .find(|tool| tool.name == "dm_check_errors")
            .expect("dm_check_errors should be registered");

        assert!(diagnostics.description.contains("cached"));
        assert_eq!(
            diagnostics.input_schema["properties"]["limit"]["default"],
            50
        );
        assert_eq!(
            diagnostics.input_schema["properties"]["limit"]["maximum"],
            100
        );
        assert_eq!(
            diagnostics.input_schema["properties"]["severity"]["enum"],
            json!(["error", "warning", "info", "hint"])
        );
        assert_eq!(
            diagnostics.input_schema["properties"]["component"]["enum"],
            json!(["parser", "dreamchecker"])
        );
    }

    #[test]
    fn enumeration_schemas_expose_hard_result_bounds() {
        let definitions = get_tool_definitions();
        let types = definitions
            .iter()
            .find(|tool| tool.name == "dm_list_types")
            .expect("type listing tool should be registered");
        let symbols = definitions
            .iter()
            .find(|tool| tool.name == "dm_search_symbols")
            .expect("symbol search tool should be registered");

        assert_eq!(types.input_schema["properties"]["limit"]["maximum"], 500);
        assert_eq!(
            types.input_schema["properties"]["cursor"]["pattern"],
            "^[0-9]+$"
        );
        assert_eq!(symbols.input_schema["properties"]["limit"]["maximum"], 200);
    }

    #[test]
    fn bounded_numeric_arguments_enforce_type_and_range() {
        assert_eq!(
            bounded_u64(&json!({}), "timeout_ms", 30, 1, 60).unwrap(),
            30
        );
        assert_eq!(
            bounded_u64(&json!({"timeout_ms":60}), "timeout_ms", 30, 1, 60).unwrap(),
            60
        );
        assert!(bounded_u64(&json!({"timeout_ms":0}), "timeout_ms", 30, 1, 60).is_err());
        assert!(bounded_u64(&json!({"timeout_ms":61}), "timeout_ms", 30, 1, 60).is_err());
        assert!(bounded_u64(&json!({"timeout_ms":"60"}), "timeout_ms", 30, 1, 60).is_err());
    }
}
