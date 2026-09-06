# Compatibility and evidence

- **Verified:** committed automated tests and a documented integration gate pass on a named platform and version.
- **Provisional:** useful automated coverage exists, but part of the integration matrix remains untested.
- **Experimental:** behavior may change or fail and is disabled by default when it crosses a security or protocol boundary.
- **Unsupported:** not exposed in normal operation or removed.

| Component | Target | Status | Evidence |
| --- | --- | --- | --- |
| Windows | Current host, 2026-08-24 | Verified for existing owned gates | Rust and installed stdio fixture evidence exists. New full-corpus analysis and `rift_compile` capabilities remain provisional until the scheduled workflow records a green run against exact repository SHAs. |
| Linux | Ubuntu 24.04 GitHub-hosted runner | Provisional | Per-change Rust and stdio gates plus independent over-64K parser and BYOND runtime jobs are configured. Promote each claim only from its own green hosted evidence. |
| macOS | Any | Unsupported | No test evidence. |
| Rust | 1.95.0 exact project toolchain | Verified | Declared in Cargo, `rust-toolchain.toml`, and CI. |
| BYOND | 516.1687 Meridian-Rift pin | Provisional | Ubuntu is the required synthetic over-64K DreamDaemon lane. Windows retains a visible diagnostic sentinel while the independent real Meridian-Rift and Tracy path remains required. |
| SpacemanDMM | `351ddc0ffb2439876d4565ce5130bb6b027ee605` | Provisional | A freshly built MCP/parser matrix must parse 65,537 declared fixture leaves and resolve the first, boundary, and last paths on Windows and Ubuntu. |
| Tracy native helper | Tracy `099df3de` (v0.14.0), protocol 82, x86_64 Windows | Provisional | Release helper, schema-2 protocol, query, strict validation, bounded transient handoff retry, explicit transition status, queue/hook health, failure-window disposition, rotation, and cancellation CTests pass locally on Windows; hosted Windows and Ubuntu results remain independent. Live BYOND evidence is still required. |
| byond-tracy hook | `d1ec4047`, x86, BYOND 516.1685-1687 | Experimental | Exact-revision x86 Windows artifact built and hash-manifested locally. A local BYOND 516.1687 technical fixture completed five compatible 30-second controls; hosted live evidence remains required. |
| MCP transport | `rmcp` 3.1.3 | Verified | Official SDK tests and installed stdio negotiation/tool smoke passed. |

Update this table only from fresh, reproducible evidence. Never infer platform support from another operating system.

For the upstream feature mapping, repaired inheritance behavior, and explicit exclusions, see the [SpacemanDMM support audit](audits/2026-09-06-spacemandmm-support.md). Registry coverage is not a claim of full editor parity or completed platform qualification.

## Named Meridian-Rift gate

The scheduled/manual workflow keeps three claims independent. `windows-meridian-compatibility` runs on the explicitly pinned Windows Server 2022 image, drives the release binary through stdio MCP, parses the real `tgstation.dme`, runs the versioned lookup/definition/search manifest, records direct and full-build artifacts, then runs the small owned auxtools protocol fixture and owned Tracy live fixture. The auxtools gate does not boot Meridian-Rift because full-game initialization time is not debugger wire compatibility; the preceding real-repository gates still prove Meridian-Rift compilation and analysis. The fixed image remains part of the live-hook compatibility baseline. `prototype-parser-compatibility` proves that the freshly built MCP/SpacemanDMM stack parses 65,537 declared fixture leaves and resolves the first, boundary, and last paths on Windows and Ubuntu. `prototype-runtime-compatibility` starts compact 50,000-leaf control and 65,537-leaf boundary worlds under BYOND 516.1687. A synthetic failure cannot skip the Windows product gates.

Ubuntu is the required synthetic DreamDaemon engine lane. The Windows synthetic entry remains a visible diagnostic until three consecutive scheduled or manual runs pass; a diagnostic failure is never reported as Windows compatibility. The real Windows Meridian-Rift Tracy launch remains required. `rift_compile` must produce fresh artifacts in both `network_mode=allow` and forced `network_mode=offline`, with a successful warm human `BUILD.cmd` between them. Endpoint observations remain best-effort and always state `capture_complete: false`.

The portable Ubuntu job still does not install BYOND or attempt `RIFT_BUILD.cmd`. BYOND installation is confined to the separate synthetic runtime and live Tracy jobs; neither supplies Meridian-Rift full-build evidence.

The portable Windows/Ubuntu Rust matrix names repository-root policy, canonical proc ownership, build provenance, standard runtime integrity, and native-evidence reader/statistics tests before the complete all-feature suite. This is portable fixture evidence only. The Windows BYOND job separately runs `Run managed provenance and integrity gate` against BYOND 516.1687 and uploads its bounded schema-1 document even when the gate fails. Compatibility status is promoted only after the corresponding hosted artifact is green; adding the workflow is not itself a verified platform result.

Source declarations are not an authoritative count of every internal BYOND prototype. The parser fixture uses one compact parent. The runtime fixture groups leaves into 256-child buckets so it tests more than 65,536 total declarations without instead tripping DreamMaker's direct-child ceiling. Evidence records declared leaves and parents separately. Parser success does not prove DreamDaemon startup, DreamDaemon startup does not prove SpacemanDMM parsing, and a diagnostic Windows sentinel failure does not become a compatibility pass.

The boundary comes from the official [BYOND 516 release notes](https://www.byond.com/docs/notes/516.html), and the synthetic command uses the documented [DreamDaemon `startup()` options](https://www.byond.com/docs/ref/info.html#/proc/startup). Ubuntu is the required engine lane because `/tg/station` runs its [integration workflow](https://github.com/tgstation/tgstation/blob/master/.github/workflows/run_integration_tests.yml) on Ubuntu 24.04 and invokes DreamDaemon through its direct [server runner](https://github.com/tgstation/tgstation/blob/master/tools/ci/run_server.sh).

## Deferred verification matrix

| Capability | Owned fixture | Named-platform/real-repository gate | Required semantic evidence | Current blocker | Status |
| --- | --- | --- | --- | --- | --- |
| DreamChecker | Language fixture with known diagnostics | Real Meridian-Rift diagnostic corpus on Windows and Ubuntu | Stable structured severities, files, and messages without treating SpacemanDMM as DreamMaker | No versioned full-corpus diagnostic assertions | Provisional |
| Map inspection | DMM fixture with known dimensions and coordinates | Representative Meridian-Rift DMM/TGM files on Windows and Ubuntu | Exact format, dimensions, descendant matches, and coordinates | No versioned real-map corpus | Provisional |
| PNG rendering | Owned DMM/DMI render fixture | Named Windows render gate using parsed real metadata | Valid PNG, expected dimensions, and non-transparent semantic pixels | No real-repository render artifact assertion | Provisional |
| DreamDaemon lifecycle | Owned runtime fixture | Required Ubuntu over-64K engine lane, diagnostic Windows sentinel, and required real Windows Tracy launch | Readiness marker, bounded output, process progress, classification, and verified owned-tree stop | Fresh hosted-runner evidence is required | Provisional |
| `Topic()` | Owned loopback ping/pong handler | BYOND 516.1687 Windows runtime gate | Exact decoded response over server-owned loopback DreamDaemon | Depends on the named lifecycle gate | Provisional |
| Tracy fixed helper | Native protocol/query/validation/session fixtures | Windows and Ubuntu native build jobs | Bounded multiplexed envelopes, deterministic queries, raw-clock/trace validation, drain/capture rotation, exact revisions and ordered patches, dual architecture, and packaged licenses | Fresh hosted runs not yet recorded | Provisional |
| Tracy live capture | Technical BYOND fixture using repository-supported `-params tracy` | BYOND 516.1687 Windows, then Ubuntu independently | Prepared hook hash, immediate post-launch capture, delayed three-capture sequence, positive clocks, complete frames/zones, queue/hook health, exact MCP build identity, diagnostic-only invalid artifacts, finalized integrity journal, artifact persistence, and clean stop | The strengthened gate is implemented; it has not yet produced fresh hosted evidence. Prior local Windows evidence predates this complete contract. | Experimental |
| Tracy offline analysis | Native query fixtures and malformed-trace errors | Packaged helper on Windows and Ubuntu | Hotspots, zone aggregates, frame percentiles, comparison identity, deterministic truncation, optional source correlation | No checked-in representative trace fixture because binary trace stability/licensing must be reviewed | Provisional |
| Tracy experiment controls | Deterministic identity, range, memory-role, network-honesty, and noise fixtures | Five 30-second BYOND 516.1687 controls on each claimed live host | Same immutable experiment/phase, complete-only statistics, separate role memory, fixed noise envelope, redacted summaries, and raw traces retained locally | Local Windows 5x30 acceptance passed with 299 complete frames per trace and a non-noisy baseline; hosted platform evidence has not completed | Experimental |
