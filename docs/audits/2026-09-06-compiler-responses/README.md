# Compiler response audit

Based on `485f716`. This repairs the output overflow reproduced by the [compiler-input audit](../2026-09-06-compiler-inputs/README.md), without changing compiler execution or provenance decisions.

## Defect and repair

The process runner retains at most 512 KiB from each stream. Returning both streams, duplicated diagnostic text and build metadata could exceed the compiler tool's 1 MiB response ceiling. JSON escaping can expand captured bytes further. The server then replaced the entire result with `limit_exceeded`, hiding the build outcome and artifact evidence even though a DMB had been created.

Compiler replies now apply separate budgets before reaching that ceiling: 64 KiB of serialized JSON per stream, 96 KiB per diagnostic severity, and 96 KiB for combined metadata, with space for summaries under a 512 KiB reply ceiling. Output defaults to an 8192-byte UTF-8 tail per stream; diagnostics default to 50 rows per severity. Long messages retain a marked prefix. Rare oversized paths are omitted whole, and metadata omissions are explicit. Artifact hashes, existence, build record association and provenance status remain available.

`include_output`, `output_max_bytes` and `diagnostic_limit` are validated before execution. Counts cover all parsed captured output, regardless of returned rows. Process-capture loss remains separate from response omission and makes diagnostic completeness false. No later log-retrieval API is added. See the [public response contract](../../compiler-output.md).

The initial four-test run had three failures and one passing invalid-input test. After repair, all 20 initially focused compiler tests passed. Two additional cases exercise maximum output with every ASCII control character and simultaneous large metadata/diagnostic budgets; the final full suites include all six new regression tests.

## Matched MCP measurements

The [probe](probe.ps1) used eight owned compiler-output workloads, three alternating baseline/candidate rounds, and default versus output-omitted candidate requests: **72 compile calls in six naturally exiting MCP sessions**. Both binaries used the same compiled fixture and fresh per-request workspaces. They were debug builds. The baseline binary was frozen before this repair and contains the compiler-input fixes committed as `485f716`; its embedded revision is `66d6f9d` with a dirty source flag. The candidate embeds `485f716` with a dirty flag. Exact binary and build identities are in [comparison.json](comparison.json).

Median response text sizes, in UTF-8 bytes:

| Workload | Baseline default | Candidate default | Candidate `include_output: false` |
| --- | ---: | ---: | ---: |
| Quiet compiler note | 2,390 | 2,514 | 2,487 |
| 40,000 ASCII bytes per stream | 82,393 | 18,921 | 2,519 |
| 20 errors and 10 warnings | 7,756 | 6,385 | 5,268 |
| 600 errors and 10 warnings | 115,296 | 16,796 | 8,042 |
| 600,000 ASCII bytes per stream | Build result lost | 18,911 | 2,509 |
| Unicode/control-character capture overflow | Build result lost | 18,916 | 2,513 |
| 100,000 control bytes per stream | Build result lost | 90,604 | 2,524 |
| One oversized error plus a warning | 402,697 | 23,152 | 6,794 |

The nine overflowing baseline calls returned only a 240-byte error body. Their pre-replacement results measured roughly 1.05–1.08 MB. Recovering these results is a **functional improvement**, not a byte-saving comparison against that small error body.

All 48 candidate calls retain the expected outcome, artifact hash and diagnostic totals. Thirty candidate/baseline pairs with an available baseline result preserve outcome fields, artifact hash/size/existence and provenance status. Returned diagnostic rows equal the baseline prefix, except explicitly shortened messages; output equals a suffix of the captured baseline stream. The 600-error case returns 50 error rows by default and still reports 600 errors. All 72 produced fixture artifacts have the same hash.

Quiet replies grow by **124 bytes** by default and remain 97 bytes larger than baseline with output omitted. The compiler tool definition adds **591 bytes** to tool discovery, including its three controls and descriptions. For the 80 KB ASCII log, default response text falls **77.0%**, and output omission reduces it **96.9%** against baseline. These are response byte measurements, not measured model-token or thinking savings. This is a useful control for verbose builds, not a universal reduction in per-call cost.

[Metrics](metrics.json) also retain request bytes, exact JSON-RPC wire bytes and round-trip times. Timing includes client JSON parsing and concurrent local qualification work; these three rounds do not establish a compiler speedup. No process-memory claim is made.

## Qualification and boundaries

- Rust **1.95.0**: **450 Windows tests passed**, zero failed, five ignored; **443 Linux/WSL 1 tests passed**, zero failed, six ignored. Strict all-target/all-feature Clippy passed on both platforms.
- Fresh source-derived SpacemanDMM audit: **50 records**, **128 source capabilities** and debugger wire layouts accounted for; **143 drift assertions** passed. The upstream revision and `meridian-read-policy-v3` patch are unchanged.
- The hash-verified `dmdoc` stdio gate passed: generated HTML, configured index, source preservation, overwrite rejection and natural MCP exit.
- [Native BYOND 516.1687 verification](native.json): three valid compile/path cases produced fresh DMBs with verified provenance; malformed defines were rejected before creating a DMB. The MCP exited naturally. The native probe used the normal Windows user context because the prior audit separately reproduced a BYOND hang in the sandbox context.
- The original intermittent WSL ownership-fixture failure did not recur; its cause remains unresolved. WSL 1 results do not replace hosted native Ubuntu CI. A small BYOND fixture does not qualify a full Meridian-Rift build or live debugger/Tracy operation.

[Verification](verification.json) binds source, fixtures, scripts and logs by SHA-256. Raw responses, logs and frozen binaries stay under ignored `target/`. No installed binary or connected MCP process was replaced. The broader [artifact/tool audit and release handoff](../2026-09-06-functional-performance-followup.md) remain active.

## Reproduction

Run from the repository root in a Visual Studio developer shell on Windows. Use the repository-pinned toolchain. Supply the frozen baseline and a freshly built candidate via environment variables.

```powershell
cargo +1.95.0 test --locked --all-features --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
rustc +1.95.0 --edition=2021 tests/fixtures/compiler_output.rs -o target/compiler-response-fixture.exe
./docs/audits/2026-09-06-compiler-responses/probe.ps1 -BaselineBinaryPath $env:BASELINE_MCP -CandidateBinaryPath $env:CANDIDATE_MCP -CompilerPath ./target/compiler-response-fixture.exe -OutputDirectory ./target/compiler-response-reproduction
./docs/audits/2026-09-06-compiler-inputs/native-probe.ps1 -BinaryPath $env:CANDIDATE_MCP -DreamMakerPath $env:BYOND_COMPILER -OutputDirectory ./target/compiler-response-native-reproduction
```

The [record validator](record.py) rebuilds this historical record from its original `target/compiler-responses-*` artifacts, checks matched reply content, and rejects a different baseline or missing qualification evidence. It does not silently relabel a new run as this recorded experiment.
