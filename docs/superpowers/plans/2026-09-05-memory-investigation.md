# Memory investigation implementation and qualification

**Goal:** Make recorded process-memory growth usable through MCP and establish a source-backed path to allocation and retention diagnosis.

**Design:** Extend the existing evidence workflow with two analysis-mode tools. Read explicitly selected schema-2 JSON through startup path policy, validate bounded samples, and calculate descriptive statistics without changing runtime collection or native dependencies. Keep recorded identity compatibility distinct from verified build provenance. Investigate lifecycle counters and allocation hooks separately before exposing new runtime operations.

**Constraints:** Rust 1.95.0; preserve unrelated work; no commits. Process metrics, object counts, allocation bytes and retaining references are different evidence. No full-game memory or performance claim follows from owned fixtures. The first phase left installation unchanged; the approved continuation adds a separately verified, opt-in native helper and prepares a tested package.

## Native continuation

Keep the ordinary v2.3.7 debugger unchanged. Build an optional helper from exact Auxtools revision `889006e334570a426f35c0a2f579c08d3d7b2186` with maintained source overlays and a hash-verified manifest. Initially support only Windows BYOND 516.1687, the runtime being qualified.

Add `memory_profile: true` to debugger launch and `dm_debug_memory` with status/start/stop actions. Return bounded JSON directly through the existing debugger transport; accept no output path or arbitrary console command. Start requires allocator capability checks. Record only allocations attributed to DM procedures on the VM thread, and observe frees on that thread. Report requested bytes, missing coverage, observed calls, recording limits and stop reason. This is allocation attribution, not a VM heap census or retaining-reference graph.

Use UCRT malloc/calloc/realloc/free hooks, a reentrancy guard, preallocated bounded tracking, a maximum 60-second capture and a capped report. A failed realloc must preserve the old allocation. Freeze and mark incomplete at capacity; ignore preexisting and unattributed allocations. Persistent detours have an inactive fast path and remain owned until process exit to avoid racing other allocator threads during unhook.

- [x] Confirm allocator mismatch in the owned live runtime: UCRT present, VC2013 CRT absent.
- [x] Test accounting: allocation/free, preexisting pointers, failed/in-place/moved/zero-size realloc, limits and deadline.
- [x] Build and qualify the optional helper against known retained/released allocations and repeated capture/stop.
- [x] Add explicit MCP controls, capability failure, expression-console guard, helper identity checks and protocol tests.
- [x] Complete focused/full Rust checks and native stdio qualification; prepare installation artifacts and a restart handoff with remaining gates.

## Process-memory tools

Files: `src/memory_evidence.rs`, `src/tools/memory.rs`, `src/tools/mod.rs`, `src/contracts.rs`, `spacemandmm-capabilities.json`, `src/lib.rs`, `tests/memory_tools.rs`, generated `docs/tool-contracts.md`, README and testing guidance.

- [x] Reproduce missing summary/comparison behavior through `call_tool` tests.
- [x] Add `dm_memory_summary` for half-open windows, per-role metrics, sampled peaks, net changes/rates, adjacent changes and sampling gaps.
- [x] Add `dm_memory_compare` with complete recorded executable/workload equality, OS/role/metric checks, explicit phase opt-in and honest missing/unequal-span results.
- [x] Reject malformed timestamps, duplicate roles, unknown request fields, unsupported metrics, oversized files and paths outside authorized roots.
- [x] Complete format, strict Clippy, full Rust tests and fresh stdio smoke.
- [x] Read retained real capture/experiment evidence with the new binary: 492 samples per metric for each of two process roles, and six compared metrics; stdio exit 0.

## DM lifecycle evidence

Use project-owned lifecycle counters and existing `dm_topic` / debugger inspection rather than enumerating every datum or storing strong references in the MCP. A selected-type fixture must create a known population, release it and report creation/removal/remaining counts. List membership is not object destruction; a held reference, cycle and queued deletion need distinct cases before claiming lifecycle coverage. Generic JSONL evidence tools can already summarize numeric counters, so a new ingestion format is unnecessary.

- [x] Qualify the owned allocation/release probe with the pinned BYOND compiler and debugger.
- [x] Record whether counts establish only removal from a known container or actual deletion.

## Allocation attribution feasibility

The pinned byond-tracy source represents zone begin/end/color and frame events, not allocations. Extending it would require allocator hooks, allocation/free events, capture-boundary semantics, bounded state, new helper queries, exact pin/patch manifests and native/live gates.

Auxtools v2.3.7 already contains a Windows profiler behind `#mem_profiler begin/end`. Source inspection found a hard-coded `msvcr120.dll`, single-thread filtering, no recording bound, silent hook-setup failure and procedure-attributed outstanding bytes. This is a narrower potential integration than adding equivalent hooks to byond-tracy, but not ready for general exposure.

- [x] Run a disposable known-allocation probe and retain its report, BYOND/helper identities and shutdown evidence.
- [x] Determine whether the pinned BYOND runtime provides nonempty attributed allocation evidence.
- [x] Specify capability and correctness gates before choosing a maintained native extension: hook availability; malloc/free/realloc success and failure; preexisting allocations; per-thread coverage; maximum records; cleanup; output containment; ownership and timeout behavior.

## Retention analysis

Debugger variables expose reachable values from selected frames/objects. They do not prove all incoming references, global roots, exact retained sizes or heap fragmentation. Retained-byte attribution needs a VM-aware object graph with stable identities and coverage guarantees; allocation stacks alone cannot supply it.

Do not advertise full heap snapshots or retaining-reference paths without an independently testable BYOND interface. The next feasibility fixture must distinguish a container-held object, an unreferenced object, a cycle and a deleted object without the observer extending their lifetimes. Native allocator evidence and DM object identities must be correlated before assigning bytes to types.

## Owned native probe result

On Windows with BYOND 516.1687 and the unchanged pinned auxtools DLL, the disposable fixture compiled with zero errors/warnings. `memory_allocate()` returned 10,000 held list entries and `memory_release()` returned zero. The native memory profiler acknowledged begin/end but produced an empty report. The debugger stopped and MCP exited with code 0. Raw responses, source and compiler artifacts are retained locally under ignored `target/memory-feasibility/`. An initial quoted-path request failed because the upstream console splits on whitespace; the corrected request used an unquoted contained path. Neither response proves usable allocation attribution.

The probe used installed release `da29e984` to investigate existing native behavior; it does not qualify the newly built memory-analysis tools. Counts prove only container membership. Live allocation coverage failed, and heap/reference graph coverage remains unimplemented. Before adding a public native capture tool, repair and qualify hook detection/accounting against this exact BYOND version and replace unrestricted console forwarding with an explicit contained operation.

`dumpbin /dependents` on the tested `byondcore.dll` found `MSVCP140.dll`, `VCRUNTIME140.dll` and UCRT heap interfaces. This supports the mismatch with the auxtools profiler's hard-coded VC2013 runtime target. Adapting the profiler requires native work and a new verified helper package; no dependency or installed helper was changed here.

The initial full-suite run hit the existing natural-exit ownership test's three-second deadline. It passed unchanged in isolation (0.26 s) and in the four-thread rerun. That rerun then caught missing capability-registry entries for the two new tools; those mappings were added before final qualification.

## First-layer local qualification (before native continuation)

- Rust `1.95.0 (59807616e 2026-04-14)`, Windows x86_64 MSVC.
- `cargo +1.95.0 test --locked --offline --all-features --no-fail-fast -- --test-threads=4`: 395 passed, four intentionally ignored, zero failed.
- `cargo +1.95.0 clippy --locked --offline --all-targets --all-features -- -D warnings` and `cargo +1.95.0 fmt --all -- --check`: passed.
- Capability registry, approved-tool list, exact mode inventories and generated contracts include both memory tools. Registry audit passed for 46 records.
- Fresh debug-binary stdio: analysis inventory 26 tools with owned parse, cached diagnostics and search; development inventory 35 tools; both exited 0.
- Real retained experiment JSON: both process roles and all six metrics summarized and compared through stdio, exit 0. Raw evidence remains under ignored `target/memory-stdio.json`.

At the end of the first phase, the process-memory implementation was qualified locally; Linux/hosted execution, release packaging, installation and post-restart exposure were unrun. The approved native continuation above supersedes that earlier scope. The original failed Auxtools probe remains failure evidence, separate from the replacement helper's qualification. Complete retention analysis remains future work.

## Native continuation qualification

- Clean pinned-source export and optional helper release build passed with Rust 1.95.0, x86 MSVC. The manifest records exact source/overlay hashes; DLL SHA-256 is `f1fdad756b83eb1e2b1cd0d47198640639adfd3057b564c818671e20bd004639`.
- Strict Clippy, formatting, the 47-record capability audit and the final full Windows Rust suite passed: **405 passed, four intentionally ignored, zero failed**.
- Fresh debug MCP plus owned BYOND 516.1687 and C fixtures passed the maintained native integration script. The C workload holds 528,384 requested bytes; its procedure reported 529,076 observed requested bytes in total, and zero outstanding bytes after release. Failed realloc preserved the old block; zero-size realloc and free removed the blocks.
- Record-limit capture froze with an explicit incomplete result; the deadline stopped accounting. Repeated capture, rejected console commands, explicit opt-in and a subsequent ordinary debugger session passed. MCP exited 0. Raw evidence: ignored `target/native-memory-integration-debug.json`.
- Installer qualification preserved native patch and Tracy telemetry metadata, including entries without patches.
- Release qualification exposed an intermittent 30-second VM response delay followed by delayed replies being accepted for later requests. The isolated rerun passed; the response-association bug was then reproduced deterministically in a transport test. Requests now fail closed after timeout/cancellation, event-wait timeouts retain partial frames, and disconnect no longer waits for an acknowledgement upstream never sends. All six focused transport tests and the final full suite passed. The intermittent VM delay itself remains unexplained.
- The final release passed the maintained native fixture with the transport correction: 529,076 observed requested bytes attributed to the known-allocation procedure, zero after release, stdio exit 0 and no owned fixture processes remaining. Evidence: ignored `target/native-memory-integration-final-release.json`.
- Final release analysis/development stdio gates passed with 26/35 tools, owned parsing, diagnostics and search. Retained process-memory evidence passed with six compared metrics. Fresh startup of the installed release advertised all 62 configured tools and exited 0.
- Installed a separate release, retained the previous release, backed up Codex configuration and changed only the registered binary/helper paths. Post-restart tool exposure remains unrun until Codex restarts; exact identities and executable resume steps are in the handoff below.

Remaining platform/acceptance gates: hosted CI, non-Windows native support, additional BYOND builds, real-game overhead/coverage and complete object-retention analysis. Release package and restart identity are recorded in the [native memory handoff](../../audits/2026-09-05-native-memory-handoff.md).
