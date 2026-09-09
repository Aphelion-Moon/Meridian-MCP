# Fixture freshness qualification and stopping point

`dm_check_fixture_sync` now checks the parser's metadata fingerprint before reusing an active snapshot. Removing a required procedure, changing its arguments, or changing the DME includes triggers a disposable parse. An unreadable include fails validation. The caller's active analysis snapshot and generation remain unchanged, including on failure.

This closes the stale-snapshot finding in the [previous checkpoint](../2026-09-10-fixture-sync-checkpoint/README.md). That reproduction remains unchanged. The shared fingerprint checks file metadata and configuration discovery; it does not prove content identity for same-size edits with preserved timestamps or isolate concurrent edits.

## Qualification

Rust was pinned to `rustc 1.95.0 (59807616e 2026-04-14)`. The [verification record](verification.json) binds the four changed source/test/documentation files, frozen executables and raw logs to this qualification.

| Gate | Windows | Linux under WSL 2 |
| --- | ---: | ---: |
| Focused fixture and analysis snapshot tests | 20 passed | 21 passed |
| Full all-features suite | 485 passed, 5 ignored | 471 passed, 7 ignored |
| Strict all-target/all-feature Clippy | Passed | Passed |
| Owned stdio freshness sequence | Passed | Passed |

The completed suites had zero failures. Before the repair, all four new fixture regressions failed, returning `verified` for changed or incomplete inputs. The first three-test red run and an interrupted compilation-only attempt are also retained; the latter is not a completed test gate.

Both [Windows](windows-stdio.json) and [Linux](linux-stdio.json) stdio results show `verified` before the mutation, then `invalid` with `required_proc_missing` immediately after it and after forced reparse. Analysis generation advances from 1 to 2 only on the explicit forced parse. Both servers exited naturally with code 0. Windows stderr and both launcher error logs were empty; Linux stderr contained ordinary INFO tracing only. Every reply separately reports `provenance_status: unverified`: these checks do not establish compiled or runtime provenance.

The code review found no blocking issue. Two existing boundaries remain: library callers must not reuse one server state across changing path policies, and disposable fixture parses use a separate parse mutex and may overlap a main-state parse. Production startup policy is immutable.

## Stopped work and remaining evidence

The next token-cost probe stopped with `unexpected_issue_digest_absent-01-rep-03`. Its launcher exited 1 while the MCP exited normally with 0; it preserved 7 of 10 result rows. Inspection reproduced a harness defect: PowerShell unwraps the one-token assignment into a scalar, so its generated `required_tokens` is a JSON string instead of an array. The failed row's raw MCP reply was not retained, so its exact server error is unknown. These partial results are not a qualified benchmark, speedup claim or model-token measurement.

The original failed script and artifacts remain under ignored `target/fixture-token-cost-*`; their hashes are in the verification record. The prepared Latin-1 and oversized-response probe, `target/fixture-input-output-probe.ps1`, has not run. No repair to token scanning, input/work budgets, encoding or response construction is included here.

The installed MCP and registration are unchanged, so **no restart is required for the existing installation**. The repair is qualified in local candidates only. Nothing was pushed or installed. Frozen candidates and raw logs stay under ignored `target/`; subsequent audit prose and commits change build identity inputs and do not make those executables builds of the later commit.

## Resume here

1. Recheck Git status, `rustc +1.95.0 --version`, and the recorded source/candidate hashes. Preserve the completed freshness results and the failed cost attempt.
2. Before editing the cost harness, copy its executed version to a separate ignored filename and confirm SHA-256 `7c08f00cb180ebb867226fabbafd74e7435161794aa4ce32ff3248aa2ca6d6c6`. Declare `requiredTokens` as `[string[]]` or explicitly wrap the manifest value; assert the one-token JSON property is an array. Save raw MCP stdout immediately after the session returns, before result validation, and retain a failed row's response. Extend the harness's existing fresh-artifact guards for that log.
3. Run the corrected cost probe once against the frozen Windows candidate, using a fresh output prefix. Then run the prepared functional probe separately:

   ```powershell
   & ./target/fixture-token-cost-probe.ps1 -BinaryPath ./target/fixture-sync-freshness-qualified-windows/mcp.exe -OutputPrefix ./target/fixture-token-cost-baseline-v2
   & ./target/fixture-input-output-probe.ps1 -BinaryPath ./target/fixture-sync-freshness-qualified-windows/mcp.exe -OutputPrefix ./target/fixture-input-output-baseline
   ```

   Review the scripts and confirm those prefixes remain unused first. If the cost probe fails, preserve it and inspect the cause before continuing. The cost harness measures end-to-end request time and response bytes, not disk I/O, server CPU or model tokens. The functional probe's oversized case intentionally expects a transport-limit error; reproduction success is not product acceptance.
4. Use complete baseline results to choose the next bounded repair. Reproduce encoding and output-budget findings before treating them as confirmed defects. Require matched workloads and equivalent results for any performance claim.
5. After implementation changes, rerun focused tests, full suites and strict Clippy with the pinned compiler. On Windows the existing local wrapper initializes the native build environment:

   ```powershell
   & ./.superpowers/sdd/2026-09-05-mcp-audit-remediation/run-rust.ps1 -CargoArguments @('test', '--offline', '--locked', '--all-features', '--test', 'fixture_manifest', '--test', 'analysis_snapshot')
   & ./.superpowers/sdd/2026-09-05-mcp-audit-remediation/run-rust.ps1 -CargoArguments @('test', '--offline', '--locked', '--all-features', '--no-fail-fast')
   & ./.superpowers/sdd/2026-09-05-mcp-audit-remediation/run-rust.ps1 -CargoArguments @('clippy', '--offline', '--locked', '--all-targets', '--all-features', '--', '-D', 'warnings')
   ```

   Linux qualification used native PowerShell 7.6.5, `TMPDIR=/tmp`, four Cargo jobs, incremental compilation disabled and `CARGO_TARGET_DIR=target/linux`. Keep Windows and Linux resource-heavy gates sequential and record each process's terminal result.

The [broader workplan](../2026-09-06-functional-performance-followup.md) remains incomplete. Additional documentation inputs, full-project builds, other tool families, the older intermittent WSL 1 guardian failure, hosted/live-platform tests and release/installed acceptance remain separate gates. Stop here until work is resumed.
