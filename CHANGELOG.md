# Changelog

All notable changes to meridian-mcp will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added bounded source controls to exact procedure inspection and explicit snapshot/truncation metadata to inspection and search. Fixed sibling/same-line procedure excerpts and BOM/Latin-1 decoding; search now rejects invalid filters, flags and limits.
- Added deterministic `dm_search_context` ranking over parsed DreamMaker symbols, documentation, source excerpts, and file paths.
- Added provenance, source-authority, compatibility, dependency, and security records.
- Added independent Windows/Ubuntu over-64K parser evidence and Ubuntu/Windows control-versus-boundary DreamDaemon evidence.
- Added source fingerprinting to `dm_parse_environment` so an unchanged environment reuses the active snapshot instead of reparsing, reported as `reused: true` with an unchanged state generation. Measured on a ~10,000-file, 65,000-type environment, a redundant reparse fell from about 35 seconds to under half a second. Files modified within two seconds of the check are always reparsed, because filesystem timestamp granularity cannot prove an edit in that window did not happen.
- Added `force` and `timeout_ms` arguments to `dm_parse_environment`. A parse that exceeds its timeout is abandoned with a structured error; because a blocking parse cannot be cancelled, the worker keeps running and the next parse queues behind it rather than running alongside it.
- Added error and warning counts, parse duration, the canonical environment path, and the pinned SpacemanDMM revision to the `dm_parse_environment` result, so diagnostic volume no longer needs a second `dm_check_errors` call to discover.
- Added cold/reuse stage timings, explicit lexical/dense readiness, deterministic schema-1 semantic chunk identities, and a checked-in relevance gate with exact MRR and natural-language recall assertions.

### Changed

- Make Topic I/O asynchronous, apply one deadline to the whole exchange, and cancel pending requests with their runtime. Validate Topic arguments before runtime access so status and stop stay responsive.
- Release search-index vector growth capacity after construction and store singleton lookup IDs inline, preserving indexed content and response formats. Added an opt-in storage probe and matched response-equivalence benchmark.
- Validate runtime launch options and readiness regexes before process creation, returning structured `invalid_input` errors. Output waits reject incorrectly typed flags/timeouts while retaining zero-time polling and the five-minute cap. Ownership fixtures now retain startup phase and child-exit evidence on failure.
- Serialized `dm_parse_environment` so overlapping calls queue instead of each building a complete object tree, and stopped holding the previous snapshot alive for the duration of a parse. Both removed a doubled peak memory footprint on large environments.
- Built the post-parse indexes concurrently and centralized detection of the parser diagnostics that mean an environment was never fully read, with a test pinning that wording to the SpacemanDMM revision it is matched against.
- Made `dm_parse_environment` reject a directory or missing path as a structured error before parsing, rather than reporting it as a parser failure.
- Made reuse cover every SpacemanDMM-registered input, corrected the fingerprint contract to path/length/settled-mtime metadata, and classified timeouts and worker failures separately from invalid input.
- Built lexical postings once after canonical proc resolution, cached source line offsets once per file, and ranked only posting/exact/verified-substring candidates before bounded top-k sorting.

- Made workflow contract parsing portable across LF and CRLF checkouts, and made the explicitly diagnostic Windows synthetic runtime lane retain evidence as a warning while Ubuntu remains the required compatibility gate.

- Defined analysis and development capability modes and evidence-based support labels.
- Clarified that `dm_compile` is a direct DreamMaker gate rather than a repository full build.
- Separated real Windows Meridian-Rift, synthetic BYOND runtime, parser-boundary, and Linux Tracy jobs so one specialized failure cannot suppress unrelated evidence.

### Removed

- Removed the unsupported inherited BYOND client-login protocol and `dm_connect_test` from the target supported product; `world.Topic()` remains separate.

### Fixed

- Read DreamDaemon `-logself` files so readiness markers and runtime output are available on Windows.
- Replaced the fixed synthetic DreamDaemon port and ambiguous 60-second missing-marker failure with port-zero launch, a five-minute wall ceiling, bounded process telemetry, explicit classifications, and verified cleanup evidence.
- Separated the flat parser stress corpus from the bucketed BYOND runtime corpus so the latter crosses 64K total declarations without testing DreamMaker's direct-child ceiling instead.
- Made synthetic runtime version provenance portable to Linux and bound its temporary DreamDaemon listener explicitly to loopback for headless hosted runners.
- Correct BYOND Topic request framing and response decoding for string and float responses.
- Parse DMM/TGM maps with SpacemanDMM for exact dimensions, instance counts, and coordinates.
- Render actual map PNGs through the SpacemanDMM minimap pipeline.
- Return type children and documentation, proc documentation and accurate body metadata, and variable documentation and declared types.
- Return experimental full-client handshake failures as structured tool errors instead of JSON-RPC internal errors.

## [0.1.0] - 2026-01-28

### Added

- Initial release as `meridian-mcp`
- **18 MCP Tools** for DreamMaker development:
  - `dm_parse_environment` - Parse .dme files and cache object tree
  - `dm_get_type` - Get type definition with vars and procs
  - `dm_get_proc` - Get proc source code and location
  - `dm_get_var` - Get variable definition and value
  - `dm_list_types` - List types matching a pattern
  - `dm_search_symbols` - Search for symbols by name
  - `dm_check_errors` - Run dreamchecker type analysis
  - `dm_get_definition` - Go to definition for any symbol
  - `dm_compile` - Compile project with DreamMaker
  - `dm_render_map` - Render .dmm map to PNG
  - `dm_map_info` - Get map dimensions and statistics
  - `dm_find_on_map` - Find object instances on maps
  - `dm_run` - Start DreamDaemon server
  - `dm_wait_for_output` - Wait for literal or regex markers in DreamDaemon output
  - `dm_stop` - Stop DreamDaemon server
  - `dm_status` - Check DreamDaemon status
  - `dm_topic` - Send Topic() calls to running server
  - `dm_connect_test` - Test BYOND client connection

- **BYOND Client Protocol** implementation
  - Full packet framing and types
  - RUNSUB encryption support
  - Connection testing capabilities

- Cross-platform support (Windows primary, Linux compatible)
- Integration with SpacemanDMM for AST parsing and type checking
- `dm_*` MCP tool names retained for compatibility with existing clients and workflows

### Technical Notes

- Built with Rust + Tokio for async I/O
- Uses tree-sitter style parsing via SpacemanDMM
- Stateful server design (caches parsed environments)

[0.1.0]: Release notes are documented in this file.
