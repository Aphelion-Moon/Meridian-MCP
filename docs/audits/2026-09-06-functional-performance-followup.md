# Functional, performance and response-efficiency follow-up

Objective: continue evidence-backed audits and repairs across Meridian-MCP, including practical response/token efficiency. Work stays in the existing checkout; commits are authorized and subagents are not used.

## Current work

- Committed the exploration comparison as `1a093fd` and the prior SpacemanDMM repair/audit as `288cf49`. The comparison remains an immutable historical baseline, including its failed inherited-variable query.
- Freshly reran 31 focused tests from the built test executables and the upstream source audit before committing. The prior full qualification remains recorded separately in the SpacemanDMM report.
- Found that implementation filtering uses lexical path prefixes, misses explicit semantic children and includes redirected non-children. Variable overrides are omitted from the implementation index.
- Found that reference lookup scans the complete object tree to recover an ancestor's declaration owner. `include_declaration` filters an already use-only table, so it cannot include a declaration.
- Language listings previously defaulted to 10,000 references/implementations or 20,000 document symbols. A new stress fixture reproduced a 1,874,020-byte reply, exceeding the 1 MiB transport ceiling.
- Found unconditional path lowercasing in the language index and search document identity. Case-sensitive filesystem handling needs a focused follow-up; this Windows qualification does not establish that behavior on Linux.
- The first release candidate increased parsed-process private memory by about 352 MB while adding previously omitted variable assignments. Its raw run and portable results were preserved. Sharing index text reduced the final median to 1.952 GB, about 50 MB below the matched baseline, with unchanged corrected rows. Complete compact replies used 25.0% fewer characters than full replies. See the [language-query measurements](2026-09-06-language-queries/README.md).

## Workplan and completion evidence

- [x] Repair semantic implementation queries, variable overrides, reference declaration handling and invalid request validation; exercise explicit parents and unrelated same-name members. Nine new regression tests cover this batch, paging and repeated text storage. All 19 focused language/proc tests pass.
- [x] Add bounded continuation and optional compact language-query responses, retaining full rows and explicit counts, ownership, source locations, generation and uncertainty. Fixtures verify complete unions and lossless compact reconstruction, including results exceeding the old transport ceiling. Real-repository measurements are recorded separately.
- [x] Apply the exploration result to server/README guidance: simple and cross-language work can use text search, warm exact queries can go directly to inspection, parsing is required only for source tools that use the snapshot.
- [ ] Audit ranked-search/exact-inspection source excerpts and metadata cost. Preserve useful code, report partial excerpts, and measure behavior on varied real subsystems before claiming a relevance improvement.
- [ ] Continue profiling parser initialization and retained memory. Language-index text sharing is qualified; final cold-parse median was still 6.3% higher than the matched baseline with variable samples. Do not claim a cold-start speedup.
- [ ] Revisit functional/runtime/artifact paths and outstanding evidence gates across the tool inventory. Keep local, hosted, static and live evidence distinct; repair reproduced defects and document platform limits.
- [ ] Qualify and commit coherent changes with appropriate focused/full checks, record candidate-versus-baseline measurements, and prepare a reproducible release handoff.

The broad goal remains active. Completion is not implied by finishing only the first language-query repair.

Local qualification for the language-query batch after storage sharing: Rust 1.95.0, strict all-target/all-feature Clippy, formatting, 415 passing tests and four existing ignored gates. The source-derived capability audit still passes for 50 records covering 128 entries and debugger wire layouts. Hosted CI, installation and live BYOND are separate gates.
