# Documentation repair: stopping checkpoint

## Latest qualification: September 10

The [September 10 qualification](../2026-09-10-docs-qualification/README.md) supersedes the current-state guidance below. It records repaired nested-source/link overwrites, early unsupported-filesystem rejection, a final readiness sample at timeout, full Windows/Linux qualification and native dmdoc checks. The installed MCP is unchanged. The [September 9 checkpoint](../2026-09-09-docs-preservation/README.md), entries below and their JSON artifacts remain historical evidence.

## Status update: 2026-09-09

Stopped at the user's renewed request. The documentation patch is now committed in `9c599fef8a23b7058b2f392b60815d07afa10f82` (`sprint done`), and the worktree was clean before this handoff update. All eight implementation/test file hashes in [verification.json](verification.json) still match after LF normalization. No implementation changes or new test runs were made for this update; the results below remain the September 6 evidence.

The Linux/WSL 1 installation failure remains unresolved in the current source. The recorded focused Windows checks and pinned dmdoc smoke passed, but this patch has no complete cross-platform qualification. `f84a1bf` remains the last batch with full local Windows and Linux/WSL test-and-Clippy qualification. This checkpoint does not replace the installed MCP or require a restart.

A read-only review also identified two source-level concerns to reproduce before release:

- The output-overlap check in `src/tools/docs.rs` protects directories containing the environment file or helper, but does not check every parsed source input. Test an existing nested source directory selected as the output with `overwrite=true`.
- `PathPolicy::output_path` resolves an existing output to its canonical target before documentation target validation. Test an output symlink to determine whether its original identity must be rejected before canonicalization.

Resume with these preservation fixtures and the Linux no-replace compatibility issue, then run the focused and full gates listed below. Keep late-created destinations and recoverable previous documentation intact. The broader inventory remains open in the [workplan](../2026-09-06-functional-performance-followup.md).

## Historical checkpoint: 2026-09-06

The remainder of this report and its JSON artifacts preserve the original stopping state. References below to an uncommitted patch or committing next describe that earlier state, superseded by the update above.

Work stopped at the user's request to find a good stopping point. The documentation patch is **uncommitted and incomplete**. `f84a1bf` is the last committed batch with full local Windows and Linux/WSL test-and-Clippy qualification. The installed MCP has not been replaced, and no restart is needed for it.

## Current patch

Purpose-written fixtures reproduced:

- Temporary-directory leaks after a late output collision and cancellation, including a helper holding a Windows file handle without delete sharing.
- An existing output file being replaced before an error was returned. Using the project directory or workspace root as the output could remove fixture source files.
- Malformed `overwrite` and unknown arguments being accepted.
- A 2,098,234-byte tool payload, above the documentation tool's 262,144-byte transport ceiling. The success metadata also overwrote the output-truncation flag.

The patch adds staging ownership with bounded cancellation cleanup, validates output type/parent/source overlap before execution, reports installation and retained-backup state separately, bounds stdout/stderr replies, and exposes `include_output`/`output_max_bytes`. The declared timeout now matches the existing 600-second helper timeout.

Replacement and restoration must preserve a destination created after preflight, even if it is empty. Focused tests disproved the assumption that `std::fs::rename` preserves an existing empty directory on Windows. The Windows implementation now uses `MoveFileExW` without replacement. Tests cover successful restoration, a restoration collision, and backup cleanup failure after the new documentation is already installed.

## Evidence at the stopping point

| Gate | Result |
| --- | --- |
| Windows documentation/compiler/Rift response integration tests | 21 passed |
| Windows directory installation tests | 4 passed |
| Windows shared response-budget unit tests | 3 passed |
| Windows strict all-target/all-feature Clippy | Passed with Rust 1.95.0 |
| Real pinned Windows dmdoc helper | Passed: generated HTML, configured index, type/member documentation, source preservation, initial overwrite rejection and natural MCP exit |
| Linux/WSL focused integration tests | 14 passed, **2 failed**, both documentation installation cases |
| Linux installation/response unit tests and Clippy | Not run after the focused failure |
| Full suites for this documentation patch | Not run |
| Hosted native Linux/macOS, full project docs, transport cancellation and abrupt server shutdown | Not qualified in this batch |

The real helper is pinned to SpacemanDMM `351ddc0ffb2439876d4565ce5130bb6b027ee605`, SHA-256 `0c6f8733550163e38f3dc5382f527113c27c912e65a1e3c169fa5cc7f944ff68`. The frozen Windows candidate is under `target/docs-checkpoint-native-candidate/mcp.exe`; its hash and the relevant source/log hashes are in [verification.json](verification.json).

The initial red integration run had two passing and five failing tests. A strengthened overlap fixture then exercised all three unsafe target choices before asserting. An initial Rust ownership error was corrected before executing candidate tests. The first installation unit run passed two tests and failed two empty-directory collision tests; the Windows primitive correction resolved those failures. Failed logs remain under ignored `target/`.

## Remaining issue before committing

This local Linux environment is WSL 1, kernel `4.4.0-19041-Microsoft`. A controlled `renameat2` probe with explicit ctypes argument types found that flags-zero renames succeed for files and directories, while `RENAME_NOREPLACE` returns **EINVAL (22)** for both. Source entries remain present and destinations remain absent in the rejected calls. See [linux-rename.json](linux-rename.json).

The current Linux branch therefore rejects valid documentation installations in this environment. Resolve that compatibility issue before committing or installing this patch. An ordinary overwrite-capable rename is not an acceptable fallback: it would violate the late-collision and restoration invariants just added. Native Ubuntu and macOS behavior has not been verified here.

## Resume

Use a Visual Studio developer shell on Windows and the repository's pinned Rust 1.95.0. Preserve this uncommitted patch and the frozen evidence.

1. Resolve Linux directory-install compatibility while preserving late destinations and recoverable old documentation. Reproduce the kernel/filesystem behavior directly before choosing an implementation.
2. Repeat the focused documentation and shared-response tests on Windows and Linux:

```powershell
cargo +1.95.0 test --locked --all-features --test docs_generation --test compiler_responses --test rift_output --no-fail-fast
cargo +1.95.0 test --locked --all-features --lib tools::docs --no-fail-fast
cargo +1.95.0 test --locked --all-features --lib tools::build_response --no-fail-fast
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
./scripts/test-spacemandmm-docs.ps1 -BinaryPath target/debug/meridian-mcp.exe -HelperManifestPath target/native-memory-release-final-package/helpers/manifest.json
```

3. Run the complete test suites before treating this as a fully qualified batch. Update user-facing documentation for the final output/cleanup contract, record fresh evidence, review and commit the coherent repair.
4. Resume the [broader workplan](../2026-09-06-functional-performance-followup.md). Remaining tool-family reviews, the earlier intermittent WSL guardian readiness timeout, hosted/live gates and release handoff are still open.

The broad goal is not complete. No further audit area was started after the stopping request.
