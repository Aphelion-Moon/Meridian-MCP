# Language-query audit and measurements

Keep Meridian's parser for semantic questions and use text search for simple discovery. The repaired language tools return correct inheritance results, support complete bounded retrieval, and have an optional smaller response format. Parsing still costs roughly 25–32 seconds and about 2 GB of private process memory on this target; these results do not justify mandatory parsing for every exploration task.

This follows the [SpacemanDMM support audit](../2026-09-06-spacemandmm-support.md) and [exploration comparison](../2026-09-06-exploration-comparison/README.md). The [protocol](protocol.md), [final results](results.json), and [initial candidate](initial-candidate.json) retain the method and evidence. Neither comparison measures billed tokens, agent reasoning effort, or end-to-end task completion time.

## Repairs

- Implementation queries follow semantic `parent_type`, include explicit parents at unrelated paths, exclude redirected lexical children, and include local variable assignments. Member queries stay within the requested type's semantic subtree and declaration family.
- References resolve declaration ownership through ancestors. `include_declaration: true` now adds an explicitly labeled declaration for types, variables and procedures. `skipped_dynamic_scope: "environment"` explains the scope of the existing uncertainty counter.
- References, implementations and document symbols default to 100 rows. They return `total_count` and continuation cursors bound to the query and snapshot. The page also stops before the transport ceiling. A controlled fixture reproduced the old 1,874,020-byte reply and now retrieves the complete set through bounded pages.
- `detail: "full"` retains the existing row fields. Optional `detail: "compact"` uses minified JSON and moves common row fields into `shared`; overlaying each row on `shared` reconstructs its full fields. Limits and malformed inputs are validated.
- The implementation index groups semantic subtrees, so narrow queries avoid a repository-wide scan. File paths, member names and owners share immutable text within the snapshot. The build's interning tables are discarded after construction.
- Server and README guidance now recommends text search for literal/file discovery and cross-language work, direct inspection for known symbols in a loaded snapshot, and parser-backed analysis for semantic questions.

## Complete response results

Eight cases cover five Meridian-Rift subsystems. Each view has 15 measurements across three fresh server processes. Full/compact order alternates, and every continuation page is retrieved. These cases used the same explicit `limit: 10000`; each complete view fit in one page. The smaller production default can require additional calls.

| Query | Final rows | Full characters | Compact characters | Baseline median ms | Final full median ms | Final compact median ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Spell implementations | 28 | 18,261 | 13,384 | 3.49 | 0.81 | 0.77 |
| Vending implementations (negative control) | 0 | 774 | 666 | 2.72 | 0.35 | 0.36 |
| Airlock references | 18 | 6,274 | 3,432 | 0.75 | 0.47 | 0.45 |
| Hydroponics references | 20 | 7,254 | 3,855 | 2.71 | 0.48 | 0.43 |
| Vending references | 2 | 1,510 | 982 | 0.52 | 0.30 | 0.30 |
| Storage descendants | 188 | 111,494 | 92,395 | 19.89 | 2.59 | 2.24 |
| Vending document symbols | 10 | 5,939 | 2,683 | 0.74 | 0.66 | 0.64 |
| Spell document symbols | 42 | 22,213 | 12,905 | 0.99 | 0.99 | 0.93 |

Total returned characters, including metadata: **173,719 full versus 130,302 compact, a 25.0% reduction**. This is a character measurement, not a token-saving estimate. Large descendant listings remain verbose; narrower questions are still preferable.

All final full and reconstructed compact rows match exactly, in order. Their hashes also match the first candidate before storage sharing. Seven baseline/final result sets are identical. The spell document adds eight real assignments at `code/modules/spells/spell.dm:44–51` that the old declaration-only index omitted; no old rows disappear. Their declaration owners are `/datum/action` or `/datum/action/cooldown`. The zero-result vending case is retained explicitly: an inherited declaration does not imply a concrete implementation inside the vending subtree.

## Startup and process footprint

Three cold processes per arm, alternating order; “cold” means no parsed server snapshot, not an empty OS file cache. GB below uses decimal bytes. These are MCP process counters after parsing, not BYOND memory or an attribution of every byte to live index objects.

| Metric | Baseline | Final candidate |
| --- | ---: | ---: |
| Median cold parse | 28.215 s | 29.980 s |
| Cold parse range | 27.681–28.356 s | 25.307–31.618 s |
| Median working set after parse | 1.865 GB | 1.821 GB |
| Median private bytes after parse | 2.001 GB | 1.952 GB |

The first candidate exposed a memory regression: median private bytes reached 2.353 GB while adding variable assignments. Sharing index text reduced that to 1.952 GB with identical corrected rows, about 402 MB below the prototype and 50 MB (2.5%) below the matched baseline. The initial run remains recorded rather than being overwritten.

**No cold-start speedup is established.** The final median was 6.3% higher, with substantial candidate variability. Warm query measurements support faster narrow implementation/reference lookup; they do not establish faster complete development tasks. Parser startup remains an audit target.

## Qualification and boundaries

- Exact Rust 1.95.0: formatting, warnings-denied all-target/all-feature Clippy, release build, and **415 passing tests, 0 failures, 4 existing ignored gates**. Nineteen focused language/proc tests include nine new regressions.
- Source-derived SpacemanDMM capability audit: 50 records account for 128 entries and debugger wire layouts at pinned revision `351ddc0ffb2439876d4565ce5130bb6b027ee605`. Explicit exclusions are not counted as implemented features.
- Both experiments used clean Rift commit `7462a6942b2e71a3ea13c00169f65f575cb281b7` and verified 42 selected source hashes before and after. All parses agreed on 65,165 types, 452,780 search symbols, 127 checker errors and zero warnings. These existing checker findings are not a compile result. All owned servers exited naturally with code zero.
- Baseline binary: `94c7dd9f9a5f88299bd079441ce579772a2ad8fb75e14a0c5f900286a85721a9`. Final tested candidate: `c654d93b19cb32530dc499a5fe1ecd550d35d3db8b46b712ac00a4b46f5e92aa`. Both expose dirty source-build provenance; the candidate was built on `288cf49` with this batch's source changes. JSON records retain the complete binary/build identities.
- Hosted CI, installation, and live BYOND/helper qualification were not performed in this batch. The connected Codex MCP registration remains unchanged. Editor/LSP session features and legacy extools remain explicit exclusions in the main support audit.
- Unconditional path lowercasing in the language/search indexes remains a case-sensitive filesystem risk requiring follow-up. This Windows run does not qualify Linux path identity. Dynamic references remain incomplete where static resolution is impossible.
- MCP JSON row fields are preserved in full detail. Rust consumers of the public index structs now receive `Arc<str>` text instead of owned `String`; this is distinct from MCP protocol compatibility.

Continue with the [functional/performance workplan](../2026-09-06-functional-performance-followup.md). The broader audit remains active.
