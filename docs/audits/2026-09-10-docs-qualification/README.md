# Documentation preservation and readiness qualification

This batch repairs documentation output preservation and missing timeout progress evidence. It follows the [September 9 checkpoint](../2026-09-09-docs-preservation/README.md), whose failing runs and source hashes remain historical records. The broader [audit workplan](../2026-09-06-functional-performance-followup.md) is still open.

## Behavior

Documentation generation rejects output files and final links before canonicalization, including Windows junctions and trailing separator/dot forms. It also rejects outputs containing any recorded parsed input. Owned fixtures reproduced both nested-source deletion and linked-target deletion before these repairs.

A private empty-directory probe checks the output filesystem's no-replace rename support before running dmdoc. Native WSL storage supports the operation; this checkout's Windows-drive mount rejects it. Rejection now preserves existing docs, cleans staging and names the unsupported operation. This does not add support to that filesystem or qualify WSL 1. The smoke runner's new `-FixtureParent` option permits native Linux fixtures.

The readiness helper now retains a final process reading at timeout. Previously, a delayed poll could leave only an early sample and report no progress despite real CPU activity. The deterministic regression crossed a one-second deadline with a 1.2-second delay: the old helper returned at 1,250 ms with one sample at 4 ms and zero recorded progress. The repaired helper retains terminal CPU/progress evidence and still rejects a marker first observed after the deadline. It rechecks the deadline after reading marker contents.

Two positive readiness fixtures separately wait for their marker writers to initialize before releasing delayed work. Their three-second deadline is unchanged. The original Linux busy-process failure was not directly reproduced by three independent matched probes (each retained two samples); the controlled delayed-poll regression proves the missing-final-reading defect without attributing that earlier run to an unmeasured cause.

## Evidence

Pinned toolchain on both platforms: `rustc 1.95.0 (59807616e 2026-04-14)`. Linux used WSL 2 kernel `6.18.33.2-microsoft-standard-WSL2` and PowerShell 7.6.5. Platform qualification ran sequentially, without concurrent heavy Windows/Linux builds.

| Gate | Final result |
| --- | --- |
| Windows full tests | 481 passed, 0 failed, 5 ignored |
| Windows strict all-target/all-feature Clippy | Passed |
| Linux full tests, native temporary storage | 467 passed, 0 failed, 7 ignored |
| Linux strict all-target/all-feature Clippy | Passed |
| Mounted-filesystem rejection regression, explicit ignored test | 1 passed; rejected before helper execution |
| Frozen Windows candidate with pinned dmdoc | Passed |
| Frozen Linux candidate with freshly built pinned dmdoc; `-FixtureParent /tmp` | Passed |
| Formatting and whitespace | Passed |

Native smoke checks exercise generated HTML, configured index and type/member markers, source preservation, overwrite rejection and natural MCP exit. The Linux helper was built from SpacemanDMM `351ddc0ffb2439876d4565ce5130bb6b027ee605`; its SHA-256 is `0bc36a609a5dc0eca3651f9ee329c7dd4b1132f3dc16d71b8b48de49a488fc07`. Windows Git verified the upstream checkout clean. Raw WSL Git initially reported checkout line-ending differences; its separate preflight log is retained, and status with the checkout's CRLF normalization was clean.

[verification.json](verification.json) binds final source, logs and frozen executable hashes. Binaries and raw logs remain under ignored `target/`; preserve the older candidates as well. Later audit prose and commits change build identity inputs, so these binaries must retain their recorded identities.

The earlier source capability gate passed 50 registry records and 128 upstream source capabilities/debugger wire layouts. Mutation gates were not rerun in this batch. These checks do not establish a speedup or model-token saving.

## Remaining work

- Reproduce protection gaps for additional dmdoc inputs and stale snapshots. Upstream `crates/dmdoc/src/main.rs` rereads the live DME/config, scans configured or inferred Markdown module directories and loads an optional index. The current overlap guard covers recorded parsed inputs, not all those live inputs.
- Qualify full-project documentation, transport cancellation, abrupt server shutdown, hosted native Ubuntu and macOS separately.
- Continue the remaining tool inventory, beginning with fixture synchronization's active-snapshot freshness, repeated whole-file token reads and intermediate response size.
- Release packaging, installed-server replacement and post-restart acceptance remain separate steps.

The installed MCP and registration were not changed by this batch. No restart is required for the current installation.
