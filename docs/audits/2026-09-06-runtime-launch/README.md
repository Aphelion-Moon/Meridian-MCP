# Runtime launch audit

Based on `628b8b9`. This batch repairs standard DreamDaemon launch settings and the shared launch path used by Tracy. It does not qualify the separate auxtools launcher or live Tracy capture.

## Findings and repair

The prior dispatcher resolved `dmb_path` before considering `working_directory`. Consequently, a relative DMB could fail even when it existed relative to the requested directory. After validation, the launcher always selected the DMB's parent and ignored the directory argument. It also omitted an explicit listener address despite advertising loopback launches.

The dispatcher now canonicalizes and contains the requested directory first, requires it to be a directory, resolves a relative DMB against it, then checks the resulting DMB's containment. The launcher sets both the process working directory and BYOND's `-cd` option. With no directory argument, the DMB's parent remains the default. Source integrity monitoring retains its existing active-DME/artifact root; selecting another working directory does not silently move that monitoring scope.

Both standard and profiled launches now supply `-ip 127.0.0.1`. Extra arguments cannot replace the managed DMB, port, working directory or listener binding. Validation distinguishes options from values consumed by `-params`, `-log`, `-home` and `-suid`, including inline values. World parameters containing numbers, file names or flag-like text remain supported. This controls the launch listener; it does not establish isolation for arbitrary networking performed by game code.

BYOND's [startup reference](https://www.byond.com/docs/ref/info.html#/proc/startup) documents the default directory behavior, `-cd`, `-ip`, positional ports and world-parameter options. Native observations below verify the directory and listener behavior independently.

## Regression evidence

Four purpose-written tests failed against the prior source:

- An owned executable observed the ignored directory, absent `-cd`/loopback arguments and failed relative-DMB launch.
- A file supplied as the working directory waited for lifecycle access instead of being rejected.
- An escaping relative DMB was resolved against the wrong base before containment could identify the requested target.
- Conflicting raw launch arguments were accepted.

All four pass after repair. The process fixture also verifies that modifying a source file beside the artifact is still recorded after launching from another directory. An additional test input exposed an overly broad initial rejection of an inline parameter ending in `.dmb`; that validation error was corrected before qualification.

## Native comparison

The [probe](live-launch-probe.ps1) compiles a small DM world and drives the actual stdio MCP with installed Windows BYOND 516.1687. It reads a different marker from each possible directory, observes listeners belonging to the returned process ID, checks world parameters, sends Topic pings, stops each owned runtime and closes the MCP naturally.

| Case | Baseline | Repaired candidate |
| --- | --- | --- |
| Default directory | Artifact directory; wildcard listener | Artifact directory; loopback listener |
| Absolute DMB and requested directory | Requested directory ignored | Requested directory used and correctly reported |
| Relative DMB and requested directory | `path_not_found` | Launches from the requested directory |
| Flag-like, numeric and inline world parameters | Parameters received | Identical parameters received |

The candidate passed every ping and stop. All four observed listener sets contained only `127.0.0.1`. The baseline's relative-DMB case could not launch; its subsequent Topic request therefore had no running game.

This probe keeps the fixture active when idle, waits for a post-initialization marker and uses the normal five-second Topic timeout. Baseline and candidate use the same protocol and fixture source. The earlier [one-second ping timeout](../2026-09-06-runtime-topic/README.md) remains recorded; these different probe conditions do not diagnose its cause or demonstrate a speedup.

Run from the repository root in a native context where the installed compiler works, using a fresh output directory:

```powershell
./docs/audits/2026-09-06-runtime-launch/live-launch-probe.ps1 -BinaryPath $env:MCP_BINARY -DreamMakerPath $env:BYOND_COMPILER -OutputDirectory ./target/runtime-launch-probe
```

## Qualification and remaining work

With Rust 1.95.0, the full suites passed **439 tests on Windows** (zero failed, five ignored) and **432 on Linux/WSL 1** (zero failed, six ignored). Strict all-target/all-feature Clippy passed on both platforms. Formatting, patch whitespace and PowerShell probe syntax checks passed.

The [verification record](verification.json) binds source files and test logs by SHA-256. The [baseline](baseline.json) and [final candidate](candidate.json) retain binary/build identities, compiler version and individual native results. Their fixture source hashes and world-parameter output match; the candidate returned exactly `pong` four times. The final native run used the fully rebuilt Windows binary after all source/schema changes. Raw logs, responses and frozen binaries remain under ignored `target/`.

The [broader workplan](../2026-09-06-functional-performance-followup.md) remains active. Linux results are local WSL 1 evidence. Hosted CI, release packaging, connected-MCP replacement, real Meridian-Rift playtesting and live debugger/Tracy qualification remain separate gates.

A [separate stdio probe](compile-followup.json) reproduced the same ordering defect in `dm_compile`: an existing `fixture.dme` relative to its supplied working directory returns `path_not_found`. The compiler helper's existing relative-path test does not cover dispatch. That repair and compiler/artifact-path review are next; compiler behavior is unchanged by this launch batch.
