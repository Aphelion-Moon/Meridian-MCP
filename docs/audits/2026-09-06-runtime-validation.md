# Runtime request validation and ownership diagnostics

Based on `1d63296`. This batch audits DreamDaemon launch/output-wait paths and improves evidence for the intermittent ownership fixture. It does not establish a cause or repair for that intermittent failure.

## Reproduced launch defect

`dm_run` compiled its readiness regex only after creating the runtime, replacing its output session and waiting 250 ms. A malformed expression therefore had process side effects before returning an error. The regression used the real MCP dispatcher and an owned purpose-written runtime executable: the failing baseline both replaced the runtime session and wrote the fixture's PID marker.

Launch arguments also used permissive JSON conversions. A string in `require_verified_provenance` became the default `false`; incorrectly typed `daemon_args`, readiness and working-directory options could be ignored. A regression demonstrated that invalid arguments waited for lifecycle access instead of being rejected. Another demonstrated acceptance of a string output-wait regex flag. These failures were observed before the production repair.

The repaired path validates declared launch fields before lifecycle access, provenance evaluation, output-session replacement or process creation. Readiness regexes are compiled once and reused while matching output. Invalid requests return a structured `invalid_input` tool error naming the field. The internal profiled-runtime entry point uses the same validation. This does not qualify all separate Tracy/debugger request parsers.

Output waits validate string, boolean and nonnegative integer types before consulting runtime state. Their existing behavior is preserved: zero performs an immediate check, omitted timeout defaults to 30 seconds, and larger unsigned values are capped at five minutes. The launch timeout remains 1–300,000 ms. The launch regression also verifies successful regex readiness and rejection of an unmanaged DMB when strict provenance is requested; existing stop/cancellation fixtures passed in both full suites.

## Ownership evidence

The previous Linux/WSL 1 failure timed out before the abrupt-owner startup marker appeared. The assertion did not record owner exit status or startup phase. Test owners now write a small phase journal covering executor creation, owner initialization, launch polling, PID observation, identity publication and cancellation. The parent detects early owner exit while awaiting startup and includes phase/marker/exit evidence in failures. Successful cases remove their owned phase files; failure evidence remains available for diagnosis. No production guardian code or deadline was changed.

An initial full Linux run with phase instrumentation passed 420 tests, with zero failures and six ignored gates. That run preceded the request-validation repair. The earlier intermittent failure remains unresolved; a passing rerun is not a cause or a fix.

## Qualification

- Red regressions: malformed regex launched the owned fixture; invalid launch options waited for lifecycle access; incorrectly typed output-wait flags were accepted.
- Initial focused green run: all three new regressions passed, along with two existing tests selected by the same filter.
- Final Rust 1.95.0 suites: **430 Windows tests passed, zero failed, five ignored; 423 Linux/WSL 1 tests passed, zero failed, six ignored**. Strict all-target/all-feature Clippy passed on both platforms, as did formatting and diff checks.
- Actual Windows stdio server: three malformed requests returned `invalid_input`, no runtime started, and the MCP exited naturally with code zero. This used the debug binary built by Cargo's full test gate.
- Six Linux ownership-fixture runs in two batches of three passed all **42 lifecycle cases**. Individual runs lasted 1.32–1.34 seconds; the latest recorded owner phase was 43 ms. This is bounded repetition evidence, not a production startup benchmark or an explanation for the earlier intermittent failure.
- Live BYOND, hosted CI, a new release build and replacement of the connected MCP binary remain separate gates. No production guardian code or deadline changed.

The [portable verification record](2026-09-06-runtime-validation-evidence.json) records source hashes, test counts, local log hashes and ownership phase summaries. Raw logs remain under ignored `target/`. The maintained provenance document was also corrected to identify the current `meridian-read-policy-v3` patch; historical audit records retain their original identities.

Run Cargo from a Visual Studio developer shell on Windows, or a native Linux shell with the repository's native PowerShell prerequisite:

```text
cargo +1.95.0 test --locked --lib invalid_ --no-fail-fast
cargo +1.95.0 test --locked --lib owned_runtime_lifecycle_keeps_unrelated_sentinel_alive -- --nocapture
cargo +1.95.0 test --locked --all-features --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
```

## Next runtime audit targets

Direct source inspection identified three additional contracts to reproduce before changing behavior:

1. `topic` retains the runtime mutex across its network request. `send_topic` moves only connection establishment to `spawn_blocking`; subsequent synchronous reads/writes execute within the async function. Verify timer, status and stop responsiveness against a delayed owned loopback peer, including one overall timeout budget.
2. The advertised `working_directory` option says it controls DreamDaemon's directory, while launch currently always uses the canonical DMB parent. Argument containment also resolves the DMB before the handler sees the requested directory. Verify relative and absolute paths through the MCP dispatcher.
3. The runtime contract describes a loopback launch, but the default DreamDaemon argument vector has no explicit bind-address flag and appends caller-supplied arguments. Qualify actual listener behavior and make the contract and invocation agree.

These remain open in the [full follow-up workplan](2026-09-06-functional-performance-followup.md).
