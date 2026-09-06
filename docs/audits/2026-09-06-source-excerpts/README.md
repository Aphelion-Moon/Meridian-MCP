# Source excerpt audit and repair

Based on `954faa4` plus this repair. The [earlier exploration comparison](../2026-09-06-exploration-comparison/README.md) and [language-query measurements](../2026-09-06-language-queries/README.md) remain historical baselines.

## Behavior

The old excerpt reader guessed declaration boundaries from unindented text. Both exact inspection and ranked search could include the next nested procedure, stop inside a multiline string, or return multiple same-line procedures as one implementation. It also read only UTF-8 files, despite the parser accepting Latin-1, and exposed a UTF-8 BOM in the first excerpt.

The reader now uses parser header and body locations against original byte offsets, then decodes physical text using SpacemanDMM's UTF-8/Latin-1 rule. It preserves CRLF line semantics and keeps snapshot text after on-disk changes. The local patch is `meridian-read-policy-v3`: it retains the parser's existing header start in one extra `Location` per implementation. It does not enable the full editor annotation tree or alter parsing/name resolution. The complete vendor delta and source inventory were regenerated against the unchanged, clean upstream pin and passed the capability audit.

Search previously accepted requests for 200 lines but retained only 80 without reporting the shorter excerpt. Snapshot excerpts now retain up to 200 lines; lexical indexing still uses at most 80 lines per document. Search defaults to 40 returned lines and exact inspection to 80. Both accept `max_source_lines: 1..200` and `include_source: false`. Invalid flags, filters and limits are rejected instead of ignored or clamped.

## Reading the response

- `state_generation` and `source_origin: analysis_snapshot` identify the analysis snapshot. No source tool silently rereads edited files.
- `source_start_line` and `source_start_column` locate the returned physical text. Columns count original bytes, matching the parser; indentation is retained when it precedes the header on its own line.
- `source_total_lines` counts the available physical span before the snapshot/request caps. `source_truncated` reports whether those caps omitted lines.
- `source_boundary: parser_body_end` means a body end in the same file bounded the excerpt. `file_end` means the parser range ended outside that file or beyond its available lines; the response stops at the physical file boundary. `declaration_line` is a one-line declaration context, as used for types and variables.
- These are physical excerpts, not preprocessor-expanded or reconstructed procedure bodies. In particular, `file_end` and `declaration_line` do not assert that the entire implementation or initializer is present. Null source fields mean no matching physical excerpt was available.
- `include_source: false` removes per-result source fields while retaining ownership, parameters, documentation and locations. Search also retains its ranking and snapshot identity.

## Qualification

The eight integration regressions first reproduced the failures or missing controls, then passed. They cover nested and same-line procedures, multiline strings/comment markers, BOM/Latin-1/CRLF, immutable snapshot excerpts, truncation, invalid search inputs and exact-inspection source budgets. Seven lower-level source tests cover offsets, invalid locations, caps and physical-boundary fallback.

- Windows Rust 1.95.0: **424 tests passed, zero failed, four existing ignored gates**. An initial full build caught a test-only use of a nonexistent `Location::default`; the corrected fixture and full rerun passed.
- Linux Rust 1.95.0 on WSL 1: **417 tests passed, zero failed, five existing ignored gates**. With native PowerShell available, an initial run found one stale patch-attribute assertion; the assertion now checks the required attributes independently, and the final full rerun passed. Platform-specific tests account for the different totals.
- Source capability audit: 50 records covering 128 source capabilities and debugger wire layouts; 143 mutation/line-ending assertions passed.
- Strict all-target/all-feature Clippy passed on Windows and Linux, as did formatting and patch application against the pinned upstream checkout. The freshly built release passed the documentation HTML/index/source-preservation/overwrite fixture and an owned stdio excerpt/metadata/invalid-input preflight.

The prior Linux run's missing native PowerShell prerequisite was addressed with a repository-local, self-contained [PowerShell 7.6.5 archive](https://github.com/PowerShell/PowerShell/releases/tag/v7.6.5). Its published SHA-256 was verified as `b34ab3b19acac1d3d4d0d3cfdb02acf62f457b0b6a962ff008132033f7566844`, and native startup reported 7.6.5. No system installation was made. The host remains WSL 1; the earlier intermittent guardian timeout is not claimed fixed by installing a test prerequisite.

## Measurements

The [protocol](protocol.md) and [runner](run.py) define a matched experiment on eight procedures across seven subsystem groups, plus four natural-language searches. Three fresh Windows release processes per arm ran in alternating order with five repetitions per query: **1,080 recorded queries**, nine candidate invalid-input checks, and six natural exits with code zero. The clean Meridian-Rift revision and 22 inspected source-file hashes remained unchanged.

The [portable results](results.json) retain every timing sample, response volume, returned identity, source boundary and binary hash. The [summary](summary.json) is reproducible with [summarize.py](summarize.py). Raw replies remain under ignored `target/`. Characters and bytes measure response volume; billed tokens and model reasoning were not measured.

| Measure | Baseline | Repaired candidate | Interpretation |
| --- | ---: | ---: | --- |
| Cold parse median | 32.20 s | 33.03 s | +2.6%; three samples per arm, noisy |
| Parsed private memory median | 1,952.80 MB | 1,965.28 MB | +12.48 MB / 0.64%, decimal units |
| Parsed working set median | 1,823.56 MB | 1,834.68 MB | +11.11 MB |
| Exact inspection, default source, eight-query total | 52,318 characters | 59,840 characters | +14.4%; fuller excerpts and boundary metadata |
| Exact-symbol search, default source, eight-query total | 23,103 characters | 25,264 characters | +9.4% |
| Candidate exact inspection without source | — | 19,565 characters | 67.3% below candidate default |
| Candidate exact-symbol search without source | — | 10,555 characters | 58.2% below candidate default |

Cold parse samples were 41.16/32.20/31.41 seconds for the baseline and 33.03/32.21/47.64 for the candidate. This is not evidence of a cold-start speedup or a reliable latency regression. Candidate per-query medians were 0.39–0.82 ms for exact inspection and 0.33–0.49 ms for exact-symbol search; these are local warm-query timings, not model completion time.

Exact-inspection semantic metadata matched across binaries. Source omission preserved every non-source field, and repeated replies were stable. A post-capture comparison verified that all 35 non-null baseline exact-inspection/search excerpts remained available under matching row identities. Every candidate excerpt was checked against its physical source location. The 200-line view returned the complete 166-line hydroponics span; the old snapshot could retain only 80 lines. Controlled fixtures, rather than these eight cases, establish sibling exclusion and decoding correctness.

Three of four natural-language rankings changed. The vending query replaced `vend` with `freebie`; the spell query replaced a variable hit with a touch-spell procedure; the airlock query reordered the same symbols. These changes are recorded, not scored as relevance improvements.

**Decision:** retain the boundary repair and explicit truncation despite the small measured memory increase. Use `include_source: false` when ownership, signatures or locations answer the question, then request bounded code when needed. Default source-bearing responses are larger, so this batch does not justify an unconditional token-saving claim or broader parser adoption. Parser initialization and retained memory remain the next performance investigation.

Run after building the candidate and stopping other builds/tests:

```powershell
python ./docs/audits/2026-09-06-source-excerpts/run.py --baseline $env:BASELINE_MCP --candidate ./target/release/meridian-mcp.exe --rift-root $env:MERIDIAN_RIFT_ROOT --output ./target/source-excerpt-comparison --rounds 3 --repeats 5
python ./docs/audits/2026-09-06-source-excerpts/summarize.py ./target/source-excerpt-comparison/results.json ./target/source-excerpt-comparison/summary.json
```

Use the baseline binary hash and clean Rift revision specified by the protocol. The runner refuses a different baseline or dirty source tree. The measured candidate SHA-256 is `9f17ed47707caa8ec9649f057fc38ed27f272fd84e8b986fbb98fbf43dcfc0ee`; both binaries were built before their respective source commits and correctly report dirty source identities. Subsequent report and test-attribute edits do not change the measured production code. Hosted CI, installation, live BYOND and a restart of connected Codex tasks remain separate gates.
