# Rift build output and evidence

The Rift wrapper could report success after an early error or invalid result record had fallen out of its log tail. Large replies could instead lose the entire build result to the transport limit. This repair observes output before tail eviction, bounds returned details, and records classified failures even when no artifacts exist.

## Reproduced and repaired

The initial fixture run failed four of five tests. It demonstrated a false success after an early error, a lost cache hit, a 1,094,987-byte reply, and a missing failed-attempt record. Invalid output options were already rejected as unknown fields; they now have explicit type and range validation.

- Both streams are analyzed as they arrive. Error totals survive diagnostic-row limits, including a limit of zero.
- Every `RIFT_RESULT` line contributes to validation. An early malformed or duplicate record cannot disappear behind a later valid record. The exact DM cache marker survives tail eviction too.
- A line exceeding 1 MiB or incomplete pipe reads prevents success. Later analyzable lines still contribute evidence. Existing explicit process/build failures retain priority.
- Rift shares the direct compiler's UTF-8/JSON response budgeting and line framing. Defaults are 8192 UTF-8 bytes per stream and 50 error lines; `include_output: false` omits logs. Artifact hashes and outcome/provenance remain available within 512 KiB. [Output contract](../../compiler-output.md)
- Classified unsuccessful builds now record a failed attempt even without a DMB or fixture manifest. Successful wrapper evidence still does **not** prove the compiler input closure or establish verified provenance.

## Matched stdio comparison

Three alternating A/B rounds used fresh private state, nine synthetic wrapper cases per arm, and the same owned compiler fixture. The candidate additionally exercised four output-control cases per round: **66 build calls in six sessions**, including 54 matched default calls. All 60 produced artifact pairs matched by hash. Each server advertised 36 tools and exited naturally.

| Case | Baseline | Candidate |
| --- | --- | --- |
| Early error, malformed result, duplicate result, oversized line | 12 false successes across three rounds | All 12 rejected |
| Early DM cache marker | Three false failures | Three valid cache hits |
| Large diagnostic output and two-stream flood | Six transport size errors | All six return bounded build evidence |
| Failure without artifacts | Three missing attempt records | Three failed attempts recorded |
| 30,000 errors; detail limits 0 / 2 / 200 | Options unavailable | Exact total and requested number of rows |

Median response-text sizes in bytes:

| Case | Baseline | Candidate |
| --- | ---: | ---: |
| Quiet success | 3,037 | 3,040 |
| Early error | 549,176, incorrect success | 11,802, failure |
| Early cache marker | 549,563, incorrect failure | 11,753, valid cache hit |
| Two-stream flood | 242-byte size error; build details lost | 20,146 |
| Candidate flood with logs omitted | — | 3,067 |
| Candidate 30,000 errors, no logs, zero rows | — | 3,202 |

Omitting the flood logs reduced the candidate reply by **84.8%**. Advertising the three new controls added 427 bytes to the serialized Rift tool definition. Quiet reply text grew three bytes; its outer JSON-RPC response shrank 32 bytes. These are byte measurements, not model-token or reasoning-cost measurements.

The flood's median end-to-end round trip increased from **248 to 312 ms**; other cases varied. Streaming analysis adds work inside the process runner, so `duration_ms` also covers different work than before. Three local debug-binary rounds do not establish a speedup, memory saving, or production latency claim. Full case timings are in [metrics.json](metrics.json).

## Qualification

- Windows: **466 passed, zero failed, five ignored**, Rust 1.95.0; strict all-target/all-feature Clippy passed. The first full run had 465 passing tests and one stale schema-field expectation. The explicit field list and output-bound assertions were updated before the complete rerun.
- Linux/WSL 1: **454 passed, zero failed, six ignored**, Rust 1.95.0; strict all-target/all-feature Clippy passed. Rift execution is Windows-only; Linux qualifies shared collection/formatting and the unsupported-platform response.
- Actual DreamMaker **516.1687**: an owned `RIFT_BUILD.cmd` wrapper compiled a tiny resource-bearing project and rejected an undefined procedure at `fixture.dm:5`. Both cases evicted over 225 KB from stdout capture, retained correct diagnostic totals, and exited the MCP naturally. The successful build produced both native artifacts with matching response hashes. Neither case established verified provenance.
- This is not a full Meridian-Rift production build, live playtest, hosted CI run, or installation test. The earlier intermittent WSL guardian readiness timeout remains unexplained.

The first comparison recorder stopped after its baseline session when it indexed a missing diagnostic array in a transport size-error response. That raw run remains under `target/rift-output-paired`; the completed experiment used a new directory after correcting the recorder. An earlier focused Cargo invocation named a nonexistent test target and did not execute tests; the corrected focused invocation passed 22 tests.

## Reproduction and evidence

Run from the repository root with Rust 1.95.0 and the platform's required native build tools. On Windows, use a Visual Studio developer shell:

```powershell
cargo +1.95.0 test --locked --all-features --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
rustc +1.95.0 --edition=2021 tests/fixtures/rift_output.rs -o target/rift-output-fixture.exe
./docs/audits/2026-09-06-rift-output/probe.ps1 `
  -BaselineBinaryPath target/compiler-diagnostics-paired/B.exe `
  -CandidateBinaryPath target/debug/meridian-mcp.exe `
  -CompilerPath target/rift-output-fixture.exe `
  -OutputDirectory target/rift-output-reproduction
./docs/audits/2026-09-06-rift-output/native-probe.ps1 `
  -BinaryPath target/rift-output-reproduction/B.exe `
  -DreamMakerPath $env:DM_EXE `
  -OutputDirectory target/rift-output-native-reproduction
```

Use fresh output directories. The frozen baseline SHA-256 is `19537b5831aa16abc84f4d89d5c63bd1e673c85a3000444423b45f51ecd03304`; the measured candidate is `ba962124348678716b928878817933c4b8e156c3dc5849cf300fd6689879ee3c`. The baseline came from the preceding compiler-diagnostic batch; its embedded precommit revision/dirty flag is retained in the data. Rebuilding a historical source revision does not recreate its exact binary identity.

The portable [comparison](comparison.json), [native results](native.json), and [verification manifest](verification.json) retain build identities, source/log hashes, outcomes, counts, and limitations. Raw protocol captures, artifacts and full logs stay under ignored `target/`. `record.py` validates the recorded experiment at its fixed paths; do not rerun it against changed source and relabel historical measurements.

Documentation-install cleanup, remaining tool-family audits and the release handoff remain active in the [workplan](../2026-09-06-functional-performance-followup.md). The installed MCP has not been replaced.
