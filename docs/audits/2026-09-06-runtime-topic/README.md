# Topic responsiveness audit

Based on `c8a8e79`. This repair covers standard `dm_topic` requests and the shared socket helper used by Tracy's wake request. Working-directory and listener-binding repairs remain separate work.

## Reproduced defects and repair

The prior implementation retained the runtime mutex across a Topic exchange and used synchronous socket reads/writes inside its async function. Only connection establishment ran in a blocking worker. Socket timeouts applied separately to individual I/O operations rather than the complete exchange.

Purpose-written loopback peers reproduced the consequences:

- A delayed reply prevented an unrelated async timer from running before the peer responded.
- A request with a 100 ms limit accepted a trickled response after 306 ms; a second baseline run accepted one after 311 ms.
- Status and stop could not acquire the runtime lock while a Topic response was pending.
- Invalid Topic arguments waited for runtime access instead of being rejected.

The socket exchange now uses asynchronous connection, write and read operations under one timeout. Cancellation drops the owned socket. The public tool retains the original output-session identity, releases the runtime lock before network I/O, and cancels the exchange when that runtime ends. A late reply cannot become a reply from a replacement session.

Topic arguments are validated before runtime access. Timeouts are integers from 1 to 60,000 ms, matching the existing tool contract, with the existing 5,000 ms default. Invalid types, embedded NUL bytes and oversized Topic packets return `invalid_input`. Elapsed requests return `timed_out`; other socket/protocol failures return `external_tool_failure`. String/float decoding and the BYOND wire format remain unchanged.

The cancellation fixture initially returned Windows socket error 10035 (`WouldBlock`) immediately: its accepted peer inherited the listener's nonblocking mode. That was a fixture error, not evidence of failed cancellation in the repaired implementation. Explicit blocking mode on the peer's bounded OS thread corrected the fixture. The corrected test verifies cancellation and socket closure. The independent timer, total-deadline, lock and input-validation regressions were observed failing before the production repair.

## Qualification

- Focused Windows run: eight tests passed, including all five new regressions and the existing packet, decoder and Tracy summary cases.
- Full suites: 435 passed, zero failed, five ignored on Windows; 428 passed, zero failed, six ignored on Linux under WSL 1. The Linux result is local qualification, not hosted native Ubuntu evidence.
- Strict all-target/all-feature Clippy passed on both platforms with Rust 1.95.0. Formatting, patch whitespace and the live probe's PowerShell syntax checks passed.
- The [verification record](verification.json) binds the final source files and local test logs by SHA-256. Raw logs remain under ignored `target/`.
- No broad latency, token savings or live Tracy claim follows from these focused responsiveness checks.

```text
cargo +1.95.0 test --locked --lib topic_ --no-fail-fast
cargo +1.95.0 test --locked --all-features --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
```

Use a Visual Studio developer shell on Windows. Linux requires native PowerShell for the repository's script integration gates.

## Live launch baseline for the next repair

The [portable live baseline](launch-baseline.json) uses the prior tested debug binary, retained by SHA-256, and installed Windows BYOND 516.1687. Its production source is the code committed as `c8a8e79`; the binary reports its earlier parent revision and dirty flag because it was built before that commit.

The first sandbox-context compile produced no output or DMB before its 30-second deadline and was terminated. The same purpose-written fixture compiled in the approved native user context. This repeats the context distinction recorded by the earlier remediation audit; it does not identify a loader cause or justify an installation change.

The native run directly observed three launch defects: the default listener was `0.0.0.0`; an absolute DMB ignored the requested working directory; and a relative DMB with a working directory failed with `path_not_found`. The engine's [startup reference](https://www.byond.com/docs/ref/info.html#/proc/startup) documents that it normally changes to the DMB directory and uses `-cd` for an override, with `-ip` selecting a numerical bind address.

One baseline ping returned `pong`; a later ping hit its one-second socket timeout. That timeout is retained and is not attributed to a cause here. The directory/listener observations remain valid; this run does not qualify all runtime behavior. The MCP exited naturally with code zero. Raw responses and binaries stay under ignored `target/`.

The [live probe](live-launch-probe.ps1) can be rerun from the repository root with explicit binary, compiler and a fresh output directory:

```powershell
./docs/audits/2026-09-06-runtime-topic/live-launch-probe.ps1 -BinaryPath $env:MCP_BINARY -DreamMakerPath $env:BYOND_COMPILER -OutputDirectory ./target/runtime-launch-probe
```

The full [workplan](../2026-09-06-functional-performance-followup.md) remains active. Live BYOND candidate qualification, hosted CI, release packaging and replacement of the connected MCP are separate gates.
