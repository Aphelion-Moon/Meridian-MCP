# Documentation preservation: September 9 stopping point

The [September 10 qualification](../2026-09-10-docs-qualification/README.md) supersedes the current-state guidance in this historical checkpoint. It records the subsequent readiness repair, passing full Windows/Linux gates and native dmdoc checks. This report and its verification JSON retain the earlier source and failure evidence.

Stopped at the user's request after the documentation repair and its final focused readiness check. Changes remain **uncommitted** on top of `2d0bb15`. This is a partial qualification checkpoint, not a release. The installed MCP and registration were not changed; no restart is required for this work.

This report supersedes the current-state guidance in the [earlier checkpoint](../2026-09-06-docs-checkpoint/README.md). Its September 6 artifacts remain historical evidence. The [broader workplan](../2026-09-06-functional-performance-followup.md) remains incomplete.

## Changes ready for review

- Documentation output validation rejects files and final directory links before canonicalization, including Windows junctions and trailing separator/`.` forms.
- Output overlap checks cover every input recorded by the parsed snapshot. Regression fixtures preserve nested included source directories both inside and outside the DME's parent.
- An owned, empty-directory probe checks no-replace rename support before launching dmdoc. Unsupported Linux filesystems produce an actionable error, preserve old documentation and clean staging.
- The README explains overwrite, output limits and installation/cleanup results. The generated tool reference now matches the existing 600-second timeout.
- The maintained dmdoc smoke script accepts an existing `-FixtureParent`, allowing Linux fixtures on native storage.
- Two readiness fixtures now finish PowerShell initialization before releasing their delayed marker writers. Their readiness deadline remains three seconds; production readiness code and timeouts are unchanged.

The initial preservation fixtures failed against the old implementation: the helper ran and the nested source or linked target contents disappeared. The new cases pass on Windows and native Linux. The unsupported-filesystem regression also failed before the preflight change because the helper ran before the opaque installation error.

## Verification and limits

Toolchain: `rustc 1.95.0 (59807616e 2026-04-14)`, matching the repository and CI pin. Linux used PowerShell 7.6.5 and WSL 2 kernel `6.18.33.2-microsoft-standard-WSL2`.

| Gate | Observed result |
| --- | --- |
| Windows full suite, before the final readiness fixture edit | 480 passed, 0 failed, 5 ignored |
| Windows strict all-target/all-feature Clippy, before that test edit | Passed |
| Windows documentation/path/contract focused tests | 21 passed |
| Linux documentation/compiler/path/contract focused tests | 30 passed, 0 failed, 1 ignored |
| Linux documentation installation unit tests | 2 passed |
| Explicit mounted-filesystem rejection regression | 1 passed; predates the final equivalent Unix validation simplification |
| Linux full suite, before the readiness fixture edit | 464 passed, **2 failed**, 7 ignored; both failures were readiness tests |
| Final focused readiness tests | Windows: 7 passed; Linux: 6 passed, **1 failed** (busy-process timeout samples) |
| Pinned Windows dmdoc stdio smoke | Passed with default fixture parent and explicit `-FixtureParent target` |
| Upstream source capability check | Passed: 50 records, 128 source capabilities/debugger wire layouts |
| Full suites and strict Clippy after the final readiness edit | Deferred at the stopping request |
| Native Linux dmdoc build/smoke | Not run |
| Hosted native Ubuntu/macOS, full-project docs, transport cancellation and abrupt server shutdown | Not qualified in this batch |

The Linux full-suite failure is retained. The final focused run passed both marker cases but failed `readiness_timeout_retains_progress_samples_for_a_busy_process`: fewer than two progress samples were retained. Its cause is not established, and there was no retry after this failure. Linux Clippy was skipped after that failure. Mutation checks were not rerun in this batch. No speedup or model-token saving was measured.

The frozen Windows MCP candidate is `target/docs-preservation-native-windows/mcp.exe`, SHA-256 `cb21ec46f925f5af5824c48aaefefc304f38d1a82911452b5ea82c3f06afc1e5`. It predates only the subsequent test/handoff edits. The smoke used the helper manifest at `target/native-memory-release-final-package/helpers/manifest.json`: dmdoc revision `351ddc0ffb2439876d4565ce5130bb6b027ee605`, SHA-256 `0c6f8733550163e38f3dc5382f527113c27c912e65a1e3c169fa5cc7f944ff68`. Do not overwrite or relabel this candidate as a later build.

[verification.json](verification.json) records current source hashes, raw log hashes, filesystem results and the corrected timing probe. Raw logs and binaries remain under ignored `target/`; preserve them when resuming.

## Linux filesystem boundary

The 16-case probe succeeded with `RENAME_NOREPLACE` on native Linux temporary storage and preserved existing file/directory destinations with `EEXIST (17)`. On this checkout's Windows-drive mount, absent destinations produced `EINVAL (22)`; existing destinations produced `EEXIST (17)`. Ordinary renames could replace destinations on both filesystems.

The repair rejects an unsupported output filesystem before helper execution. It does **not** add installation support to that filesystem or establish WSL 1 compatibility. Do not replace the no-replace primitive with ordinary rename. The September 6 WSL 1 kernel result remains a separate historical observation.

## Readiness failure diagnosis

The original tests included child interpreter/cmdlet initialization in a three-second readiness window. A corrected probe launched three independent harnesses concurrently and passed the exited launcher, rather than the marker writer, to the relevant wait.

The two live writers produced markers about 2.53 and 2.56 seconds after launch. The exited-launcher case produced its marker about **3.22 seconds** after writer launch; its last recorded poll at 2.990 seconds saw no marker, and the wait returned `process_exited` at 3.242 seconds. The immediate post-result observation found the marker and a live writer. This reproduces the launcher failure and supports separating fixture initialization from timed readiness. It does not directly reproduce the live test failure or explain the older intermittent WSL 1 guardian timeout.

Earlier instrumented probes used different sequencing/process selection and are not matched evidence. Their logs remain retained with that limitation. The final fixtures use a bounded bootstrap/release handshake, verify actual `READY` contents and retain ownership of writer termination.

## Resume in this order

1. Preserve the dirty patch and evidence. Diagnose the final Linux busy-process timeout sample failure with a matched probe that records the actual result and initialization/poll timing. Do not assume it shares the marker-writer cause or relax its assertions without evidence. Review `tests/process_readiness.rs` and the documentation changes with the failed and corrected logs.
2. Requalify the final source on Windows in a Visual Studio developer shell and on Linux with native temporary storage. Run each platform without concurrent heavy builds:

   ```text
   cargo +1.95.0 test --locked --all-features --no-fail-fast
   cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
   cargo +1.95.0 fmt --all -- --check
   git -c core.safecrlf=false diff --check
   ```

   On this WSL checkout use `TMPDIR=/tmp`, `CARGO_TARGET_DIR=target/linux`, and add `target/linux-tools/pwsh-7.6.5` to `PATH`. Keep new logs distinct from the retained failed full run.
3. Select the ignored `unsupported_filesystem_is_rejected_before_execution` test on the mounted filesystem. Prefer the freshly built `docs_generation` test executable with `--ignored --exact unsupported_filesystem_is_rejected_before_execution` and a mounted `TMPDIR`, so switching temporary storage does not rebuild Cargo dependencies. This is a separate rejection gate, not a native-success test.
4. Build native Linux dmdoc from the clean exact-pinned `integration/SpacemanDMM` checkout through `scripts/build-spacemandmm-helpers.ps1`. **Unset `CARGO_TARGET_DIR` for this builder**, which copies from the upstream checkout's `target/release`:

   ```powershell
   ./scripts/build-spacemandmm-helpers.ps1 -UpstreamPath ./integration/SpacemanDMM -OutputDirectory ./target/docs-preservation-linux-helper -ManifestPath ./target/docs-preservation-linux-helper/helpers/manifest.json
   ./scripts/test-spacemandmm-docs.ps1 -BinaryPath ./target/linux/debug/meridian-mcp -HelperManifestPath ./target/docs-preservation-linux-helper/helpers/manifest.json -FixtureParent /tmp
   ```

   Run these in native Linux PowerShell. Verify clean upstream source before building; freeze and hash the new MCP/helper candidates separately.
5. Record results, review and commit a coherent qualified repair. Deployment, installed-server verification and hosted/live acceptance are separate steps.
6. Resume the broader tool inventory only when requested. Remaining documentation review includes stale snapshots, additional live dmdoc inputs and filesystem authority; the current overlap protection covers recorded parsed inputs.

No further tool family or native helper build was started for this stopping request.
