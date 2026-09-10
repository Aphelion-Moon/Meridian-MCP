# Fixture token scanning and encoding audit

Fixture synchronization now accepts Latin-1 source that SpacemanDMM can parse and avoids loading the same text file separately for every required token. The controlled Windows comparison preserved all 10 complete fixture-sync replies. Timing improved for some workloads but remained variable; this is not a general speedup or model-token saving claim.

This continues the [fixture freshness checkpoint](../2026-09-10-fixture-freshness/README.md). The manifest loader, procedure checking and metadata freshness rules are unchanged.

## Findings and repair

The baseline uses `read_to_string` inside a loop over required tokens. An owned fixture with a raw `0xE9` byte in a comment parsed with zero errors, but synchronization failed with `stream did not contain valid UTF-8`. The new integration regression reproduced that failure before the repair.

The scanner now normalizes and deduplicates required tokens once, loads each needed text input once, and tests only unmatched tokens. It stops when all tokens are found. Missing issues retain the original manifest order, spelling and duplicates. Overlaps and CRLF/LF aliases remain supported, files are never concatenated, and native modules/service executables are excluded from text matching. Read failures still abort validation.

Valid UTF-8 text follows the existing CRLF-to-LF normalization. Otherwise each physical line uses SpacemanDMM's UTF-8/Latin-1 fallback, preserving valid Unicode lines beside Latin-1 lines. Lone and terminal CR characters remain text; leading BOM text is retained for token matching. This is physical-line text decoding, not the DreamMaker lexer's per-token encoding semantics. Tests cover UTF-8, Latin-1, mixed-encoding lines, overlapping/multiline tokens, BOM, CRLF, lone CR, role exclusion, duplicate missing issues and an actual cross-file boundary.

## Controlled comparison

The same corrected script generated ten immutable tiny fixtures per run. Each fixture declares 32 configuration files of 256 KiB each (8 MiB total), alongside the small source/binding pair. Negative cases request 1, 16 or 64 absent tokens, with three repetitions per count. A positive case exercises overlapping and multiline CRLF tokens. Each case is explicitly parsed before synchronization.

| Absent tokens | Baseline median | Candidate median | Change |
| ---: | ---: | ---: | ---: |
| 1 | 587 ms | 592 ms | +0.85% |
| 16 | 1,651 ms | 1,231 ms | -25.44% |
| 64 | 4,967 ms | 4,260 ms | -14.23% |

The candidate's 64-token samples ranged from 3,082 to 5,678 ms, overlapping the baseline range of 4,793 to 5,025 ms. The single positive case took 548 ms before and 554 ms after. These sequential Windows **test/debug** measurements include manifest hashing, validation, transport and harness JSON decoding. There are no disk-I/O counters, isolated CPU measurements, process-memory measurements, release-build comparisons or model-token measurements.

[Baseline](baseline-cost.json), [candidate](candidate-cost.json) and [comparison](comparison.json) records bind executable/build identities, manifest hashes, complete issue digests, timings and response bytes. All ten full sync payloads match after replacing only the unique owned fixture root; response byte counts are unchanged. Both servers and launchers exited naturally with code 0.

## Preserved failures and qualification

The first cost attempt failed because PowerShell serialized a one-item token list as a scalar. Its original script and partial result remain unchanged under ignored `target/fixture-token-cost-*` and in the prior checkpoint's artifact inventory. The corrected `target/fixture-token-cost-probe-v2.ps1` asserts JSON array shape and saves raw MCP stdout before validating replies. The successful baseline and candidate each contain all ten rows with matching expected issue digests.

The first combined focused gate also exposed a test-directory collision. Its process-ID/counter name reused a directory created five days earlier, and leftover configuration contaminated the test baseline. The [collision evidence](directory-collision.json) preserves timestamps and the failed-log hash. The test helper now uses exclusive directory creation and skips existing names, preserving old fixtures. This was a test-isolation repair; the parser implementation did not change. Subsequent focused gates passed all 22 Windows tests. A final test refinement checks a token that really would span two adjacent files if concatenated.

Qualification used pinned Rust 1.95.0, with all commands exiting 0:

| Gate | Windows | Linux (WSL2) |
| --- | --- | --- |
| Focused tests | 22 passed | 23 passed |
| Full suite | 487 passed, 5 ignored | 473 passed, 7 ignored |
| Strict Clippy, all targets/features | Passed | Passed |

Both suites recorded zero failures. Full qualification and raw artifact identities are recorded in [verification.json](verification.json). Source review found no blocking scanner issue. The raw functional-probe JSON includes host paths in successful reply strings because its audit sanitizer missed extended Windows paths; it remains ignored and unchanged. The [portable functional summary](functional-results.json) records the relevant outcomes and raw hashes without copying those paths.

## Remaining work

The functional probe separately reproduced an output failure: a valid 2,055,013-byte manifest produces a 2,054,841-byte reply, exceeding the 1,048,576-byte transport cap. The server returns `limit_exceeded` and drops the fixture classification. The scanner repair does not change that result. The next repair must evaluate every requirement while retaining bounded issue/argument details, preserve classification from the complete issue count, and report omissions explicitly. That repair has not started.

When resumed, add failing tests for huge argument lists, many missing issues, JSON-escaped paths/tokens, and summary-only output. A proposed `issue_limit` (default 50, range 0-200) needs runtime validation as well as schema documentation. Budget serialized JSON bytes before cloning retained argument details; keep complete small arrays and mark any omitted arguments. Bound provenance reasons and the combined reply too, preserving classification, identities and counts. Invalid manifests remain invalid with incomplete validation and bounded error details. These are next-step recommendations, not implemented controls.

Input hashing and scanning still run synchronously and have no total byte/work limit. Raw bytes coexist with decoded text, so peak memory remains proportional to the largest eligible input and Latin-1 conversion can expand it. Shared manifest-loader bounds, scheduler behavior, large reply construction and release-scale measurements remain separate work; this patch does not establish those guarantees.

Reproduce the measured comparison in this checkout with the retained script and new, unused output prefixes:

```powershell
& ./target/fixture-token-cost-probe-v2.ps1 -BinaryPath ./target/fixture-sync-freshness-qualified-windows/mcp.exe -OutputPrefix ./target/fixture-token-cost-baseline-next
& ./target/fixture-token-cost-probe-v2.ps1 -BinaryPath ./target/fixture-token-scan-final-windows/mcp.exe -OutputPrefix ./target/fixture-token-cost-candidate-next
python ./target/compare-fixture-token-probes.py ./target/fixture-token-cost-baseline-next.json ./target/fixture-token-cost-candidate-next.json ./target/fixture-token-cost-comparison-next.json
```

Use the pinned toolchain and maintained Windows wrapper recorded in the previous checkpoint for code validation. Preserve frozen binaries and failed attempts; later audit prose changes build identity inputs. The user authorized committing and pushing this batch at the stopping point. The installed MCP and registration are unchanged, so no restart is needed for the existing installation. The [full inventory workplan](../2026-09-06-functional-performance-followup.md) remains incomplete. Stop here until work is resumed.
