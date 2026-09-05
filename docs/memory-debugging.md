# Memory investigation

`dm_memory_summary` and `dm_memory_compare` analyze memory samples already saved by Tracy captures and completed experiments. They work in analysis mode, require no parse or native helper, and never start a process or change an artifact.

## Find growth and peaks

Pass the path of a schema-2 `.tracy.meridian.json` sidecar or completed experiment JSON:

```json
{
  "evidence_path": "<authorized-root>/experiment/steady.tracy.meridian.json",
  "begin_ms": 10000,
  "end_ms": 40000,
  "sample_limit": 20
}
```

Offsets are milliseconds from experiment launch, not from the start of the trace. The interval includes `begin_ms` and excludes `end_ms`. Omit the bounds to use all samples in the document, including any samples retained just outside a capture for context. No boundary values are interpolated.

Each process and metric gets its own start/end usage, minimum, sampled peak, peak time, net change, net bytes per second, largest adjacent rise/fall, and observed span. Windows working set, private bytes and virtual bytes remain distinct; Linux RSS and virtual bytes remain distinct. They must not be added together or compared as equivalent measurements.

Statistics use every selected sample. `sample_limit` controls only the first returned samples per metric (default 20, maximum 100, zero omits samples). `samples_truncated` indicates omitted samples. Empty windows return no metric statistics; one sample has no growth rate. An interval longer than twice the nominal sampling interval is counted as a gap. `missed_samples` is the original series-wide counter, not an estimate for the selected window.

Growth is an observation, not a leak diagnosis. Peaks between samples can be missed, and memory retained by an allocator is not necessarily retained by live DM objects. Allocation sites, object sizes and reference chains are not derived from process totals.

## Compare windows

```json
{
  "baseline": {
    "evidence_path": "<authorized-root>/experiment/first.tracy.meridian.json",
    "begin_ms": 10000,
    "end_ms": 40000
  },
  "current": {
    "evidence_path": "<authorized-root>/experiment/second.tracy.meridian.json",
    "begin_ms": 40000,
    "end_ms": 70000
  }
}
```

The tool rereads both documents and requires matching recorded executable/workload objects, MCP build ID, process roles, OS, metric kinds and nominal sampling intervals. PIDs may differ between runs. Phase labels must match unless `allow_different_phases: true` explicitly requests a descriptive phase comparison. Unequal observed spans are reported; results are not a performance-improvement verdict.

Deltas are current minus baseline. `net_change_delta_bytes` is a signed decimal string to preserve the full possible difference between two signed byte changes. Other byte deltas are signed numbers. Missing rates remain null.

Each result hashes the input document. `identity_verification: recorded_not_verified` means the identity came from that document; it is not independently proved against a managed build, signed, or authenticated by the trace. The tools do not follow paths inside evidence, load raw traces, or expose the full recorded identity. They accept at most 16 MiB, four unique process roles and 100,000 samples per document, and reject non-monotonic metric timestamps and unsupported schemas/units.

## Deeper debugging

### Native allocation attribution (experimental)

With the optional memory helper installed, launch an owned debugger using `dm_debug_launch` with `memory_profile: true`. This initially supports **Windows BYOND 516.1687**. The normal debugger keeps its original pinned DLL.

1. Call `dm_debug_memory` with `{"action":"status"}` to check availability.
2. Start with `{"action":"start","duration_ms":10000,"max_records":20000}`.
3. Exercise the selected workload, or evaluate a known test procedure.
4. Stop with `{"action":"stop","row_limit":100}` to retrieve the report.

The helper observes UCRT malloc/calloc/realloc/free on the VM thread. It groups currently outstanding **requested allocation bytes** by DM procedure and reports the observed peak and successful allocation/resize-call counts. It preserves the old allocation after failed realloc. Preexisting blocks are ignored until an observed realloc, which tracks the resulting requested size. Resizes use the current procedure when available, otherwise preserving prior attribution. Successful allocations without a DM procedure or prior attribution are counted separately and their bytes are excluded.

Captures are limited to 60 seconds and 100,000 outstanding records. Accounting stops at the deadline or record limit; call `stop` to retrieve and clear the pending capture before starting again. A record-limit result is incomplete and frozen at that point. At most 1,000 procedure rows are returned, with truncation indicated. Results include the helper hash and launch provenance; they are returned directly, without a native output path. An empty report does not establish allocation coverage.

Other threads, frees performed on other threads, custom allocators, VM pool contents, object identities and retaining references are outside coverage. Requested bytes exclude allocator overhead. Debugger activity can affect the workload; a procedure's remaining allocations can include caches. These results cannot prove a leak or an object's retained size. Hooks keep an inactive fast path after capture and remain installed until the owned process exits. Treat this as diagnostic instrumentation, not a performance benchmark.

Build the optional package from [Auxtools revision 889006e3](https://github.com/willox/auxtools/tree/889006e334570a426f35c0a2f579c08d3d7b2186):

```powershell
./scripts/build-auxtools-memory.ps1 -SourceRoot <auxtools-checkout> -BuildRoot ./target/aux-memory-build -OutputDirectory ./target/aux-memory-package
```

This requires Rust 1.95.0 with `i686-pc-windows-msvc` and Visual Studio C++ build tools. Pass `-MemoryHelperDirectory ./target/aux-memory-package` to the maintained installer alongside its normal arguments. The resulting `helpers/auxtools-memory/manifest.json` verifies the DLL, exact upstream revision and all maintained overlays before a memory-enabled launch. Restart the MCP client after changing its installed binary. See [native qualification](../TESTING.md).

### Object lifetime and remaining gaps

Use process growth to select a workload and interval, then use source inspection, debugger variables and project-owned lifecycle counters to investigate a suspected cause. Object counts must be labeled as counts; they do not establish allocated or retained bytes. Avoid holding extra object references just to count them, because that can change their lifetime.

The pinned byond-tracy hook emits procedure zones and frame events; its current event representation has no allocation/free records. Tracy itself has allocation instrumentation, but allocation queries alone cannot recover events that the hook never emitted.

The original Auxtools v2.3.7 Windows profiler hooks `msvcr120.dll` and silently returns if it is missing. Its allocation table has no size cap. The maintained optional helper replaces that implementation with explicit capability checks and bounded UCRT recording.

Sources: [pinned byond-tracy hook](https://github.com/spacestation13/byond-tracy/blob/d1ec404737b04b1ea73d6df4a1b477deacdb1900/prof.c), [pinned Tracy allocation API](https://github.com/wolfpld/tracy/blob/099df3de3dc37eca4712c06b8320fb9c53596edd/public/tracy/Tracy.hpp), [auxtools memory profiler](https://github.com/willox/auxtools/blob/v2.3.7/debug_server/src/mem_profiler.rs).

An owned Windows BYOND 516.1687 probe confirmed that UCRT was loaded and `msvcr120.dll` was absent. The original profiler acknowledged capture but wrote an empty report. The replacement recorded nonempty procedure attribution for the same allocation workload. Container removal still does not prove object destruction; the owned native fixture separately tests known allocation/reallocation/free behavior.

`dm_debug_evaluate` accepts DM expressions and rejects debugger console commands beginning with `#`. Use the explicit MCP controls for profiling.

If a debugger request times out or is cancelled, stop and relaunch the debugger. The transport rejects subsequent requests because upstream replies have no request IDs. Ordinary event-wait timeouts preserve partially received frames. Stopping sends the upstream disconnect command without waiting for an acknowledgement it does not provide.

The [implementation and investigation record](superpowers/plans/2026-09-05-memory-investigation.md) separates delivered tools from native feasibility and remaining gates.
