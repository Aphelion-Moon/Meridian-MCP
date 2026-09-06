# Source identity and platform qualification

Based on `8b62d6c` plus this repair. The earlier language-query measurements remain unchanged; this batch makes no latency or memory improvement claim.

## Reproduction and repair

On Linux, two real files named `Source.dm` and `source.dm` each defined a different type and a `resolve` procedure at the same line and column. The language index returned six document symbols for either file instead of three. Exact search for the upper file's procedure also returned the lower file's body under the upper file's ownership metadata.

Both identity tables unconditionally lowercased paths and replaced backslashes. They now preserve native paths outside Windows. Windows retains its existing case and separator normalization. Search terms remain case-insensitive; this change concerns file identity.

`tests/source_identity.rs` exercises the tools against a parsed DME with two distinct Linux files. Both tests failed before the repair and passed afterward. A Windows fixture checks case aliases in the include and document queries. Fixtures require zero parse errors, so a skipped include cannot silently pass the test setup. This qualifies the tested Windows and Linux behavior; it does not establish filesystem behavior on every Unix platform.

The Linux run also exposed a test harness assumption: the nested Cargo build in `tests/build_identity.rs` inherited the outer `CARGO_TARGET_DIR`, but searched for its executable in the fixture's default target directory. Setting an explicit private fixture target fixes that failure. The existing failing test passed with the same outer target override after the change.

## Evidence and limits

- Exact local toolchain on both platforms: Rust 1.95.0 (`59807616e`). Linux dependencies were already available and all Cargo runs used `--offline --locked`.
- Focused Linux tests: nine language-query tests, one relevance test and two source-identity tests passed. The Windows equivalents passed with one platform-specific identity test.
- Windows all-feature suite: **416 passed, zero failed, four existing ignored gates**.
- The local Ubuntu environment is **WSL 1**, kernel `4.4.0-19041-Microsoft`, using a separate `target/linux` build directory. It is not native Ubuntu CI evidence.
- One full Linux attempt produced eight guardian-readiness/lifecycle failures. The first Linux library run and a subsequent isolated library run passed all 143 tests, with two ignored fixture entry points. An owned-process probe measured 26 successful guardian starts, including two batches of 12 concurrent starts; the slowest readiness after spawning was 23.788 ms against the unchanged two-second deadline. These observations do **not** identify the intermittent failure's cause or demonstrate its repair. No guardian code or timeout was changed.
- The complete Linux attempt (`--no-fail-fast`, with the Windows test workload stopped) finished with **396 passed, 13 failed and five existing ignored gates**. All 13 failures were missing native `pwsh`: five BYOND compatibility fixture tests, one capability-registry audit test and seven process-readiness tests. The guardian tests passed in this attempt, without changes. Do not describe this host's full Linux qualification as passing or the intermittent guardian issue as fixed. Windows PowerShell does not substitute for executing Linux scripts against native Linux paths.
- Hosted CI, installed MCP replacement and live BYOND remain separate gates. This batch does not change an already-running MCP process.

Warnings-denied all-target/all-feature Clippy passed on both Windows and Linux. `cargo fmt --all -- --check` and `git diff --check` passed. No release build or new performance benchmark was required for these identity and fixture changes; earlier release measurements remain tied to their recorded binary hashes.

## Next reproduced retrieval defects

A separate owned stdio fixture found three issues for the next batch:

1. Both exact procedure inspection and ranked search include the next sibling procedure when declarations are nested under an indented type block.
2. Search accepts `max_source_lines: 200` but can return only the 80 lines retained in its snapshot, without reporting truncation.
3. A string value for `include_source` is silently treated as the default `true`, and an out-of-range result limit is clamped rather than rejected.

Repair these with parser fixtures and explicit snapshot/excerpt metadata. The pinned parser already records `ProcValue.body_range`; check its boundary semantics before adding more text heuristics. Preserve the bounded snapshot storage; do not claim better relevance, lower memory or fewer tokens without matched measurements on varied subsystems.

## Reproduce

Run Cargo from a configured Visual Studio developer shell on Windows:

```powershell
cargo +1.95.0 test --locked --test source_identity --test language_queries --test search_relevance --test build_identity
cargo +1.95.0 test --locked --all-features
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.95.0 fmt --all -- --check
```

On Linux, use a native Rust 1.95.0 toolchain and native PowerShell for the script integration gates. A separate build directory can be selected without affecting the nested build-identity fixture:

```sh
export CARGO_TARGET_DIR=target/linux
cargo +1.95.0 test --locked --test source_identity --test language_queries --test search_relevance --test build_identity
cargo +1.95.0 test --locked --all-features --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
```
