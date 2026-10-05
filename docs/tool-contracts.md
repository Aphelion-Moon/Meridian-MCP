# Tool contracts

Generated from `src/contracts.rs`; do not edit by hand.

Startup `MERIDIAN_MCP_TOOL_PROFILE` selects `all` (default), `code`, `assets` or `runtime`. Profile membership only narrows the existing mode, helper and authorization gates; it never enables unavailable capabilities. Status and parsing are shared by every profile.

Max output bytes bounds the serialized SDK `CallToolResult` body, including text/structured duplication, JSON escaping, metadata and `resultType`. The client-owned JSON-RPC request ID is outside this cap. Output schemas and native structured content are advertised for MCP 2025-06-18 and newer; all versions retain compatibility text. Mutation outcomes survive optional-detail reduction.

| Tool | Profiles | Mode | Support | Effects | Timeout ms | Max output bytes | Summary |
| --- | --- | --- | --- | --- | ---: | ---: | --- |
| `dm_audit_icons` | all, assets | Analysis | Experimental | read | - | 1048576 | Audit parsed icon evidence and duplicate DMI states. |
| `dm_check_errors` | all, code | Analysis | Provisional | memory | - | 1048576 | Read bounded cached parser and DreamChecker diagnostics. |
| `dm_check_fixture_sync` | all, code | Analysis | Experimental | read | - | 1048576 | Validate declared fixture source contracts and build provenance. |
| `dm_compare_dmi_states` | all, assets | Analysis | Provisional | read | - | 1048576 | Compare complete DMI states including common lazy changes. |
| `dm_compile` | all, code, runtime | Development | Provisional | read, write, process, destructive | 1800000 | 1048576 | Run an allowlisted DreamMaker compiler gate. |
| `dm_debug_control` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 262144 | Pause, continue, or step the active debuggee. |
| `dm_debug_evaluate` | all, runtime | Development | Experimental | read, process, loopback, project behavior, destructive | 30000 | 1048576 | Evaluate an expression in the active debuggee. |
| `dm_debug_exception_info` | all, runtime | Development | Experimental | read, process, loopback | - | 262144 | Read the last retained runtime exception. |
| `dm_debug_launch` | all, runtime | Development | Experimental | read, process, loopback | 60000 | 262144 | Launch one owned interactive or headless auxtools session. |
| `dm_debug_memory` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | Control bounded, opt-in native allocation attribution. |
| `dm_debug_scopes` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | Read variable scopes for a debug frame. |
| `dm_debug_set_breakpoints` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | Replace source-oriented auxtools breakpoints. |
| `dm_debug_set_exception_breakpoints` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 262144 | Toggle breaks on DreamMaker runtimes. |
| `dm_debug_set_function_breakpoints` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | Set canonical proc breakpoints. |
| `dm_debug_source` | all, runtime | Development | Experimental | read, process, loopback | - | 1048576 | Read the retained auxtools standard-definition source. |
| `dm_debug_stack_trace` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | Read bounded debuggee stack frames. |
| `dm_debug_stop` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 262144 | Disconnect and terminate the owned debugger session. |
| `dm_debug_threads` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | List auxtools debuggee stacks. |
| `dm_debug_variables` | all, runtime | Development | Experimental | read, process, loopback | 30000 | 1048576 | Read a bounded variable-reference page. |
| `dm_debug_wait_for_event` | all, runtime | Development | Experimental | read, process, loopback | 300000 | 1048576 | Wait for a bounded debugger event. |
| `dm_diff_maps` | all, assets | Analysis | Provisional | read | - | 1048576 | Compare coordinate models across two DMM/TGM maps. |
| `dm_dmi_info` | all, assets | Analysis | Provisional | read | - | 1048576 | Profile DMI metadata and frame pixels without altering art. |
| `dm_document_symbols` | all, code | Analysis | Provisional | read | - | 1048576 | List declarations in one parsed source file. |
| `dm_extract_dmi` | all, assets | Development | Experimental | read, write, destructive | - | 262144 | Mechanically extract a selected DMI state without altering source art. |
| `dm_find_dmi_duplicates` | all, assets | Analysis | Experimental | read | - | 1048576 | Find cross-file exact and lazy-change DMI duplicates. |
| `dm_find_implementations` | all, code | Analysis | Provisional | memory | - | 1048576 | Find type or member implementations. |
| `dm_find_on_map` | all, assets | Analysis | Provisional | read | - | 1048576 | Find exact type instances in a DMM/TGM map. |
| `dm_find_references` | all, code | Analysis | Experimental | read | - | 1048576 | Find bounded exact member references. |
| `dm_generate_docs` | all, code | Development | Experimental | read, write, process, destructive | 600000 | 262144 | Generate contained HTML through the verified exact dmdoc helper. |
| `dm_get_definition` | all, code | Analysis | Provisional | memory | - | 262144 | Locate an exact parsed definition. |
| `dm_get_proc` | all, code | Analysis | Provisional | read | - | 1048576 | Inspect exact proc implementations and source excerpts. |
| `dm_get_type` | all, code | Analysis | Provisional | memory | - | 1048576 | Inspect an exact DreamMaker type. |
| `dm_get_var` | all, code | Analysis | Provisional | memory | - | 262144 | Inspect a DreamMaker variable through semantic inheritance, with value and declaration ownership. |
| `dm_list_render_passes` | all, assets | Analysis | Provisional | memory | - | 262144 | List pinned SpacemanDMM render-pass behavior. |
| `dm_list_types` | all, code | Analysis | Provisional | memory | - | 1048576 | List parsed types under an optional prefix. |
| `dm_map_info` | all, assets | Analysis | Provisional | read | - | 1048576 | Read DMM/TGM dimensions and atom statistics. |
| `dm_memory_compare` | all, runtime | Analysis | Experimental | read | 30000 | 1048576 | Compare process memory for matching recorded builds and workloads. |
| `dm_memory_summary` | all, runtime | Analysis | Experimental | read | 30000 | 1048576 | Summarize sampled process memory over a selected time window. |
| `dm_native_evidence_compare` | all, runtime | Analysis | Experimental | read | 600000 | 1048576 | Compare identity-compatible native evidence runs. |
| `dm_native_evidence_summary` | all, runtime | Analysis | Experimental | read | 120000 | 1048576 | Summarize bounded redacted native runtime evidence. |
| `dm_parse_environment` | all, code, assets, runtime | Analysis | Provisional | read | 1800000 | 1048576 | Parse and atomically install DreamMaker analysis and lexical indexes. |
| `dm_render_map` | all, assets | Development | Provisional | read, write, destructive | - | 262144 | Render a contained DMM/TGM map output. |
| `dm_render_maps` | all, assets | Development | Experimental | read, write, destructive | - | 1048576 | Render a bounded typed batch of map chunks. |
| `dm_run` | all, runtime | Development | Provisional | read, write, process, loopback | 300000 | 1048576 | Start a contained DreamDaemon program on loopback. |
| `dm_search_context` | all, code | Analysis | Provisional | read | - | 1048576 | Rank lexical candidates from parsed symbols and source context. |
| `dm_search_symbols` | all, code | Analysis | Provisional | memory | - | 1048576 | Search parsed symbol names. |
| `dm_server_status` | all, code, assets, runtime | Analysis | Provisional | memory | - | 262144 | Report immutable startup policy, build identity, analysis generation, and owned runtime summary. |
| `dm_status` | all, runtime | Development | Provisional | read, write | - | 1048576 | Inspect server-owned DreamDaemon state. |
| `dm_stop` | all, runtime | Development | Provisional | read, write, destructive | - | 262144 | Stop the server-owned DreamDaemon process. |
| `dm_topic` | all, runtime | Development | Provisional | loopback, project behavior, destructive | 60000 | 262144 | Call world.Topic on the loopback game server. |
| `dm_tracy_capture` | all, runtime | Development | Experimental | read, write, process, loopback | 330000 | 1048576 | Rotate the persistent collector for one validated window and publish an atomic `.tracy` plus schema-2 sidecar pair. |
| `dm_tracy_compare` | all, runtime | Development | Experimental | read, process | 180000 | 1048576 | Compare two traces by proc source identity. |
| `dm_tracy_control_stats` | all, runtime | Development | Experimental | read, process | 2400000 | 1048576 | Validate 3-20 repeated Tracy controls and calculate fixed noise statistics. |
| `dm_tracy_frame_stats` | all, runtime | Development | Experimental | read, process | 120000 | 262144 | Summarize ServerTick frame durations. |
| `dm_tracy_hotspots` | all, runtime | Development | Experimental | read, process | 120000 | 1048576 | Return bounded deterministic trace hotspots. |
| `dm_tracy_launch` | all, runtime | Development | Experimental | read, write, process, loopback | 600000 | 262144 | Launch an MCP-owned profiled DreamDaemon on loopback. |
| `dm_tracy_prepare` | all, runtime | Development | Experimental | read, write, destructive | - | 262144 | Install the verified byond-tracy hook beside a contained DMB. |
| `dm_tracy_status` | all, runtime | Development | Experimental | loopback | - | 262144 | Inspect profiled runtime and capture state. |
| `dm_tracy_stop` | all, runtime | Development | Experimental | read, write, loopback, destructive | 30000 | 262144 | Stop capture and the profiled DreamDaemon. |
| `dm_tracy_zone` | all, runtime | Development | Experimental | read, process | 120000 | 1048576 | Inspect one profiled proc across source locations. |
| `dm_wait_for_output` | all, runtime | Development | Provisional | read, write | 300000 | 1048576 | Wait for bounded server-owned DreamDaemon output. |
| `rift_compile` | all, code, runtime | Development | Provisional | read, write, process, network, destructive | 1800000 | 1048576 | Run Meridian-Rift's contained RIFT_BUILD.cmd full-build gate; reserve outer cleanup time and validate its versioned artifact result. |
