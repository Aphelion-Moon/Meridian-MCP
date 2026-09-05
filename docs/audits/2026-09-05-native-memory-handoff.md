# Native memory debugging handoff

The memory tools and optional Auxtools allocation helper are implemented. Changes remain uncommitted. The [usage guide](../memory-debugging.md) explains evidence boundaries and the [workplan](../superpowers/plans/2026-09-05-memory-investigation.md) records qualification.

## Delivered

- `dm_memory_summary` and `dm_memory_compare` analyze saved process-memory evidence in analysis mode.
- `dm_debug_launch` accepts `memory_profile: true` to use a separately verified helper on Windows BYOND 516.1687.
- `dm_debug_memory` provides bounded status/start/stop controls and procedure-attributed allocation reports.
- `dm_debug_evaluate` rejects upstream console commands. Normal debugger sessions keep the original pinned Auxtools DLL.

## Verified locally

Rust 1.95.0: 405 tests passed, four intentionally ignored; strict Clippy, formatting and the capability audit passed. The final release passed the owned native fixture: allocation, failed and zero-size reallocation, freeing, deadline/record limits, repeated capture and opt-in boundaries. Its allocation procedure reported zero outstanding bytes after release. No owned fixture processes remained. Installer checks retained existing helper patch and telemetry metadata.

Release stdio checks passed for the 26-tool analysis and 35-tool development inventories, including fixture parsing, cached diagnostics and search. Saved process-memory evidence returned 492 samples per metric for two process roles and six compared metrics. Fresh startup of the installed release advertised all 62 configured tools, eight effective roots and the existing compiler allowlist; it exited 0.

## Resume after restart

1. Restart Codex, then call `dm_server_status`. Confirm that `mcp_build.build_id` and `mcp_build.executable_sha256` match the identities below, and that `dm_memory_summary`, `dm_memory_compare` and `dm_debug_memory` are advertised in the configured development server.
2. Reparse the task's DME with `dm_parse_environment` before source inspection; parse state is per server process.
3. For allocation capture, compile the selected fixture/workload and launch its owned debugger with `memory_profile: true`. Call `dm_debug_memory` status, start, exercise the workload, then stop. Stop the debugger before rebuilding.

The native helper excludes other threads, cross-thread frees, custom allocators/VM pools and retaining references. No complete heap or real-game leak claim is established. Hosted CI, other BYOND versions/platforms, real-game capture overhead and object-retention graphs remain separate gates. A timed-out/cancelled debugger request requires stop/relaunch; delayed replies cannot be reused for later requests. Disconnect no longer waits for an acknowledgement upstream never sends.

## Package status

Installed alongside the previous release at `%LOCALAPPDATA%/meridian-mcp/releases/memory-94c7dd9f9a5f/meridian-mcp.exe`:

- MCP SHA-256: `94c7dd9f9a5f88299bd079441ce579772a2ad8fb75e14a0c5f900286a85721a9`.
- Build ID: `ca3862be43c37402c8371ecbff7be614bbe7ced8465858396cda81b3cb23d55a`.
- Native helper SHA-256: `f1fdad756b83eb1e2b1cd0d47198640639adfd3057b564c818671e20bd004639`.
- Configuration backup: `%LOCALAPPDATA%/meridian-mcp/restart-backups/20260905-235300-memory-94c7dd9f9a5f/config.toml.before`.

Codex registration now points to this binary and its helper manifest. Other environment settings, roots, arguments, working directory, tool filters and timeouts were preserved. The previous `da29e984b7d7` release remains available. Fresh installed-process startup passed; loading this release in Codex still requires the restart above.

Raw local evidence remains under ignored `target/`: `native-memory-final-tests.log`, `native-memory-integration-final-release.json`, `memory-stdio-final-release.json`, `native-memory-installed-status.json` and `native-memory-installation.json`. The initial release delay and isolated rerun are retained separately in `native-memory-integration-release.json` and `native-memory-integration-release-repro.json`; the cause of that intermittent VM delay remains unresolved. Do not publish machine-specific raw logs.
