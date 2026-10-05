# Meridian-MCP

Meridian-MCP lets AI coding assistants search and inspect DreamMaker / SS13 projects through the Model Context Protocol (MCP). It provides code navigation, static diagnostics, icon and map inspection, and optional BYOND build, debugging and profiling tools.

**Analysis mode is read-only and enabled by default.** Development mode adds compilation, generated files and runtime control when explicitly configured.

- **Code:** search a large repository, inspect exact symbols, and find definitions, references and diagnostics.
- **Assets:** inspect and compare DMI icons; search, diff and render DMM/TGM maps.
- **Development:** compile, run a local world, call project test hooks, debug with auxtools, and profile with Tracy.

[Quick start](#five-minute-analysis-setup) · [Development setup](#development-setup) · [Workflows](#common-workflows) · [Tool reference](#complete-tool-reference) · [Testing](TESTING.md)

## Authority and safety boundaries

Parsing checks source with SpacemanDMM and DreamChecker. It does **not** prove that DreamMaker compiles the project or that the game works. Use the repository's documented build and test commands for those checks.

The server only accesses roots authorized at startup. Development executables must be allowlisted, runtime connections stay on loopback, and process controls target only server-owned processes. Tool calls cannot expand these permissions. See the [security model](docs/security.md) for details.

## Five-minute analysis setup

For Codex on Windows, with Rust and the Windows C++ build tools installed:

1. Build the release binary with the repository-pinned Rust toolchain:

   ```powershell
   cargo +1.95.0 build --locked --release
   ```

2. Add this entry to your existing Codex `config.toml`. Replace both example paths with existing absolute paths:

   ```toml
   [mcp_servers.meridian-mcp]
   command = 'C:\path\to\meridian-mcp\target\release\meridian-mcp.exe'

   [mcp_servers.meridian-mcp.env]
   MERIDIAN_MCP_MODE = 'analysis'
   MERIDIAN_MCP_ROOTS = 'C:\path\to\Meridian-Rift'
   ```

3. Restart Codex, call `dm_server_status`, then call `dm_parse_environment` with the contained `.dme` path. Parse again after source changes; an unchanged environment reuses the active snapshot rather than reparsing.

Analysis mode needs no BYOND installation. HTML generation requires development mode and a packaged, verified dmdoc helper.

## Development setup

Use the installer and configuration updater to validate the binary, helpers, authorized roots and private state directory. Requirements depend on the tools you enable:

| Capability | Additional prerequisite |
| --- | --- |
| Direct compilation and normal runtime | An allowlisted BYOND installation containing `dm.exe` and its sibling runtime executables |
| Meridian-Rift full build | Windows, an authorized Meridian-Rift checkout, and `MERIDIAN_MCP_RIFT_BUILD=offline` or `network` |
| HTML documentation | Exact dmdoc helper recorded by the helper manifest |
| Auxtools debugging | Packaged auxtools v2.3.7 DLL and the x86 Microsoft Visual C++ runtime |
| Tracy profiling | Manifest-verified host helper and x86 byond-tracy hook for the supported BYOND baseline |

The complete packaging example is in [Operator and contributor reference](#operator-and-contributor-reference). At minimum, development configuration sets `MERIDIAN_MCP_MODE=development`, supplies one or more roots, allowlists the compiler, and points `MERIDIAN_MCP_STATE_DIR` to an existing writable directory outside every workspace root. Restart the MCP client after changing startup authorization.

### Verify a Codex installation

Fully quit and reopen Codex after installing a binary or changing MCP settings. Closing a task alone does not restart the server.

From a terminal, run `codex mcp get meridian-mcp` and confirm the enabled entry points to the intended binary. Then check through Codex:

1. `dm_server_status`: confirm `mcp_build.complete: true`, the expected revision/hash and enabled capabilities.
2. `dm_parse_environment`: load an authorized `.dme`; expect `success: true` and `retrieval.lexical.status: ready`.
3. Repeat the parse: expect `reused: true` and the same `state_generation`.
4. `dm_check_errors`: expect `analysis.source: cached_snapshot` and `recomputed: false`.
5. `dm_search_context`: try a project-specific query and inspect an exact result.

A missing include or source diagnostic may be a checkout problem, not an installation failure. Read the error before changing files. Each restarted session needs a fresh parse.

## Common workflows

### Analyze code

Use text search for literal names, file discovery and questions spanning DM and other languages. Use the parser when you need inheritance, declaration ownership, resolved references or checker diagnostics. The [exploration comparison](docs/audits/2026-09-06-exploration-comparison/README.md) records the measured tradeoffs.

1. Before using parsed source tools, call `dm_parse_environment` with an authorized `.dme`.
2. For a known symbol, go directly to `dm_get_type`, `dm_get_proc`, `dm_get_var`, or `dm_get_definition`. For discovery, use `dm_search_context` or partial-name `dm_search_symbols`.
3. Use references, implementations, document symbols and diagnostics for impact analysis. Reparse after source changes. Parser success is not compiler success.

An unchanged environment reuses its snapshot. Reuse checks file paths, sizes and modification times, not content hashes; use `force: true` when you need a full reparse. Responses report timings and the active generation. Performance depends on the project and machine.

Tool outputs use generated, named schemas. Clients negotiating MCP 2025-06-18 or newer receive `outputSchema` and native `structuredContent`; every supported version retains compatibility text from the same projection. Earlier initialized clients receive text only. The declared output limit counts the serialized SDK result body, including duplicated structured/text content, JSON escaping, metadata and `resultType`; the client-owned JSON-RPC ID is outside that limit. Under final output pressure, mutation replies retain the operation outcome, installed output hashes, per-item status and cleanup/recovery facts while marking optional details omitted.

Semantic responses identify the captured snapshot in `analysis` and MCP `_meta.analysis`; legacy plain-text errors retain their text and carry the identity in `_meta.analysis`. The block includes `snapshot_id`, generation, environment, cached source and completeness. `disk_state: "unknown"` makes no claim that current files still match. Optional `expected_snapshot` rejects stale handles before semantic work; each accepted call uses one captured snapshot even if another parse finishes meanwhile. Reuse keeps the ID, a successful replacement changes it, and a server restart invalidates prior IDs. Failed parses retain the active identity and report `details.requested_environment` separately.

Search uses lexical BM25 ranking; embeddings and vector search are not configured. For a known symbol, use exact lookup. Procedure results distinguish the **implementation owner** (executable body) from the **declaration owner** (declaration metadata).

To reduce response size, set `include_source: false` on `dm_get_proc` or `dm_search_context`. Otherwise, `max_source_lines` accepts 1–200 lines (defaults: 80 for inspection, 40 for search). Source lines and encoded bytes are shared across the returned procedures or search hits, with at most 200 lines in one response. Documentation, constant previews and member work also share bounded budgets. Excerpts report their snapshot boundaries and truncation; see the [source-excerpt audit](docs/audits/2026-09-06-source-excerpts/README.md).

`dm_get_type` keeps its default full-detail fields for small types. To select sections, pass `sections` containing `documentation`, `vars`, `procs` or `children`; `detail: "compact"` omits constant previews. Large types expose `pagination.next_cursor` over the selected variables, procedures and children, in that order. Follow it with the same type, section selection and detail; `limit` may change. A large individual field reports `field_omissions`, and a byte-limited page still advances. Type listings likewise reduce the row count when needed to retain complete paths and a continuation cursor.

Implementation listings cover the requested type and its semantic descendants. Use `dm_get_proc` to inspect an inherited body outside that subtree, or query its declaration owner for the wider implementation family.

Diagnostics come from the last successful parse. Filter by file, severity, component or rule, and follow `pagination.next_cursor` for more results. `truncated: true` with `diagnostic_page_limit` means another page is available.

Reference, implementation and document-symbol listings default to 100 rows and also stop at a response byte budget. Follow `pagination.next_cursor` with the same query, filters and detail to retrieve the complete `total_count`; page size may change. Type and diagnostic listings use the same volatile v2 cursor contract. Numeric cursors and v1 language cursors are intentionally rejected; start a new first page. A reparse or server restart invalidates all cursors. Use `detail: "compact"` for shorter responses: fields under `shared` apply to every row, and each row supplies its remaining fields. `detail: "full"` is the default. Reference `include_declaration: true` adds a row labeled `declaration`; `skipped_dynamic` counts unresolved expressions across the whole environment.

### Compile and exercise a world

`dm_compile` runs DreamMaker directly. For Meridian-Rift's full build, `rift_compile` runs the separate `RIFT_BUILD.cmd`; it does not replace the human `BUILD.cmd` workflow.

After building, use `dm_run` → `dm_wait_for_output` → `dm_topic` / `dm_status` → `dm_stop`. Topic requests need a test handler supplied by the project. See [build provenance and runtime integrity](#operational-details) for stale-artifact checks.

Standard, debugger and Tracy launches issue a `runtime_id`. Follow-up controls accept optional `expected_runtime`; a stale expectation fails before reaching a replacement session. Retained runtime output keeps its original ID after process exit, and pending waits remain attached to that output. Runtime analysis metadata describes the matching snapshot captured at launch. Native debugger `stddef.dm` content is labeled separately as `native_debugger_stddef`.

Launch options and readiness regexes are validated before starting a process. Invalid options return `invalid_input`; flags such as `require_verified_provenance` must be JSON booleans. Output waits accept `timeout_ms: 0` for an immediate check and cap longer waits at five minutes.

For `dm_compile`, `working_directory` also resolves relative DME paths. Malformed options are rejected before compilation. `success` requires a DMB as well as a successful compiler exit; check `dmb_updated` and provenance separately for freshness and verified inputs.

Both build tools return bounded log tails and diagnostic details. Use `include_output: false` for shorter replies, or adjust `output_max_bytes` and `diagnostic_limit`. Errors and Rift wrapper evidence are read before log eviction; incomplete analysis prevents build success. See [compiler output](docs/compiler-output.md).

DreamDaemon binds to `127.0.0.1`. Set `working_directory` to resolve a relative DMB and run the game from that directory; otherwise it runs from the DMB's directory. Additional `daemon_args` cannot override the DMB, port, directory or bind address. World parameters passed with `-params` remain supported.

`dm_topic` uses one timeout for connection, delivery and response, from 1 to 60,000 ms (default 5,000). Replacement is excluded through packet delivery. Status and stop remain available while it waits for the response; stopping its runtime cancels the pending request.

### Inspect icons and maps

Use `dm_dmi_info` before comparing or extracting DMI states. `dm_find_dmi_duplicates` looks across contained scopes for exact matches plus cropped, palette-only, mirrored, rotated, and scaled copies. `dm_audit_icons` checks statically resolvable inherited `icon` and `icon_state` references. Generated PNGs are written only by the explicit development tools and only to contained paths.

For maps, use `dm_map_info` for structure, `dm_find_on_map` for type-path occurrences, `dm_diff_maps` for coordinate-model differences, and `dm_list_render_passes` before `dm_render_map` or `dm_render_maps`.

### Debug with auxtools

Compile and parse matching source, launch the contained DMB with `dm_debug_launch`, configure breakpoints, continue or step, and inspect threads, frames, scopes, and variables after a stop event. The adapter owns one Windows BYOND host and does not attach to arbitrary processes. See the [detailed debugger workflow](#debugger-workflow) and the per-tool descriptions below.

### Profile with Tracy

Prepare the verified hook, launch an owned profiling runtime, capture one or more bounded windows, and inspect hotspots, zones, frames, comparisons, and repeated controls offline. Each accepted trace is paired with an identity and queue-health sidecar. See the [Tracy profiler](#tracy-profiler) reference and [native evidence analysis](docs/native-evidence.md).

## Capability and platform status

Core analysis, compilation, map tools and runtime controls are **provisional**. DMI analysis, HTML documentation, auxtools and Tracy are **experimental**. Optional tools appear only when their startup prerequisites are met.

Windows and Ubuntu have separate test evidence; macOS is unsupported and untested. Auxtools and Meridian-Rift's full-build wrapper are Windows-only. The inherited BYOND client login protocol is unsupported.

See [compatibility and evidence](docs/compatibility.md) for support definitions, tested versions and remaining integration gates. A passing fixture does not establish full-game compatibility.

The [SpacemanDMM support audit](docs/audits/2026-09-06-spacemandmm-support.md) maps the pinned parser, checker, language, icon, map, documentation and debugger capabilities to MCP tools and explains the editor and legacy-backend exclusions.

## Complete tool reference

Analysis tools are read-only. Development mode adds compilation, file generation and runtime control. Full builds, debugging and Tracy need additional startup settings. See [tool contracts](docs/tool-contracts.md) for each tool's permissions, side effects, limits and support status.

### Analysis mode

| Tool | Description |
| --- | --- |
| `dm_server_status` | Show build identity, enabled capabilities, authorized roots, analysis generation and runtime state. |
| `dm_parse_environment` | Load a `.dme` and its search index, or reuse the unchanged snapshot. |
| `dm_check_fixture_sync` | Check fixture signatures and inputs against parsed source and available build records. |
| `dm_get_type` | Inspect an exact type, its members and inheritance. |
| `dm_get_proc` | Inspect a procedure, its implementations, owners and source. |
| `dm_get_var` | Inspect a variable declaration, value and source location. |
| `dm_list_types` | List type paths with prefix/depth filters and pagination. |
| `dm_search_symbols` | Find symbols by partial name. |
| `dm_search_context` | Search symbols, documentation and source with lexical BM25 ranking. |
| `dm_check_errors` | Read and filter cached parser and DreamChecker diagnostics. |
| `dm_get_definition` | Find the definition of an exact type, variable or procedure. |
| `dm_document_symbols` | List declarations in one source file. |
| `dm_find_references` | Find resolved references to a declaration; dynamic accesses may be unresolved. |
| `dm_find_implementations` | List parent and child implementations of a symbol. |
| `dm_dmi_info` | Inspect icon states, frames, dimensions, hashes and warnings. |
| `dm_compare_dmi_states` | Compare states for exact, cropped, recolored, mirrored, rotated or scaled copies. |
| `dm_find_dmi_duplicates` | Find exact and transformed duplicates across icons. |
| `dm_audit_icons` | Check static icon references and report missing or unresolved states. |
| `dm_map_info` | Show map dimensions, tile counts and common types. |
| `dm_diff_maps` | Compare map contents by coordinate, ignoring dictionary keys and variable ordering while preserving atom order. |
| `dm_list_render_passes` | List available map render passes. |
| `dm_find_on_map` | Find a type and its descendants on a map. |
| `dm_native_evidence_summary` | Summarize local runtime artifacts with hashes, redaction and separate clock domains. |
| `dm_memory_summary` | Summarize recorded process-memory growth, peaks and sampling gaps in a selected time window. |
| `dm_memory_compare` | Compare memory windows with matching recorded builds, workloads and metric types. |
| `dm_native_evidence_compare` | Compare verified, matching builds and workloads across repeated measurements. |

`dm_check_fixture_sync` evaluates every declared requirement and returns complete issue counts. Set `issue_limit` from 0 to 200 (default 50); zero returns counts without issue details. `validation_complete` distinguishes evaluated requirements from an invalid manifest, while `truncated` reports omitted details. Map loading rejects grids above 16,777,216 cells before allocation, including sparse maps with extreme coordinates.

### Development mode

| Tool | Description |
| --- | --- |
| `dm_compile` | Run an allowlisted DreamMaker compiler directly; return diagnostics and build evidence. |
| `rift_compile` | Run the fixed Windows Meridian-Rift full-build wrapper within its startup permissions. |
| `dm_render_map` | Render one map z-level to an authorized PNG path; replacement requires explicit overwrite. |
| `dm_render_maps` | Render a bounded batch of map chunks. |
| `dm_extract_dmi` | Export an icon frame or state contact sheet as PNG. |
| `dm_generate_docs` | Generate HTML using the packaged, verified dmdoc helper. |
| `dm_run` | Start one owned loopback DreamDaemon; reject known-stale managed artifacts. |
| `dm_wait_for_output` | Wait for a literal or regular-expression readiness marker. |
| `dm_status` | Show runtime, build provenance and workspace-integrity status. |
| `dm_stop` | Stop the owned DreamDaemon and finalize integrity checks. |
| `dm_topic` | Call a project-provided `world.Topic()` handler on the owned runtime. |

`dm_generate_docs` needs a parsed project and an output directory whose parent exists. `overwrite=true` replaces that directory; files, directory links and outputs containing source, configuration, Markdown or index inputs are rejected. Live inputs are checked again before installation. Use `include_output=false` to omit helper logs, or `output_max_bytes` to cap each returned stream (default 8 KiB, maximum 64 KiB).

On a documentation error, check `installed` and `cleanup_complete`: new HTML may already be installed while an old backup needs cleanup. Linux output filesystems must support no-replace directory renames; when a mounted Windows drive is rejected, use an authorized output path on native Linux storage. Unsupported filesystems are rejected before the helper runs.

Atomic file outputs also require filesystem support for no-replace renames. If another writer creates the destination during generation, Meridian preserves it. A failed restoration reports the retained backup path for recovery.

### Auxtools debugger

Enable `MERIDIAN_MCP_DEBUGGER=auxtools` in development mode with the verified auxtools v2.3.7 DLL and x86 Microsoft Visual C++ runtime. The adapter is Windows-only and experimental.

| Tool | Description |
| --- | --- |
| `dm_debug_launch` | Launch an owned BYOND host: interactive DreamSeeker by default, or headless DreamDaemon. |
| `dm_debug_set_breakpoints` | Replace source breakpoints for one parsed file. |
| `dm_debug_set_function_breakpoints` | Replace breakpoints using exact procedure identities. |
| `dm_debug_set_exception_breakpoints` | Choose whether to break on runtime exceptions. |
| `dm_debug_control` | Pause, continue, step into, step over or step out. |
| `dm_debug_threads` | List debugger threads and their identifiers. |
| `dm_debug_stack_trace` | Read a page of stack frames for a thread. |
| `dm_debug_scopes` | Get argument, local and global variable references for a frame. |
| `dm_debug_variables` | Read a page of values from a debugger variable reference. |
| `dm_debug_evaluate` | Evaluate an expression in the debuggee; this can change game state. |
| `dm_debug_memory` | Start or stop bounded allocation attribution with the optional Windows memory helper. |
| `dm_debug_exception_info` | Read the latest retained runtime exception. |
| `dm_debug_source` | Read the session-provided `stddef.dm` through its issued reference. |
| `dm_debug_wait_for_event` | Wait for events after a sequence number; report dropped events. |
| `dm_debug_stop` | Disconnect and terminate the owned debugger process tree. |

#### Debugger workflow

1. Compile and parse matching source, then call `dm_debug_launch`.
2. Set the complete desired breakpoint list; each setter replaces its active set.
3. Continue or step with `dm_debug_control`, then wait with `dm_debug_wait_for_event` using the last event sequence.
4. Inspect `dm_debug_threads` → `dm_debug_stack_trace` → `dm_debug_scopes` → `dm_debug_variables`.
5. Call `dm_debug_stop` before rebuilding, reparsing changed source or ending the session.

Only one runtime can be active: normal, debugger or Tracy. The debugger does not support attaching to an existing process or selecting an arbitrary DLL. Expression evaluation runs game code and may have side effects.

### Tracy profiler

Enable `MERIDIAN_MCP_TRACY=byond` in development mode with both verified native helpers. The live baseline uses Tracy protocol 82 and BYOND 516.1687. See [Tracy setup and capture](docs/tracy-profiling.md) for packaging, supported versions and headless-world wake behavior.

| Tool | Description |
| --- | --- |
| `dm_tracy_prepare` | Install the verified profiling hook; replacing a different file requires explicit overwrite. |
| `dm_tracy_launch` | Start an owned profiling runtime and collector, then verify producer readiness. |
| `dm_tracy_capture` | Capture a bounded window and publish a validated trace with its evidence sidecar. |
| `dm_tracy_status` | Show runtime, collector, capture and queue-health status. |
| `dm_tracy_stop` | Stop the owned profiling session and finalize its integrity journal. |
| `dm_tracy_hotspots` | Rank procedures by time or call count. |
| `dm_tracy_zone` | Summarize an exact profiled procedure. |
| `dm_tracy_frame_stats` | Report ServerTick frame statistics and percentiles. |
| `dm_tracy_compare` | Compare two traces by procedure, file and line. |
| `dm_tracy_control_stats` | Check 3–20 compatible captures for variation and baseline eligibility. |

Offline trace analysis does not require a parsed environment. An active parse adds source correlation without changing measurements. Accepted traces retain identity and queue-health evidence; raw artifacts stay local.

## Operator and contributor reference

### Rust build and verification

The repository pins Rust 1.95.0 with rustfmt and Clippy to match CI. BYOND integration gates additionally require the project-pinned BYOND version. See [CONTRIBUTING.md](CONTRIBUTING.md) before making code changes.

```powershell
cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.95.0 test --locked --all-features
cargo +1.95.0 build --locked --release
cargo +1.95.0 deny check
```

The release binary is `target\release\meridian-mcp.exe` on Windows and `target/release/meridian-mcp` on Linux.

### Repository command inventory

Run scripts from the repository root. [TESTING.md](TESTING.md) lists exact commands, prerequisites and integration gates.

| Entry point | Use |
| --- | --- |
| `test_mcp.ps1` / `test_parse.ps1` | Verify a built binary over stdio, optionally against a real DME. |
| `scripts/build-spacemandmm-helpers.ps1` | Package dmdoc from the pinned upstream checkout. |
| `scripts/build-tracy-helpers.ps1` | Build and verify the pinned native Tracy helper and hook. |
| `scripts/fetch-auxtools.ps1` | Download and verify the pinned debugger DLL. |
| `scripts/install-meridian-mcp.ps1` | Install the binary and verified helpers; prepare private state. |
| `scripts/configure-codex-meridian-mcp.ps1` | Update an existing server entry while preserving unrelated settings. |
| `scripts/run-tracy-experiment.ps1` | Capture repeated controls and retain local evidence. |
| `scripts/validate-tracy-evidence.ps1` | Validate existing evidence without launching BYOND. |

### Packaging and configuration example

```powershell
./scripts/build-spacemandmm-helpers.ps1 `
    -UpstreamPath C:\path\to\SpacemanDMM `
    -OutputDirectory ./target/package `
    -ManifestPath ./target/package/helpers/manifest.json

./scripts/fetch-auxtools.ps1 -DestinationRoot ./target/package

./scripts/install-meridian-mcp.ps1 `
    -BinaryPath ./target/release/meridian-mcp.exe `
    -HelperManifestPath ./target/package/helpers/manifest.json `
    -AuxtoolsRoot ./target/package `
    -DestinationRoot C:\path\to\installed-meridian-mcp `
    -InstalledName meridian-mcp.exe `
    -WorkspaceRoots C:\path\to\Meridian-Rift `
    -RepositoryRoots C:\path\to\Meridian-Rift `
    -StateDirectory C:\path\to\private-meridian-state `
    -Development

./scripts/configure-codex-meridian-mcp.ps1 `
    -ConfigPath C:\path\to\.codex\config.toml `
    -BinaryPath C:\path\to\installed-meridian-mcp\meridian-mcp.exe `
    -HelperManifestPath C:\path\to\installed-meridian-mcp\helpers\manifest.json `
    -WorkspaceRoots C:\path\to\Meridian-Rift `
    -RepositoryRoots C:\path\to\Meridian-Rift `
    -StateDirectory C:\path\to\private-meridian-state `
    -Development
```

Add `-EnableTracy` to both installation and configuration only when the combined manifest contains the verified Tracy helper and hook. Restart Codex after changing its MCP configuration.

Keep the builders' `helpers/licenses` directory with the helper binaries and the debugger notices with each debugger package. The installer retains Meridian-MCP's root `LICENSE`, rejects missing or conflicting required notices before replacing files, and copies those notices into the installed package. A custom manifest location remains supported through its relative helper paths. See the [dependency policy](docs/dependency-policy.md) for component licenses and the separate requirements for binary distribution.

### Startup configuration

The server reads immutable startup configuration:

- `MERIDIAN_MCP_MODE`: `analysis` (default) or `development`.
- `MERIDIAN_MCP_ROOTS`: semicolon-separated workspace roots on Windows; platform path-list syntax elsewhere.
- `MERIDIAN_MCP_REPOSITORIES`: optional path list of explicitly authorized local Git working trees. At startup, Meridian-MCP discovers and verifies their linked worktrees using fixed local Git commands, then adds those exact canonical paths to the effective roots.
- `MERIDIAN_MCP_COMPILERS`: allowlisted DreamMaker executables.
- `MERIDIAN_MCP_STATE_DIR`: required in development mode. This existing writable private state directory must be outside every workspace root and stores local atomic build records, failed-attempt history, and runtime-integrity journals; it is never published as evidence. Multiple MCP processes may share it through operation-scoped operating-system locks.

Writer tools using the same private state directory share an execution lease for each Git worktree (or the DME/DMB parent directory outside Git). Direct and Rift builds and standard, debugger, and Tracy runtime sessions exclude each other within that scope, including requests authorized through nested workspace roots. A runtime retains the lease until its owned process tree and integrity finalization finish. Active ancestor and descendant scopes also exclude each other, preventing overlapping non-Git artifact directories from admitting concurrent writers. Separate linked worktrees can execute independently. Cooperating hosts must use the same private state directory; external writers are not coordinated.

For projects without Git worktree metadata, the working directory must be the canonical DME/DMB parent or a directory beneath it. Sibling or ancestor working directories now fail closed as ambiguous layouts. Git worktrees retain support for a separate contained working directory within the same worktree.

An interrupted host leaves durable active execution state. A later request returns `recovery_required` even when the operating-system lock is available: lock release and PID disappearance do not prove descendant cleanup. Automatic crash recovery is not implemented. Establish complete writer cleanup and resolve the recorded execution state before retrying; do not delete the marker or change state directories to bypass this boundary.
- `MERIDIAN_MCP_RIFT_BUILD`: `disabled` (default), `offline`, or `network`. The ceiling is immutable and `rift_compile` remains absent unless enabled.
- `MERIDIAN_MCP_HELPER_MANIFEST`: build-produced manifest for the exact dmdoc helper; absent or mismatched helpers keep `dm_generate_docs` unavailable.
- `MERIDIAN_MCP_DEBUGGER`: `disabled` (default) or `auxtools`. Auxtools requires development mode, one allowlisted `dm.exe`, its sibling `dreamseeker.exe`, and the fixed hash-verified DLL beside Meridian-MCP.
- `MERIDIAN_MCP_TRACY`: `disabled` (default) or `byond`. Tracy requires development mode and exact `tracy-server-helper` (host x86_64) and `byond-tracy` (x86) manifest entries for the current platform and BYOND baseline.

All roots and repositories must exist. `MERIDIAN_MCP_REPOSITORIES` also authorizes verified linked Git worktrees. `dm_server_status` reports those effective roots under `immutable_startup_roots`. Restart after changing permissions.

`rift_compile` defaults to `network_mode=offline`; `network_mode=allow` requires a startup ceiling of `network`. Offline mode configures cooperative package-manager restrictions, not an operating-system firewall.

The configuration updater expects the named server and environment tables to exist already. It updates the installed binary, manifest, roots, mode, state, debugger, and Tracy settings while preserving unrelated servers and existing keys such as the compiler allowlist and Rift build ceiling. A complete Windows development table can therefore look like this before or after the updater runs:

```toml
[mcp_servers.meridian-mcp]
command = 'C:\path\to\installed-meridian-mcp\meridian-mcp.exe'

[mcp_servers.meridian-mcp.env]
MERIDIAN_MCP_MODE = 'development'
MERIDIAN_MCP_ROOTS = 'C:\path\to\Meridian-Rift'
MERIDIAN_MCP_REPOSITORIES = 'C:\path\to\Meridian-Rift'
MERIDIAN_MCP_COMPILERS = 'C:\Program Files (x86)\BYOND\bin\dm.exe'
MERIDIAN_MCP_STATE_DIR = 'C:\path\to\private-meridian-state'
MERIDIAN_MCP_RIFT_BUILD = 'offline'
MERIDIAN_MCP_HELPER_MANIFEST = 'C:\path\to\installed-meridian-mcp\helpers\manifest.json'
MERIDIAN_MCP_DEBUGGER = 'auxtools'
MERIDIAN_MCP_TRACY = 'disabled'
```

## Operational details

The [memory investigation guide](docs/memory-debugging.md) covers saved process-memory samples and opt-in native allocation attribution. Object retaining-reference graphs remain unsupported.

A managed artifact records compiler, source and output identities. A later failed compile or changed recorded input/output makes it stale, and launch tools reject it. An unmanaged human-built DMB is marked `unverified`; set `require_verified_provenance` to reject it. Use `dm_check_fixture_sync` to check a declarative fixture before building. See [provenance](docs/provenance.md).

The private state directory stores build records and `runtime-integrity/` journals outside workspace roots. A five-second monitor records changes; status and stop also refresh the journal, including after natural process exit. Only exact owned files may be exempted. The server reports `source_integrity_warning` and never reverts workspace changes.

Native measurements before game start are labeled `pre_game_cumulative`, not presented as gameplay intervals. See [native evidence analysis](docs/native-evidence.md) for measurement and comparison rules.

AphelionDMM's adapter supports only parsing, map information and diagnostics through its [versioned compatibility declaration](tests/compatibility/aphelion-dmm.json).

## Trust, design, and policy documents

- [Provenance and inherited code](docs/provenance.md)
- [Source authority](docs/source-authority.md)
- [Compatibility and evidence](docs/compatibility.md)
- [Dependency policy](docs/dependency-policy.md)
- [Security policy](SECURITY.md)
- [Detailed security model](docs/security.md)
- [Architecture](docs/architecture.md)
- [Native evidence analysis](docs/native-evidence.md)
- [Tracy profiling](docs/tracy-profiling.md)
- [Tool contracts](docs/tool-contracts.md)
- [Testing](TESTING.md)

## License

Meridian-MCP is distributed under MIT. Dependencies retain their own licenses; see [dependency policy](docs/dependency-policy.md).
