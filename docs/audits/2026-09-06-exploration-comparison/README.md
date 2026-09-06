# Code exploration comparison: Meridian-MCP and text search

**Decision: use text search first, with Meridian-MCP when parser knowledge helps. Keep the parser; remove the expectation that every exploration must begin with a parse and ranked search.**

In this sample, both approaches supplied the required source-backed facts after follow-up. MCP saved three operations across seven real questions, but returned almost twice as much text and required a substantial initial parse. Its strongest demonstrated benefits were semantic ownership and reference filtering, plus successful discovery in an unfamiliar subsystem. This supports selective use, with moderate confidence; it is not a blinded test of model productivity.

No MCP implementation, development-policy file, installation or registration was changed by this study.

## What was tested

Seven questions covered vending, item storage, airlocks, spells and hydroponics. Four small manufactured cases tested explicit parents, conditional compilation, same-name references and excluded files. The [protocol](protocol.md) records the questions and amendments; [operations](operations.json) contains every search and read, including unsuccessful attempts.

The control used `rg` file/text search and numbered source excerpts. The MCP arm used bounded ranked search and exact inspection tools. Its UI-to-DM case additionally needed two ordinary searches to read TypeScript; those are charged to the MCP-assisted total. A pure MCP-only workflow could not complete that cross-language case.

The target was Meridian-Rift revision `7462a6942b2e71a3ea13c00169f65f575cb281b7`. The isolated release executable had SHA-256 `94c7dd9f9a5f88299bd079441ce579772a2ad8fb75e14a0c5f900286a85721a9`. The connected app process was older, so it was not used for measured queries. Checkout identity and 42 evidence-file hashes were checked; the source stayed unchanged. Production code was not edited or executed.

## Measured costs

These are complete-case operation counts and exact returned-text character counts from the exploratory walkthrough. They are **not actual billed or reasoning-token counts**. Output includes MCP's JSON fields, sources and metadata. Parse/setup calls are additional and shown separately. Tool discovery, orchestration wrappers, discussion and final answers are outside these per-arm measurements.

| Case | Text operations | MCP-assisted operations | Text characters | MCP-assisted characters |
|---|---:|---:|---:|---:|
| R1: exact vending procedure and dispensing | 2 | 2 | 5,192 | 7,481 |
| R2: inherited duffel-bag size | 4 | 4 | 5,801 | 5,645 |
| R3: negative vending override lookup | 1 | 1 | 0 | 650 |
| R4: airlock obstruction protection | 3 | 3 | 6,872 | 12,600 |
| R5: browser purchase through DM dispensing | 6 | 4 | 5,342 | 11,236 |
| R6: spell implementations | 1 | 1 | 3,730 | 18,136 |
| R7: plant water/nutrient consumption | 3 | 2 | 5,924 | 7,463 |
| **Total** | **20** | **17** | **32,861** | **63,211** |

The MCP-assisted total contains 15 MCP calls and two text operations. Input-operation descriptions were 2,219 characters versus 2,404 for text search; returned content dominated the recorded text cost. A crude characters/4 proxy would put returned content near 15,803 versus 8,216 tokens. That proxy has no verified relationship to this model's tokenizer, caching or hidden reasoning usage and must not be treated as billing evidence.

Five fixed-query replays alternated arm order and verified result equivalence before accepting timings. They repeat backend work, not independent reasoning trials.

| Cost | Measurement |
|---|---:|
| Seven-case text backend time, median | 3.395 s |
| Seven-case MCP-assisted backend time, median, already parsed | 0.093 s |
| Fresh full-project parse, median of five processes | 29.324 s |
| Fresh parse range | 27.886–38.170 s |
| Unchanged-snapshot validation, median of five calls | 1.566 s |
| MCP working set retained after parse | approximately 1.86 GB |
| MCP private bytes retained after parse | approximately 2.00 GB |

“Fresh” means a new parser process, not an emptied operating-system file cache. Backend time sums operation durations and excludes model deliberation, desktop tool overhead and network latency. Source-read timing excludes Python/shell startup, while `rg` subprocess startup and MCP request/response transfer are included. The final R5 read was timed separately and added to its case totals. These are resource measurements, not end-to-end task completion times.

If only backend latency mattered, the median parse would amortize after about nine such seven-case batches, or roughly 63 comparable questions. This is an illustrative calculation, not a forecast: real questions, edits, parser lifetime and cache reuse differ. It also does not repay the additional returned text. A warm parser can still be worthwhile immediately when it answers a semantic question that text search cannot settle cheaply.

The analysis inventory's serialized definitions occupied 16,497 characters; the ten navigation/parse definitions occupied 5,794. Those sizes were measured separately. The app's lazy loading and caching were not instrumented, so they are not charged as tokens on every turn.

## What the answers showed

- **Exact lookup:** both found `vend(params, user, greyscale_colors)` and its call to `dispense`. Both established that newly created stock decrements `item_record.amount`, returned items are handled separately, and pickup depends on the signal, reachability and hands. MCP was fast but returned extra metadata and documentation.
- **Inherited values:** `dm_get_var` returned “Variable not found” for the duffel bag's inherited `w_class`. Following its parent recovered the value, 4, assigned on `/obj/item/storage/backpack` and declared on `/obj/item`. The same limitation reproduced in the explicit-parent fixture. This is a real interaction cost, not evidence that the parser cannot represent inheritance.
- **Implementation sets:** both found the same 28 spell owners, files and lines: one base implementation and 27 overrides. Every reported source location was checked. An unrelated component's same-name procedure was excluded. Both found no concrete vending `attackby` implementations. The 28-result MCP response was almost five times the text-search output, largely because of repeated structured fields.
- **Behavior discovery:** the hydroponics query ranked the correct `process` procedure first and saved a follow-up step. It showed nutrient depletion and water drain, with shortage damage and trait exceptions; age advancement is not directly paused by those shortage checks. The airlock query initially ranked a tram door, a door controller and gun safety, omitting the main airlock `close` procedure from its first three results. Refinement recovered the `safe`/`force_crush` decision and a safety toggle. The ordinary source reads also found the safety-wire controls.
- **Parser correctness fixture:** MCP resolved an explicit `parent_type`, selected the active `#if` branch, excluded the uncompiled `orphan.dm`, returned only the two statically resolved `reset` calls, and reported one skipped dynamic call. These are useful distinctions that a raw matching line does not establish. Manual reading of the small fixture reached the same answers, so this proves tool behavior, not a production-scale reasoning advantage.

The full-project parse reported 127 parser/checker errors; the manufactured fixture reported zero. Parsed results were checked against selected source, and the spell owner/location sets matched. This does not establish repository-wide parser completeness or compiler/runtime correctness.

## Can response cost be reduced?

Yes. A post-hoc, question-focused display projection reduced the MCP-assisted total from 63,211 to 38,249 characters, only about 16% above the text control. It kept all returned candidate source excerpts and removed repeated or unnecessary metadata; the inherited-type view retained its parent and local variables. This projection was not a second solved-task trial, and removing fields can affect subsequent reasoning. It demonstrates an output-size opportunity, not a measured token or productivity improvement.

The immediate development priorities suggested by this experiment are:

1. Add compact, task-focused responses and avoid repeating build identity and ownership fields on every row.
2. Resolve inherited variable values and their assignment/declaration owners in one request.
3. Improve query-local source excerpts and evaluate discovery on held-out behavior questions before expanding search infrastructure.
4. Measure parser startup/residency and reuse across actual development sessions. Do not assume a more elaborate search engine will repay these costs.

## Recommended working policy

Use `rg` and narrow reads for known identifiers, local edits, simple call paths and cross-language exploration. For a known symbol with an already warm parser, use the exact MCP inspection tool directly; ranked search is not a mandatory preliminary step.

Use the parser when the question depends on semantic parents, inherited methods, active preprocessing, override ownership, or distinguishing same-name references. Parse once for that work, reuse the snapshot, and reparse after relevant changes. Pair static references with text inspection for dynamic calls and non-DM code.

Keep build, debugger, profiling, memory, map and image capabilities available. This experiment evaluated source exploration and gives no basis for removing those capabilities. It also gives no basis for replacing repository build/runtime checks with parser diagnostics.

## Limits and reproduction

This was one analyst in one conversation. Knowledge leaked across arms and related cases; alternating order cannot remove that. R6 was added after both R3 arms returned an empty set, and R7 was added to broaden behavior discovery. Neither replaces an unfavorable result. The final R5 control read was added during rubric review because its first excerpt did not reach the dispensing call; it is included in the six-operation count and timing. Required source-backed facts were checked manually, with executable set/signature checks where applicable; there was no independent blinded grader.

An initial replay accidentally let pathless `rg` search empty piped stdin. Its query timings were discarded. The corrected harness supplies explicit paths and checks normalized response signatures, preventing an empty search from masquerading as a speedup. Its independent parse measurement remains valid and is included in the five-process parse sample. Raw failed and accepted runs remain in ignored `target/exploration-comparison/`.

The reusable experimental harness below is baseline-pinned and Windows-only. It checks the binary hash, checkout revision, source hashes and all 49 recorded operation responses, then shuts down its isolated MCP. It does not change app registration or the connected MCP. The original five-replay timing study and a separate one-replay validation of this packaged harness both passed; the latter matched all 49 operations and exited 0.

```powershell
python docs/audits/2026-09-06-exploration-comparison/run.py `
  --binary "$env:LOCALAPPDATA/meridian-mcp/releases/memory-94c7dd9f9a5f/meridian-mcp.exe" `
  --rift-root ../Meridian-Rift `
  --output target/exploration-comparison-replay-new `
  --repeats 5
```

Choose a new output directory. Keep raw responses local because they contain absolute paths and source excerpts. A changed binary or checkout needs a separate candidate study and renewed answer grading; this baseline should not silently accept changed semantics.

Artifacts: [measurements and source hashes](results.json), [operation sequence](operations.json), [expected response signatures](expected.json), [replay harness](run.py), [fixture](fixture/fixture.dme), [protocol](protocol.md).
