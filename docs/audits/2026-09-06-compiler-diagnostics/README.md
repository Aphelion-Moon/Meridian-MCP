# Streaming compiler diagnostic audit

Based on `ddebb0c`. This fixes compiler failure detection and provenance when errors fall outside the retained output tail. The previous [response-budget repair](../2026-09-06-compiler-responses/README.md) remains a separate historical baseline.

## Reproduced defect

`dm_compile` parsed diagnostics only after the process runner had discarded all but the final 512 KiB of each stream. A zero-exit compiler emitting an early error followed by ordinary progress could therefore return `success: true`. With stable parsed inputs and a newly written artifact, it could also create verified provenance.

The initial regression run reproduced that false success and counted only **11,915 of 30,000 errors** in a larger fixture. The actual stdio comparison reproduced **nine early-error builds marked successful and verified** by the frozen baseline. This is a build-classification defect, independent of whether raw logs are returned to the caller.

## Repair

The process runner now offers a bounded synchronous output observer. Compiler analysis receives every chunk before tail eviction, including the final drain. Separate per-stream line buffers preserve split UTF-8, CRLF, final unterminated lines and the established stdout-then-stderr diagnostic ordering. Diagnostic fields borrow the parsed line; JSON rows are built only within the requested retention limit and byte budget.

Counts continue after row retention stops. Each stream retains at most the requested 0–200 rows and 96 KiB of serialized rows per severity, plus a partial line of at most 1 MiB. These are retention bounds, not a measured process-memory claim. The existing reply formatter applies the combined per-severity and total response budgets.

`diagnostic_summary` now distinguishes:

- `scope: "observed_output"`: counts are computed before log eviction.
- `analysis_complete`: all observed lines were analyzed and both readers reached EOF.
- `capture_complete`: raw output tails retained every observed byte; this can be false while diagnostic analysis is complete.
- `output_complete` and `oversized_lines`: distinguish incomplete pipe reads/drains from lines exceeding the 1 MiB analysis limit.

Incomplete analysis returns `success: false` and `diagnostic_analysis_error`, and records `diagnostic_analysis_incomplete` when an otherwise successful process cannot establish a build result. A normal compiler error records `compiler_failed`. A failure with no earlier managed build remains unverified; a failure after a verified build makes the previous record stale. The strengthened provenance regression exercises both builds and checks the retained record ID and failed attempt.

The generic process runner also reports read completeness, preserves already-received queued chunks after a drain deadline, and avoids aborting readers that have reached EOF while their task completion is being published. Compiler launch, timeout limits, artifact hashing and source verification rules are unchanged. See the [public output contract](../../compiler-output.md).

## Measurements

The [probe](probe.ps1) runs four owned output scenarios with diagnostic limits 0, 2 and 200, always omitting raw output. Three alternating baseline/candidate rounds make **72 compile calls**, each preceded by a fresh parse, in six naturally exiting MCP sessions. Both arms use the same fixture binary and fresh workspaces. The baseline contains `ddebb0c`'s response repair and embeds revision `485f716` with a dirty flag; the candidate embeds `ddebb0c` with a dirty flag. Exact identities are in [comparison.json](comparison.json).

| Scenario | Baseline | Candidate |
| --- | --- | --- |
| Quiet valid build | Success; verified | Success; verified |
| Early error followed by progress beyond both tails | Success; verified; zero errors | Failure; one error and one warning; failed attempt |
| 30,000 errors and 20,000 warnings | Partial counts; first returned error at line 18,086 | Exact counts; first returned error at line 1 |
| One unterminated line larger than 1 MiB | Success; verified | Analysis incomplete; failed attempt |

All 36 candidate calls satisfy the expected outcomes, counts, completeness and artifact identities. Each baseline session records nine successful attempts and three failures; each candidate session records three successful attempts and nine failures. All 72 fixture artifacts have the same hash. The native BYOND checks below are separate from this fake compiler.

With two rows per severity, median response text sizes are 2,194 → 2,295 bytes for quiet builds, 2,278 → 2,787 for early errors, 2,951 → 3,024 for the 50,000-diagnostic case, and 2,281 → 2,734 for the oversized line. The extra bytes describe outcomes and diagnostics the baseline missed. No request options or discovery fields were added in this batch.

The candidate retains no JSON diagnostic rows when the requested limit is zero. For the 50,000-diagnostic case, recorded median request times were 1,694 → 967 ms at limit zero and 1,887 → 896 ms at limit two. These debug-build observations include concurrent Windows qualification work and only three rounds, so they do not establish a compiler speedup. `duration_ms` also covers different work: most diagnostic parsing now happens inside the process runner. [Metrics](metrics.json) preserve both timings and exact response/wire sizes. Model tokens, thinking cost and process-memory savings were not measured.

## Qualification and remaining work

- Rust **1.95.0** final full suites: **458 Windows tests passed** (zero failed, five ignored) and **451 Linux/WSL 1 tests passed** (zero failed, six ignored). Strict all-target/all-feature Clippy passed on both.
- Eight new regressions cover eviction, complete totals with bounded rows, oversized analysis, previous-build invalidation, byte-split parsing, incomplete reads and drain deadlines. The existing process-output test also verifies observers receive every retained and evicted byte.
- The first full run failed the new provenance assertion because its fixture had no earlier verified record. The strengthened fixture now creates that record before provoking failure and passes. Linux additionally reproduced the previously unresolved `guardian readiness timed out` failure. The final Linux run, without concurrent Windows builds, passed; this does **not** prove the timeout's cause or fix it. Both failed logs remain bound in [verification.json](verification.json).
- [Native BYOND 516.1687](native.json) produced fresh verified DMBs in three valid path cases and rejected malformed defines before compilation. A [deliberately invalid native source](native-error.json) returned one error at line 2, complete analysis, no DMB and a compiler exit code of -1. Both MCP sessions exited naturally. The probes used the normal Windows user context because the earlier audit established a sandbox-specific BYOND hang.
- The hash-verified dmdoc execution fixture passed with the frozen candidate, covering generated HTML, custom index, source preservation, overwrite rejection and natural MCP exit.

The [verification record](verification.json) binds source, scripts and logs by SHA-256. Raw results and frozen binaries remain under ignored `target/`. WSL 1 is not hosted native Ubuntu CI; these native compiler checks are not a full Meridian-Rift build or live debugger/Tracy qualification. The installed MCP remains unchanged.

The [62-tool workplan](../2026-09-06-functional-performance-followup.md#remaining-inventory-review) remains active. Next priorities are Rift build tail classification/response volume, documentation-install failure cleanup, the remaining artifact paths, and the unresolved guardian timeout.

## Reproduction

From the repository root, use a Visual Studio developer shell on Windows and the pinned toolchain. Supply the frozen baseline and a freshly built candidate through environment variables.

```powershell
cargo +1.95.0 test --locked --all-features --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
rustc +1.95.0 --edition=2021 tests/fixtures/compiler_output.rs -o target/compiler-diagnostic-fixture.exe
./docs/audits/2026-09-06-compiler-diagnostics/probe.ps1 -BaselineBinaryPath $env:BASELINE_MCP -CandidateBinaryPath $env:CANDIDATE_MCP -CompilerPath ./target/compiler-diagnostic-fixture.exe -OutputDirectory ./target/compiler-diagnostic-reproduction
./docs/audits/2026-09-06-compiler-diagnostics/native-error-probe.ps1 -BinaryPath $env:CANDIDATE_MCP -DreamMakerPath $env:BYOND_COMPILER -OutputDirectory ./target/compiler-native-error-reproduction
```

[record.py](record.py) validates this historical run from its original `target/compiler-diagnostics-*` artifacts. It rejects a different baseline, missing gates or incorrect outcomes rather than silently relabeling a new experiment.
