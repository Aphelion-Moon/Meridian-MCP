# Retained search storage audit

The final candidate reduced median parsed-process private memory by **74.6 MB** and working set by **15.1 MB** in the matched Windows run. All 2,340 replies matched. Timing did not establish a general speedup.

Based on `7755d81` plus this compaction. The [source-excerpt experiment](../2026-09-06-source-excerpts/README.md) remains the prior measured baseline. SpacemanDMM stays at the same upstream pin and `meridian-read-policy-v3` local patch.

## Finding and repair

The search index grows vectors while collecting documents, postings and exact-lookup IDs, then retains them unchanged for the entire analysis snapshot. The read-only [baseline storage probe](baseline-storage.json) found 452,780 documents and 12,587,530 postings on the pinned Meridian-Rift tree. Their vector backing storage, including exact-lookup IDs and document lengths, occupied 464,124,896 bytes for 357,156,800 bytes of live rows: **106,968,096 bytes (23.0%) were spare growth capacity**.

The constructor releases document-vector spare capacity before indexing and compacts completed posting/lookup vectors before installing the snapshot. Exact lookups store a single document ID inline and allocate a list when a second document shares the key. Index entries, weights, source budgets and response schemas are unchanged. A regression first failed with 448,616 retained bytes for 137,752 bytes of fixture rows; it checks that the finished index stays within a 5% backing-storage budget and still resolves the last inserted symbol. A second regression reproduced needless singleton buffers; a five-override fixture checks list growth and source selection.

The probe also found 104.59 MB of separately allocated metadata text containing only 24.53 MB of unique text. File paths alone occupied 45.05 MB for 0.98 MB of unique paths. This identifies a further sharing opportunity; this batch measures capacity compaction independently. Source text and documentation remain intact.

The earlier stage samples show substantial preprocessing variation and a measurable DreamChecker stage. The parser comment now describes the diagnostic contract instead of presenting an older checker opt-out experiment as a current cost estimate. DreamChecker still runs for every new snapshot.

## Initial measured candidate

The [initial compaction-only results](initial-candidate.json), [summary](initial-summary.json) and [structural probe](initial-candidate-storage.json) are preserved. All 2,340 replies matched, source state was unchanged, warm reuse succeeded, and all six processes exited naturally. Removing 106.97 MB of vector spare capacity reduced median process private bytes by only **52.37 MB (2.67%)**; working set increased 10.14 MB. Cold-parse median increased from **31.35 to 32.32 seconds (+3.1%)**, and index-construction median increased from 7.884 to 8.674 seconds. The initial candidate is therefore evidence of a mixed tradeoff, not a speedup.

This prompted the inline-ID refinement: most canonical symbols have one implementation, so allocating and subsequently shrinking a vector for each is avoidable. The same original baseline and unchanged 78-query protocol are retained for its qualification.

## Final measurements and decision

The [final results](results.json) and [summary](summary.json) record another 2,340 queries, three fresh processes per arm, five repetitions per query, successful warm reuse, unchanged source Git state and 22 inspected file hashes, and six natural exits with code zero. Complete replies matched across both binaries and all repetitions, excluding only build identity. Each 78-query pass returned the same **386,944 characters/UTF-8 bytes**. No response content or token-volume reduction is attributed to this batch.

| Measure | Original baseline | Final candidate | Observed change |
| --- | ---: | ---: | ---: |
| Parsed private memory median | 1,962.66 MB | 1,888.03 MB | −74.63 MB / 3.80% |
| Parsed working set median | 1,831.83 MB | 1,816.75 MB | −15.08 MB / 0.82% |
| Cold-parse median | 25.35 s | 26.27 s | +0.92 s / 3.63% |
| Index-construction median | 6.086 s | 6.268 s | +0.182 s |
| Warm-reuse median | 1.256 s | 1.339 s | +0.083 s |

MB uses decimal units. All three final-candidate process memory samples were below their baseline counterparts. Cold-parse samples were 25.35/24.77/32.77 seconds for the baseline and 26.27/24.60/26.90 for the candidate; paired differences have mixed signs. Timing is variable, and no startup or query speedup is established. The broad `datum` query's median was 78.8 ms versus 90.0 ms; exact-inspection query medians ranged from 0.20–0.64 ms versus 0.24–0.68 ms. These samples do not justify a general latency improvement claim.

The [final structural probe](candidate-storage.json) found **451,290 single-ID symbol entries and 51,085 single-ID name entries**. These avoid 502,375 lookup buffers. The entry remains 24 bytes, equal to the previous vector descriptor on this target. Measured vector backing storage fell from 464.12 MB to 353.14 MB; this 110.99 MB structural reduction is distinct from the smaller process-memory reductions above. Document, posting, metadata, documentation and source-payload counts remained unchanged.

**Decision:** retain the change as a memory optimization for long-lived parsed snapshots. Keep the selective-parser workflow and explicit source controls from the earlier comparison. Duplicate metadata text and broad-query latency remain further performance opportunities; this result does not establish that parsing is cheaper than text search for ordinary exploration.

The final Windows release SHA-256 is `048c4cb970514ac7a06f784d232b2f5d11f923d3043101de5c566d3576dafcbe`. Binary build identities and hashes are recorded in the results; both measured builds correctly report working source changes. Raw replies and retained binaries remain under ignored `target/`.

## Verification

- Initial compaction candidate, Rust 1.95.0 (`59807616e`): **425 Windows tests passed, zero failed, five ignored; 418 Linux/WSL 1 tests passed, zero failed, six ignored**. The new opt-in storage probe accounts for one additional ignored test on each platform.
- Strict all-target/all-feature Clippy passed on both platforms; formatting and the focused ten-test search suite passed. The capacity regression was observed failing before the repair.
- The baseline structural probe used release code; the initial candidate probe used debug code. All document/posting/text cardinalities and payload counts matched; every measured initial-candidate vector capacity equaled its live row count. These are structural comparisons. Their parse timings are deliberately excluded from the performance comparison.
- The initial release candidate built successfully and is identified by SHA-256 `36a1270cde9dca2ba7387e6e81fe1feec1f6a5fa246d10d86ffad7ded5a1440d`. Repeated Visual Studio shell initialization emitted a command-length warning in the local wrapper; the existing developer environment remained usable and Cargo completed with exit zero. Subsequent gates invoke the wrapper once per shell.

The same local WSL 1 environment and native repository-local PowerShell prerequisite used by the source-excerpt batch were retained. Passing these suites does not establish a cause or repair for the earlier intermittent guardian timeout.

The first full Linux run after the inline-ID refinement finished with **419 passed, one failed, six ignored**. `owned_runtime_lifecycle_keeps_unrelated_sentinel_alive` passed stop/drop/EOF/transport-error/no-executor/cancellation cases, then exceeded its unchanged eight-second startup-marker deadline in the abrupt-owner case. Neither that case's marker nor its PID file remained afterward. No guardian or runtime-ownership code was changed. The failed run is retained; the fixture needs isolation and better phase/exit diagnostics before assigning a cause.

Final verification for the inline-ID candidate: **427 Windows tests passed, zero failed, five ignored; 420 Linux/WSL 1 tests passed, zero failed, six ignored**. All twelve focused search tests, strict Clippy on both platforms, formatting and the release build passed. After the Windows build workload stopped, the lifecycle fixture passed alone in 1.38 seconds and then in the complete Linux rerun. Its deadlines were unchanged. The initial failure remains an unresolved integration finding; these passes do not prove its cause or repair.

The source-derived SpacemanDMM audit passed for all 50 registry records, 128 source entries and debugger wire layouts. All 143 capability-drift assertions passed. The final release binary also passed the documentation stdio fixture: HTML generation, custom index, unchanged source files, overwrite rejection and natural shutdown.

## Reproduction and scope

The [protocol](protocol.md) and [runner](run.py) compare 78 query variations across seven subsystem groups, including exact inspection, source controls, broad/common terms and filters. Default settings produce 2,340 query samples across six fresh processes. Complete reply equality excludes only build identity; unchanged counts alone are insufficient.

```powershell
$env:MERIDIAN_SCALE_DME = Join-Path $env:MERIDIAN_RIFT_ROOT 'tgstation.dme'
cargo +1.95.0 test --locked --release --lib retained_search_storage_profile -- --ignored --nocapture
python ./docs/audits/2026-09-06-search-storage/run.py --baseline $env:BASELINE_MCP --candidate ./target/release/meridian-mcp.exe --rift-root $env:MERIDIAN_RIFT_ROOT --output ./target/search-storage-comparison --rounds 3 --repeats 5
python ./docs/audits/2026-09-06-search-storage/summarize.py ./target/search-storage-comparison/results.json ./target/search-storage-comparison/summary.json
```

Storage-probe payload counts exclude allocator metadata, hash buckets and the parser AST. Process private bytes and working set come from the separate matched release experiment. Query characters/bytes do not measure billed tokens or reasoning. Hosted CI, live BYOND, installation and the earlier intermittent WSL 1 guardian timeout remain separate qualifications.
