# Compiler input and artifact audit

Based on `66d6f9d`. This batch covers the standard `dm_compile` dispatcher, argument validation, missing-artifact results and failed-attempt recording.

## Reproduced defects

- Relative DME paths were resolved before the supplied working directory. The existing compiler helper test passed while an actual stdio request returned `path_not_found` for an existing file.
- Invalid option types were silently defaulted or filtered. Fifteen malformed requests started the owned compiler fixture. A native BYOND request with `defines: ["FLAG", 42]` also compiled and created a DMB while echoing the unfiltered array back in the response.
- A compiler exit code of zero with no parsed errors returned `success: true` even when no DMB existed.
- With no artifact or fixture manifest, that missing-artifact failure was not retained in the private build-attempt history.

The first four regression tests failed against the prior source. A fifth test subsequently reproduced the missing attempt record. The corresponding five tests and all 11 existing compiler-runner tests pass after repair.

## Resulting behavior

Compiler and runtime dispatch share canonical working-directory resolution. Each requested directory must exist, be a directory and remain inside the configured roots. The relative DME is resolved against it and receives its own containment check. An escaping path is rejected. Without a working-directory argument, compilation still runs from the DME's parent.

Compiler options are parsed before process creation. Incorrect types, unknown fields, malformed define arrays and NUL bytes return `invalid_input`. The documented minimum timeouts are enforced; larger valid integer timeouts still cap at 30 minutes for total duration and 15 minutes for idle time. Defines with or without the `-D` prefix remain supported.

`compiler_succeeded` reports a successful compiler exit with no parsed errors. `success` additionally requires the DMB to exist. A missing DMB returns a tool error with `artifact_error`, and a configured private state store retains the failed attempt. These checks do not validate arbitrary DMB contents. Freshness and input provenance remain separate through `dmb_updated` and the provenance fields; an unchanged artifact is not promoted to verified merely because the compiler exited successfully.

## Native and platform verification

The [native probe](native-probe.ps1) drives the actual stdio MCP and installed Windows BYOND 516.1687. It parses a small owned DM fixture before compiling, uses fresh output directories and compares default, absolute-with-directory, relative-with-directory and malformed-defines requests. The [baseline](baseline.json) records the original path and validation defects. The [candidate](candidate.json) creates fresh DMBs with verified input provenance in all three valid cases and rejects the malformed defines without creating a DMB. Both MCP sessions exit naturally with code zero.

Full Rust 1.95.0 suites passed **444 tests on Windows** (zero failed, five ignored) and **437 on Linux/WSL 1** (zero failed, six ignored). Strict all-target/all-feature Clippy passed on both platforms. Formatting, patch whitespace and probe syntax checks passed. The [verification record](verification.json) binds final source, regression logs, platform gates and native binary identities by SHA-256. Raw logs, responses and frozen binaries stay under ignored `target/`.

```powershell
./docs/audits/2026-09-06-compiler-inputs/native-probe.ps1 -BinaryPath $env:MCP_BINARY -DreamMakerPath $env:BYOND_COMPILER -OutputDirectory ./target/compiler-native-probe
```

This is a small-fixture compile gate, not a Meridian-Rift production build or runtime test. Linux qualification uses WSL 1. Hosted CI, live debugger/Tracy gates, release packaging and connected-MCP replacement remain separate. The [broader workplan](../2026-09-06-functional-performance-followup.md) remains active.

## Response-limit follow-up

An additional owned compiler emits 600,000 bytes on each output stream and creates a DMB. The process runner retains 512 KiB per stream. Their combined JSON reply exceeds the tool's 1 MiB limit, so the server replaces the entire compile result with `limit_exceeded`; compilation and artifact status disappear. The [prior binary](response-baseline.json) produced 1,050,745 response bytes and the [current candidate](response-candidate.json) produced 1,050,806, both above 1,048,576. This problem remains unfixed and is the next compiler-response/efficiency repair. The byte difference reflects the added result fields, not a measured performance regression.

Reproduce from the repository root in a Windows Visual Studio developer shell:

```powershell
rustc +1.95.0 --edition=2021 ./docs/audits/2026-09-06-compiler-inputs/output-fixture.rs -o ./target/compiler-output-flood.exe
./docs/audits/2026-09-06-compiler-inputs/response-probe.ps1 -BinaryPath $env:MCP_BINARY -OutputDirectory ./target/compiler-response-probe
```

The response probe collects selected fields and retains full raw responses only in its local output directory. It is an output-volume fixture, not BYOND compile evidence.
