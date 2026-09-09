# Audit stopping point

Implementation stops at commit `862db44` (`Preserve documentation inputs and final readiness evidence`). The [documentation/readiness qualification](../2026-09-10-docs-qualification/README.md) records 481 passing Windows tests and 467 passing Linux tests, strict Clippy and native pinned-dmdoc checks on both platforms. This checkpoint adds evidence and next steps only; it does not repair fixture synchronization or qualify the entire MCP.

The installed MCP and registration are unchanged. No restart is needed for the existing installation. Nothing was pushed or installed. Preserve the frozen candidates and raw logs under ignored `target/`; later documentation commits change build identity inputs and do not turn those candidates into builds of the new commit.

## Reproduced finding

`dm_check_fixture_sync` can report `classification: verified` using an outdated active parse. In one owned Windows stdio session, a required procedure was removed from a copied generated binding while its required protocol token remained. The tool noticed the changed manifest identity but still accepted the old procedure tree. A forced reparse then detected the missing procedure.

| Observation | Classification | Issue |
| --- | --- | --- |
| Original fixture after initial parse | `verified` | None |
| Required procedure removed, no reparse | `verified` (incorrect) | None |
| Same mutated fixture after forced reparse | `invalid` | `required_proc_missing` |

All three replies separately reported `provenance_status: unverified`; structural classification is not compiled or runtime verification. The [portable probe result](probe.json) retains manifest, binding and executable hashes. Its `status: passed` means the reproduction assertions passed, including the incorrect middle result; it is not a product acceptance result.

The probe used the frozen Windows candidate from the completed batch, exited naturally with code 0, and cleaned its unique fixture copy. Original repository fixtures were not edited. Analysis generation advanced from 1 to 2 only when explicitly forced. This is one functional reproduction, not a timing or token-cost benchmark.

Current source explains the result: [`matching_or_fixture_snapshot`](../../../src/tools/fixture.rs) reuses an active snapshot by DME path alone. Required procedure checks use that tree, while required token checks read live files. The parser already has a freshness helper in [`src/tools/parse.rs`](../../../src/tools/parse.rs); its fingerprint checks metadata and discovery paths, not cryptographic content identity.

Local raw artifacts, retained without overwriting:

- `target/stale-fixture-stdio-probe.ps1`: SHA-256 `8fb17cba5f63bea3dde59e3cfb7bd281b5aab7797a9d6f6c5eaef2536cee5dea`.
- `target/stale-fixture-stdio-probe.json`: SHA-256 `b9a7a9742aef12801da309483677d2b684771ca1293647104b03a13a02cb7596`.
- `target/stale-fixture-stdio-probe.stderr.log` and `target/stale-fixture-stdio-probe-launcher-error.log`: both empty.
- `target/docs-preservation-qualified-windows/mcp.exe`: SHA-256 `873100a676fbede7416b6665812dad2a6a64329fd79180d588c54e69d17d3b55`.

## Resume here

1. Recheck Git status and the pinned Rust version before editing. Add a regression in [`tests/fixture_manifest.rs`](../../../tests/fixture_manifest.rs): parse an owned fixture, remove its required procedure while retaining the token, then check synchronization without reparsing. Also exercise a changed argument signature. Assert that auxiliary validation preserves the user's active analysis snapshot/generation.
2. Observe the failing regression, then repair snapshot freshness. Consider sharing the parser's existing reuse check and parsing into a temporary state when reuse cannot be established. Decide explicitly whether the fixture contract requires stronger content identity; metadata freshness must not be described as content proof.
3. Run the focused gate with Rust 1.95.0 and an initialized native build environment:

   ```text
   cargo +1.95.0 test --offline --locked --all-features --test fixture_manifest
   ```

   In this Windows checkout, the existing local wrapper imports the VS Build Tools environment:

   ```powershell
   & ./.superpowers/sdd/2026-09-05-mcp-audit-remediation/run-rust.ps1 -CargoArguments @('test', '--offline', '--locked', '--all-features', '--test', 'fixture_manifest')
   ```

4. Repeat the owned stdio sequence against a separately frozen repaired candidate: initialize analysis mode; parse the copied fixture; check sync; remove `/proc/meridian_fixture_state_batch(payload)` from the copied `generated_bindings.dm` while retaining `#define MERIDIAN_FIXTURE_PROTOCOL 4`; check sync; force parse; check sync. The repaired second check must report the missing procedure without requiring the explicit parse. Use `scripts/MeridianMcpSession.psm1`, separate result names and a contained temporary fixture; keep this baseline immutable. Analysis mode must not inherit development-only state configuration.
5. Independently measure repeated whole-file reads per required token and intermediate issue serialization, then add appropriate input/work/reply bounds. These are source observations, not yet measured performance regressions or repaired limits. Run relevant Windows/Linux focused and full gates after implementation changes and record candidate identity separately from installed state.

The [broader workplan](../2026-09-06-functional-performance-followup.md) remains incomplete. Additional live dmdoc Markdown/index inputs and stale sources, full-project documentation, the older intermittent WSL 1 guardian failure, remaining tool families, hosted/live platform gates and release/installed acceptance remain open. No new repair or test run was started to close this checkpoint.
