# Dependency policy

Direct dependencies use Cargo's lockfile and conservative version requirements. Git dependencies must be pinned to an exact revision in `Cargo.toml`; a floating branch is not releaseable.

Updating SpacemanDMM requires parser and diagnostic fixtures, DMM/TGM tests when relevant, a full Meridian-Rift parse and representative queries, performance comparison, refreshed license/advisory results, and the exact new revision in `Cargo.toml`, `Cargo.lock`, provenance, and compatibility documentation.

The approved integration baseline is SpacemanDMM revision `351ddc0ffb2439876d4565ce5130bb6b027ee605`, whose workspace MSRV is Rust 1.95. Meridian-MCP therefore pins Rust 1.95.0 in both local tooling and CI; use that exact compiler for dependency, Clippy, test, and release evidence.

Native profiler dependencies are pinned independently: Tracy `099df3de3dc37eca4712c06b8320fb9c53596edd` (v0.14.0), byond-tracy `d1ec404737b04b1ea73d6df4a1b477deacdb1900`, protocol 82, and the checked-in clock/empty-queue/health patches. Updating any one requires review of wire compatibility, raw-clock access, health-event layout, supported BYOND offsets and hook prologues, licenses, fixed-command APIs, native CTests on Windows and Ubuntu, patch/artifact hashes, and fresh live evidence for each claimed BYOND/platform pair. Do not substitute Tracy `master`, a release binary, or an unverified byond-tracy hook; matching protocol numbers alone do not prove compatibility.

Range-aware analysis relies on the pinned Tracy server's zone-occurrence, child-zone, frame-boundary, timer-multiplier, and base-time APIs. An upstream update must revalidate complete/partial classification, inclusive/self percentiles, half-open range conversion, trace reopen behavior, and byte-stable fixed-command output before the pin changes. Control statistics are Meridian-owned Rust calculations and must not inherit GUI defaults silently.

The byond-tracy build also requires the checked-in empty-queue initialization patch. The builder applies it to a private copied source file and fails if it no longer applies cleanly. Review or remove the patch explicitly when changing the upstream hook revision; never carry it forward by fuzzy manual editing.

Windows BYOND 516.1687 runtime provisioning pins the official NuGet package `Microsoft.DXSDK.D3DX` version `9.29.952.8` at SHA-256 `ead0906ae8a26c18a7525da7490127a2110f7c58f18293738283e30e97c6ea4b`. CI extracts only the unmodified x86 release `D3DX9_43.dll` application-local and retains `LICENSE.txt` and `NOTICE.md`. Changing the package, version, hash, architecture, DLL selection, or placement requires loader testing on a clean Windows runner and a provenance update. Never replace the package with a system-wide DirectX installer or download it during Meridian-MCP startup. The same preflight verifies x86 `MSVCP140.dll`, `VCRUNTIME140.dll`, and `mfc140u.dll`; a missing component is installed only through a valid Microsoft-signed x86 VC redistributable.

Run `cargo update` only for an intentional dependency change. Review the lockfile diff, then run formatting, clippy, all-feature tests, release build, and `cargo deny check`. Advisory exceptions must be narrow, documented, time-bounded, and later removed.

The optional Windows allocation helper is built from Auxtools revision `889006e334570a426f35c0a2f579c08d3d7b2186` (v2.3.7), its committed lockfile and the maintained files under `helpers/auxtools-memory/`. Its separate manifest must match the DLL hash, revision, protocol 1 and every overlay hash embedded in the MCP binary. It initially permits only BYOND 516.1687. Changes require bounded accounting tests, a clean pinned-source helper build and the owned native memory integration gate. The normal debugger retains its original release DLL pin.

Auxtools has a root MIT license, while the pinned upstream README and `debug_server/LICENSE` declare the debugger component GPLv3. Its inherited Cargo license declaration does not resolve that distinction. Both debugger packages retain the root license, component license and upstream README; neither debugger DLL is described as MIT-only. Review the component declarations and corresponding-source requirements before binary distribution.

The maintained builders stage SpacemanDMM, Tracy, byond-tracy and bundled native dependency notices with their helper artifacts. Installation checks required notices before replacing package files, rejects conflicting copies, and copies them into `helpers/licenses`; the optional memory helper also retains its notices beside its DLL. A manifest may be outside the package when its helper paths still resolve to the maintained `helpers/bin/<platform>` layout. These checks preserve the selected notices; they do not establish complete Rust/helper dependency attribution, source availability or distribution qualification. Review those requirements for the exact binary, helper manifests and source revisions being released.

## Temporary advisory exceptions

The following RustSec entries report unmaintained transitive crates, not known vulnerabilities. They
remain visible as `cargo deny` notes and expire for review on 2026-11-23.

| Advisory | Dependency path | Removal condition |
| --- | --- | --- |
| `RUSTSEC-2024-0370` | SpacemanDMM `dreammaker` -> `get-size-derive` -> `proc-macro-error` | Remove when the pinned parser graph no longer uses `proc-macro-error`. |
| `RUSTSEC-2024-0388` | SpacemanDMM `dreammaker` -> `derivative` | Remove when SpacemanDMM replaces `derivative`. |
| `RUSTSEC-2024-0425` | SpacemanDMM `dreammaker` -> `get-size` | Remove when SpacemanDMM replaces `get-size`. |
| `RUSTSEC-2024-0427` | SpacemanDMM `dreammaker` -> `get-size-derive` | Remove when SpacemanDMM replaces `get-size-derive`. |
| `RUSTSEC-2024-0436` | `image` -> `exr` -> `pulp` -> `paste` | Remove when a compatible image graph no longer uses `paste`. |
| `RUSTSEC-2025-0141` | Meridian-MCP debugger protocol -> `bincode 1.3.3` | Remove only when the pinned auxtools wire protocol adopts a maintained, byte-compatible codec. |

Do not renew an exception without checking the current SpacemanDMM revision, compatible direct
dependency releases, and the advisory database. A vulnerability or unsoundness advisory is not
covered by these maintenance-only exceptions.

SpacemanDMM's root license at the pinned revision is detected as GPL-3.0-or-later. The cargo-deny
clarifications, including the `dmi` crate which omits a manifest license field, are hash-locked to that file so a source-license change fails closed. Binary distributors
must evaluate obligations for the complete graph. This is not legal advice.

The baseline also carries the locally vendored `meridian-read-policy-v4` actual-open hooks for dreammaker and dmm-tools. Review the [complete local delta and license](../vendor/spacemandmm/README.md) independently of the unchanged upstream revision. The capability audit verifies its declared delta hash. Helper builds and CI upstream checkout pins remain at the exact baseline; they are not claims that those separate helper executables enforce these in-process hooks.

`serde_path_to_error` 0.1.20 (MIT OR Apache-2.0) is used only at the strict request boundary to retain nested field paths. Raw Serde messages and caller values are not returned; bounded field paths and fixed recovery text form the diagnostic. The SDK and all parser/helper revisions remain pinned as above.
