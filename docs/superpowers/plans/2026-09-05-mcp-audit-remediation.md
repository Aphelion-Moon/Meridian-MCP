# Meridian-MCP Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Implementation is not authorized by the audit request; do not execute this plan until asked.

**Goal:** Correct the ten audit findings while preserving exact lookup, immutable startup policy, artifact evidence, and the existing tool surface.

**Architecture:** Enforce policy at the actual resource and process boundaries. Keep immutable analysis snapshots, but make their dependency identity complete and their workers cancellation-safe. Reuse existing process containment, private-state records, and bounded output rather than adding parallel ownership systems.

**Tech Stack:** Rust 1.95.0, Tokio, Rayon, rmcp 3.1.3, pinned SpacemanDMM `351ddc0ffb2439876d4565ce5130bb6b027ee605`, PowerShell 7, Windows BYOND, native Tracy helpers.

**Spec:** [Performance, integration, and functionality audit](../../audits/2026-09-05-performance-integration-functionality.md).

## Global constraints

- Baseline audited revision: `48d6d54160959eafcc4992e20e56cc586437709f`. Recheck HEAD and dirty work before implementation.
- Work in the existing checkout; preserve unrelated changes. Do not create a worktree, commit, push, install, or replace a running MCP without explicit authorization.
- Read current repository guidance and `TESTING.md` before editing. If a task reaches Meridian-Rift DM source, read its current style, standards, autodoc, and modularization guides first.
- Keep Rust at 1.95.0. A dependency change needed for transitive I/O policy must be narrow, pinned, and independently qualified; do not combine it with a general upgrade.
- Keep analysis mode free of compilation/runtime effects. Startup roots and capability ceilings remain immutable.
- Preserve failed-parse state, retained build artifacts, failed-attempt provenance, and explicit unverified results.
- Accept LF and CRLF in external inputs while preserving the repository's LF policy.
- Use only disposable fixtures for mutation and fault injection. Do not run destructive full-build compatibility against the primary Meridian-Rift checkout.
- Report Rust, installed stdio, native helper, full-corpus analysis, real BYOND runtime, hosted CI, and human acceptance separately.
- Keep durable reports free of account names and machine-specific absolute paths. Raw local logs remain ignored.

## Order and ownership boundaries

| Order | Task | Findings | Dependency | Deliverable |
| --- | --- | --- | --- | --- |
| 1 | Enforce final compiler selection | F2, P1 | None | Every selected compiler is allowlisted before spawn |
| 2 | Enforce transitive read policy | F1, P1 | None | Parser and dependent reads cannot escape startup roots |
| 3 | Repair snapshot dependency identity | F5, P2 | Task 2 | Authorized external includes and config discovery invalidate reuse |
| 4 | Establish trustworthy compile provenance | F3, P1 | Tasks 1–3 | Verified records describe the effective, stable build inputs |
| 5 | Own standard runtime process trees | F4, P1 | None | Stop, EOF, and owner failure clean up owned processes |
| 6 | Keep runtime controls responsive | F7, P2 | Task 5 | Output waits do not monopolize lifecycle control |
| 7 | Bound parse admission and cancellation | F6, P2 | Task 3 | End-to-end request deadline and one active parser worker |
| 8 | Bound and reuse decoded assets | F8/F9, P2 | Task 2 | Pre-allocation limits, bounded residency, effective cache hits |
| 9 | Bound Tracy transport and shutdown | F10, P2 | None | Backpressure cannot defeat control deadlines or forced cleanup |
| 10 | Qualify and prepare deployment handoff | All | Tasks 1–9 | Reviewed evidence matrix and explicit remaining gates |

Each task is independently reviewable. The order favors policy and artifact correctness over general speed tuning. Do not begin an unmeasured search-ranking rewrite or dense-retrieval implementation as part of this work.

## Task 1: Enforce final compiler selection

**Files:** modify `src/tools/compile.rs` and, if centralizing validation, `src/tools/mod.rs`; extend `tests/compiler_runner.rs` and `tests/config_and_paths.rs`.

**Interface:** the final `PathBuf` passed into `ProcessSpec.program` must be the canonical result of `PathPolicy::executable`, regardless of whether the client supplied it. Preserve `executable_not_allowed` for an explicitly denied executable.

- [x] Add a regression for an empty allowlist and omitted `compiler_path`; assert a policy/configuration error and zero process-start evidence.
- [x] Add cases for a sole allowlisted compiler, a different conventional installation, and several allowlisted compilers. Use the existing test-executable fixture pattern in `tests/compiler_runner.rs` so the negative gate does not require BYOND.
- [x] Reproduce the existing defect with the caller payload below, then resolve default selection from immutable configuration and validate it before argument construction/spawn.

```json
{"name":"dm_compile","arguments":{"dme_path":"<owned fixture DME>","timeout_ms":1}}
```

- [x] Run `cargo +1.95.0 test --locked --test compiler_runner --test config_and_paths`. Verify that explicitly selecting and implicitly selecting the same denied compiler produce the same policy outcome.

**Done:** no branch reaches spawn with an unvalidated compiler; no new general executable fallback is introduced.

## Task 2: Enforce transitive read policy

**Files:** modify `src/tools/parse.rs`, `src/tools/mod.rs`, `src/path_policy.rs`, and affected source/icon adapters; extend `tests/config_and_paths.rs` and `tests/analysis_snapshot.rs`. If pinned SpacemanDMM has no supported loader hook, a narrow reviewed parser dependency patch also touches `Cargo.toml`, `Cargo.lock`, its revision registries/build scripts, and matching CI pins.

**Interface:** carry the immutable `PathPolicy` from `ToolExecutionContext` into every analysis operation that can open derived paths. Canonicalize each path against all effective roots; do not substitute the DME parent for startup policy.

- [x] Create a temporary parent containing `allowed/fixture.dme` and sibling `external.dm`. Give the server only `allowed` as a root. Use this fixture content:

```dm
// allowed/fixture.dme
#include "../external.dm"
```

```dm
// external.dm
/datum/audit_external
	proc/read_marker()
		return "AUDIT_OUTSIDE_ROOT"
```

- [x] Verify the current escape, then add rejection tests that preserve the prior generation and never expose the marker. Test both direct includes and conditional nested includes. Add platform-supported symlink/junction cases.
- [x] Inspect the exact pinned preprocessor's file-open path. Enforce containment there through a supported loader hook or a narrowly pinned upstream patch. A text-only include pre-scan or filtering the final index is not an acceptable security boundary.
- [x] Add a positive case where both the DME root and sibling root are explicitly authorized. Exercise exact proc inspection and search as well as parsing.
- [x] Audit icon references and map rendering resource opens for the same transitive policy path; add a contained resource and an escaping resource case before changing those adapters.
- [x] Run `cargo +1.95.0 test --locked --test config_and_paths --test analysis_snapshot --test map_capabilities --test dmi_analysis` and the analysis stdio smoke.

**Done:** policy covers actual derived reads, and valid multi-root projects remain supported. Record any upstream dependency work explicitly rather than weakening root enforcement to avoid it.

## Task 3: Repair snapshot dependency identity

**Files:** modify `src/analysis_snapshot.rs`, `src/source_fingerprint.rs`, `src/project.rs`, `src/tools/parse.rs`; extend `tests/analysis_snapshot.rs`, `tests/project_profile.rs`, and parse unit tests.

**Interface:** `AnalysisSnapshot::source_inputs()` describes all authorized parser inputs. Configuration discovery contributes an identity even when the expected file is absent. Continue reporting `reused` and stable `state_generation` for unchanged inputs.

- [x] Reuse Task 2's authorized multi-root fixture. Parse `var/value = 1`, change only the external DM to `var/value = 2`, then request normal reparse. Require `reused: false`, a higher generation, and `Float(2.0)` from `dm_get_var`.
- [x] Add a configuration-absent fixture; create `SpacemanDMM.toml`, reparse, then change and delete it in separate cases. Require each discovery transition to invalidate reuse. Use a real diagnostic override fixture to check the resulting diagnostic behavior, not just the generation counter.
- [x] Remove the DME-parent-only input filter after applying Task 2's policy. Preserve missing/failed-input evidence so disappearing dependencies fail closed instead of silently dropping from the fingerprint.
- [x] Align source excerpts with snapshot identity: either retain the snapshot excerpt or explicitly reject/label an on-disk excerpt whose source differs. Do not silently combine current text and stale semantic locations.
- [x] Run `cargo +1.95.0 test --locked --test analysis_snapshot --test project_profile --lib source_fingerprint` and the parse unit tests with `cargo +1.95.0 test --locked --lib tools::parse::tests`.

**Done:** the two F5 reproductions no longer reuse stale state, and an unchanged fixture still reuses the same generation.

## Task 4: Establish trustworthy compile provenance

**Files:** modify `src/tools/compile.rs`, `src/tools/rift.rs`, `src/build_provenance.rs`, and associated parameter/record documentation; extend `tests/build_provenance.rs`, `tests/compiler_runner.rs`, and `tests/rift_compile.rs`.

**Interface:** `BuildRecord` represents the effective build arguments and a complete pre-spawn source identity. Post-build comparison decides whether verified promotion is permitted. The launch gate continues to reject managed stale artifacts and expose unmanaged artifacts as unverified.

- [x] Add a controlled compiler fixture that writes a fresh DMB and exposes a synchronization point while compiling; reuse existing process-fixture conventions. This avoids making a race test depend on DreamMaker speed.
- [x] Add three red cases: a DME gains an include after the active parse; a command-line define selects a different include; and a source changes after the compiler starts but before it exits. Require unverified/stale classification unless the effective closure and consumed bytes are proved.
- [x] Capture build inputs before spawn using the effective compiler configuration. Compare input identities after completion. If the parser cannot represent a compiler-only branch, return a precise unverified reason rather than promoting the record.
- [x] Include effective defines/arguments in versioned build identity. Define compatibility behavior for older private records explicitly; preserve them and classify unsupported verification data safely.
- [x] Apply the same rule to `record_rift_provenance`. Preserve the authoritative Rift controller result and validate its artifacts; do not infer full-build freshness solely from an active analysis snapshot.
- [x] Run `cargo +1.95.0 test --locked --test build_provenance --test compiler_runner --test rift_compile --test runtime_integrity`. Run the owned provenance integration after the local BYOND fixture blocker has been resolved.

**Done:** edits to any file actually consumed by the selected build invalidate launch verification; a passing compiler exit alone cannot create false verified provenance.

## Task 5: Own standard runtime process trees

**Files:** modify `src/tools/runtime.rs`, `src/state.rs`, `src/mcp.rs`, and reuse `src/process.rs`; extend `tests/runtime_tools.rs`, `tests/runtime_integrity.rs`, and `tests/process_runner.rs`.

**Interface:** standard runtime state owns both the child and its containment lifetime. Shutdown terminates the owned tree before integrity finalization. Drop/owner-loss cleanup must not depend on a surviving async executor.

- [x] Add a fake runtime child that starts a descendant and emits a readiness marker. Record process identity rather than only a PID.
- [x] Test explicit stop, stdin EOF, abrupt MCP exit, and cancellation during startup. Confirm both child and descendant are gone within the bounded cleanup window and an unrelated sentinel remains alive.
- [x] Reuse `ProcessContainment` and the kill-on-drop patterns already used by compiler/debugger paths. Retain containment for the runtime lifetime instead of letting a local guard drop after launch.
- [x] Route clean transport shutdown through a bounded lifecycle finalizer. Keep a process-ownership fallback for forced termination and separately validate journal recovery.
- [x] Run `cargo +1.95.0 test --locked --test process_runner --test runtime_tools --test runtime_integrity` on Windows and Linux. Owned real DreamDaemon qualification follows in Task 10; this checkbox records the native process and Rust fixture gates.

**Done:** runtime ownership ends with the server or an explicit stop; cleanup never targets an arbitrary process.

## Task 6: Keep runtime controls responsive

**Files:** modify `src/tools/runtime.rs`, `src/state.rs`, and status accessors as needed; extend `tests/process_readiness.rs` and `tests/runtime_tools.rs`.

**Interface:** output waits observe a session-specific output log and notifications without retaining the runtime mutex. Every result is tied to the original session so a subsequent launch cannot satisfy an old wait.

- [x] Start the fake runtime from Task 5 and submit a missing-marker wait with `timeout_ms: 300000`. Concurrently issue status and stop.
- [x] Require each control request to complete within one second in the deterministic fixture; require the wait to terminate with the original process/session outcome.
- [x] Clone bounded observation state under the mutex, then release it before sleeping or waiting for output. Compile a requested regex once per wait, outside the polling loop.
- [x] Add a stop-then-relaunch case so old output cannot be mistaken for new readiness. Cover launch-readiness cancellation as well as explicit `dm_wait_for_output`.
- [x] Run `cargo +1.95.0 test --locked --test process_readiness --test runtime_tools --test server_status` and the runtime module unit tests.

**Done:** long observations no longer delay inspection or termination, while lifecycle transitions remain serialized.

## Task 5b: Restore Unix ownership after the lifecycle changes

Execution exposed that the former Unix containment adapter was a no-op. A temporary refusal prevents unowned launches while this follow-through restores standard and Tracy runtime capability. Implement after Task 6 so it uses the final session observation interfaces.

- [x] Add a small sibling guardian that owns a process group and watches a CLOEXEC owner-lifetime pipe. Keep the actual DreamDaemon child, PID, output, and exit status.
- [x] Keep the lifetime writer inherited until the target joins the guardian group before exec, closing the owner-loss launch race. Internal guardian mode runs before MCP configuration and Tokio startup.
- [x] Qualify explicit stop, EOF, owner termination, startup cancellation, natural exit, executor loss, setup failure, and descriptor inheritance in native Ubuntu fixtures. Verify owned descendants stop and an unrelated sentinel survives.
- [x] Confirm tree termination before integrity finalization on both platforms. A termination request alone is insufficient; retain ownership and leave the journal retryable on cleanup failure or timeout.
- [x] Preserve generic compiler/collector containment and Windows lifecycle behavior. State the cooperative Unix process-group boundary; deliberate group escape and independent guardian-plus-owner failure are separate guarantees.

**Done:** Unix runtime launch works again with owner-loss cleanup, and both platforms finalize integrity only after confirmed owned-tree termination.

## Task 7: Bound parse admission and cancellation

**Files:** modify `src/tools/parse.rs`, `src/state.rs`, and snapshot validation scheduling; extend parse unit tests and `tests/analysis_snapshot.rs`.

**Interface:** `timeout_ms` is a total request budget. A non-abortable worker retains exclusive parser admission until it exits, even if its caller is dropped. Failed or cancelled work does not install a new snapshot.

- [x] Add a deterministic worker barrier. Hold the first worker open and submit a second request with a short timeout. Require it to time out in the queue without starting a second worker.
- [x] Drop the first caller's future while the worker barrier remains closed. Submit another parse and assert the active-worker count never exceeds one. Release the barrier explicitly for test cleanup.
- [x] Place permit ownership in worker supervision before spawning. Use one deadline for permit acquisition, reuse validation, and parsing. Avoid an explicit-timeout-only cleanup path.
- [x] Run synchronous fingerprint validation on bounded blocking work. Preserve the cheap unchanged-snapshot route and timing fields; document whether individual stage timings include scheduling.
- [x] Run `cargo +1.95.0 test --locked --lib tools::parse::tests -- --nocapture` and `cargo +1.95.0 test --locked --test analysis_snapshot`.

**Done:** F6's queued 1 ms request cannot wait for an unrelated full parse, and cancellation cannot cause two parser workers to coexist.

## Task 8: Bound and reuse decoded assets

**Files:** modify `src/spaceman/dmi.rs`, `src/tools/dmi.rs`, `src/state.rs`, and `src/limits.rs`; extend `tests/dmi_analysis.rs` and `tests/map_capabilities.rs` if shared loading changes.

**Interface:** unchanged content can return an existing `DecodedDmi` before pixel decode. File reads, decoder allocation, blocking jobs, and total scan residency each have enforced limits.

- [x] Add injected small-limit tests: file exceeds byte ceiling, image dimensions exceed pixel ceiling, several individually valid images exceed the scan decoded-byte ceiling, and concurrent loads exceed the configured job budget.
- [x] Add a decode counter around the decoder boundary. Repeated loads of unchanged bytes must decode once; changed bytes with preserved metadata must invalidate. Multiple references to one DMI during `audit_icons` must share one decoded asset.
- [x] Bound input reads to the ceiling plus one detection byte. Inspect dimensions and configure decoder limits before allocating full pixels.
- [x] Separate content hashing/cache lookup from decode. Coalesce concurrent same-identity loads; preserve asset generation semantics when pixels change.
- [x] Replace unbounded scan retention with bounded batches or a decoded-byte budget that counts live `Arc` references outside the cache. Apply the existing four-job setting through a shared admission mechanism.
- [x] Run `cargo +1.95.0 test --locked --test dmi_analysis` and relevant DMI unit tests. Record decoder counts, peak live decoded bytes, cold/warm elapsed time, and output equivalence on a representative icon fixture.

**Done:** cache hits avoid decode, and exceeding a declared limit is detected before the excessive allocation/work occurs.

## Task 9: Bound Tracy transport and shutdown

**Files:** modify `src/tracy_collector.rs`; extend `tests/tracy_protocol.rs` and `tests/tracy_tools.rs`.

**Interface:** `request_with_timeout` applies one deadline to writer acquisition, frame write, and response. Collector stop has an independent termination deadline and does not require a successful `session_stop` exchange.

- [x] Use `tokio::io::duplex(1)` with a peer that never reads. Send a valid request larger than the buffer and require a timeout within the configured total budget.
- [x] Add blocked-writer contention, caller cancellation, late response, and unterminated oversized-frame cases. Assert no pending-request leak and bounded frame storage.
- [x] Compute one deadline at entry and cover every awaited I/O stage. Ensure pending IDs are removed on write failure, timeout, cancellation, and transport failure.
- [x] Make forced child cleanup race against the absolute stop deadline even if the protocol writer remains blocked. Retain identity and process containment checks.
- [x] Run `cargo +1.95.0 test --locked --test tracy_protocol --test tracy_tools`. Follow with the pinned native and live Tracy fixture gates; portable transport tests alone do not qualify a real capture.

**Done:** blocked stdin cannot indefinitely prevent status, cancel, or stop, and oversized responses cannot grow memory past the framing limit.

## Task 10: Qualify and prepare deployment handoff

**Files:** update `TESTING.md`, `docs/architecture.md`, `docs/provenance.md`, `docs/tracy-profiling.md`, and generated `docs/tool-contracts.md` only where behavior changed. Add a dated verification report under `docs/audits/`. Update `tests/parse_reuse_scale.rs` only to separate optional corpus-specific relevance assertions from portable timing checks.

- [x] Run the exact local gates in a correctly initialized native toolchain environment:

```powershell
rustc +1.95.0 --version --verbose
cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.95.0 test --locked --all-features
cargo +1.95.0 build --locked --release
cargo +1.95.0 deny check
```

- [x] If contracts changed, regenerate with `cargo +1.95.0 run --locked --bin render_tool_docs`, then run `cargo +1.95.0 test --locked --test tool_contracts --test documentation`.
- [x] Run both maintained installed-binary checks against the fresh binary:

```powershell
./test_mcp.ps1 -SkipBuild -BinaryPath ./target/release/meridian-mcp.exe -Mode development
./test_mcp.ps1 -SkipBuild -BinaryPath ./target/release/meridian-mcp.exe -Mode analysis -DmePath ./tests/fixtures/language/fixture.dme -SearchQuery 'return supplied value'
```

- [x] Repeat the labeled `search_relevance` gate. For the selected real corpus, record commit and dirty state, cold/warm stage timings, request percentiles, and peak memory in at least five sequential runs without concurrent compilation. Keep the ten-query table, but do not assert Dogmos presence on a checkout without Dogmos.
- [x] Resolve the owned BYOND fixture's no-output startup blocker and run compiler/runtime/Topic/cleanup/provenance gates from `TESTING.md`. Do not substitute a full-game boot for the owned protocol tests.
- [x] Run pinned Tracy native tests and the owned live fixture. Record dropped events, control responsiveness, memory, artifact validity, journal state, and cleanup.
- [ ] Run a separately authorized real Meridian-Rift profiling session. This was not requested during local remediation.
- [ ] Run the destructive full-build compatibility script only in a disposable checkout explicitly authorized for that gate. Report hosted Windows/Linux CI separately from local checks.
- [x] Verify `git diff --check`, inspect the complete diff, scrub durable reports, and confirm only authorized changes remain. Leave everything uncommitted.
- [x] Install the user-authorized pushed release, verify its binary identity and standalone startup, and update Codex registration with a configuration backup. See the [restart handoff](../../audits/2026-09-05-codex-restart-handoff.md).
- [x] After Codex is fully quit and reopened, verify status, parse, repeated reuse, cached diagnostics, and search through the newly exposed app tools. Passed on 2026-09-05; see the restart handoff.

**Final acceptance matrix:** every finding has a passing regression, every affected capability has a named integration result, and every unrun or blocked gate is explicit. Do not declare deployed, live-qualified, or performance-improved based only on a local Rust green.

## Current handoff state

Tasks 1–9 and the added Task 5b are implemented and independently reviewed. Authorized local Task 10 qualification is complete: Windows 388 / Linux 381 Rust tests passed, strict lint/format/dependency gates passed, both release stdio modes passed, and Windows owned runtime, provenance, debugger and full-duration Tracy fixtures passed. Five sequential real-corpus measurements and their limits are recorded in the [qualification report](../../audits/2026-09-05-remediation-verification.md). All final Windows runtime checks were repeated after the scale gate relinked the executable; the report identifies the exact final bytes.

The user committed and pushed remediation as `da29e984b7d7cf1c31cac9f64b1d1d93c13c0ff9`; all six hosted CI jobs passed. The clean release is installed and selected in Codex. Installed standalone status, parse/reuse, cached diagnostics, canonical search and documentation generation passed; exact identities and preserved settings are recorded in the [restart handoff](../../audits/2026-09-05-codex-restart-handoff.md). Deployment documentation updates remain uncommitted.

Post-restart app-tool acceptance passed for the installed build: status identity, fresh parse, repeated reuse, cached diagnostics and exact search. Linux live x86 execution is unavailable on the current WSL1 host. Destructive full-build compatibility and real-game profiling remain separate unrun gates.

## Follow-up identified during qualification: debugger stop latency

The owned headless auxtools fixture passes all six requests, but `dm_debug_stop` takes about 30 seconds. This separate debugger protocol was not changed by the Tracy transport work.

- [x] Confirm the pinned auxtools disconnect response contract and add a nonresponsive-peer regression covering blocked send and absent acknowledgement. Upstream's reader exits without acknowledging `Disconnect`; `disconnect_does_not_wait_for_an_ack_the_upstream_never_sends` and `disconnect_is_bounded_when_the_peer_stops_reading` in `src/spaceman/debugger.rs` cover both cases.
- [x] Give debugger stop one bounded deadline independent of protocol completion; retain actual process ownership through confirmed cleanup and preserve honest failure outcomes. `DebuggerSession::stop_with_pool` bounds the disconnect send (1 s), process exit (2 s) and contained cleanup (2 s), and reports `MutationOutcome` with `recovery_required` on failure.
- [ ] Repeat the owned debugger fixture and record stop latency, peer-failure behavior, cancellation and process cleanup. Establish the shutdown latency target before implementation.

The bounded stop is applied, with a worst case of about 5 s against the 30,027 ms observed before. A live fixture measurement has not been recorded yet. The qualification report retains the original observation and the relevant source path.
