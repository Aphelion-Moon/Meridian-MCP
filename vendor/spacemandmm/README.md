# SpacemanDMM local read-policy patch

Baseline: SpaceManiac/SpacemanDMM revision `351ddc0ffb2439876d4565ce5130bb6b027ee605`.
Local delta identity: `meridian-read-policy-v4`.

Only the `dreammaker` and `dmm-tools` crate directories are vendored. The upstream LICENSE is retained verbatim beside them. Existing upstream author and source notices remain intact. Adjacent crates remain exact-revision Git dependencies; no registry versions or upstream revision were upgraded. Package manifests expand the inherited edition 2024 and Rust 1.95 metadata and replace sibling paths with exact-revision dependencies. The root Cargo patch unifies dreamchecker's dreammaker types with the local crate.

`local-delta.patch` is the complete text delta from the baseline crate directories, with LF line endings. `local-delta.sha256` records its SHA-256 (UTF-8, LF). Parse and server-status responses expose the delta name and hash separately from the upstream revision. Independently built dmdoc/debugger helpers remain upstream builds and do not inherit these in-process loader hooks.

`source-files.json` binds that delta identity to the complete shipped crate inventory and LICENSE using SHA-256 over UTF-8 text with LF normalization. The capability audit rejects changed, missing, or additional files, while accepting LF and CRLF checkouts. After a reviewed vendor change, regenerate both the delta and source inventory; a patch-document checksum alone is not source verification.

## Boundary

`dreammaker::ReadPolicy` delegates canonical path resolution to a host-owned immutable policy. Context checks the initial lexer input, every preprocessor DM include, and configuration loads immediately before opening the resolved path. A separate denial flag survives disabled diagnostics and makes Meridian discard the whole candidate parse. Search indexing reuses the same checked resolver. Parsed contexts retain the startup policy for subsequent source inspection.

`dmm-tools::IconCache` accepts an owned policy when constructed and checks each icon load before decoding. Its independent denial flag is checked before Meridian encodes or writes the rendered artifact. DMI analysis checks every discovered file before loading, and directory traversal checks each directory before listing it.

These are canonicalization-before-open checks, matching the project's PathPolicy contract. They are not OS-handle-based protection against an adversary concurrently replacing directories between canonicalization and open.

Version 3 also retains the parser's existing procedure-header start location in `ProcValue.header_location`. This adds one `Location` per implementation without enabling the complete editor annotation tree. Meridian uses it with the existing body range to keep physical source excerpts within their own declarations, including multiple procedures on one line. It does not alter parsing or name resolution. Source excerpts remain physical source, with explicit file-boundary fallback when a body range leaves the declaring file.

## Review and renewal

Version 4 rejects cyclic `parent_type` graphs before constant evaluation and retains a separate cycle flag so disabled diagnostics cannot make Meridian accept the candidate. It also adds a checked, cell-limited DMM loader. Meridian limits the dense grid to 16,777,216 cells before allocation; upstream callers retain the existing loader API.

The authoring exporter also uses `Preprocessor::branch_with_current_defines` to inspect constants with the exact live macro stacks. This small public API copies existing define state into a buffer-only child; it does not change ordinary preprocessing. It avoids treating the root environment's EOF source location as later than included-file definition locations when inspecting final state.

Compare the vendored files against the exact baseline and review `local-delta.patch`; do not edit Cargo's cache. Keep upstream helper/CI pins unchanged for this patch. Re-run containment, snapshot, map, DMI, stdio, and the repository's full Rust qualification before promoting compatibility. Any upstream revision change requires the normal dependency-update matrix and a fresh delta/hash.

Changed baseline files:

- `dreammaker/Cargo.toml`
- `dreammaker/src/error.rs`
- `dreammaker/src/lexer.rs`
- `dreammaker/src/lib.rs`
- `dreammaker/src/objtree.rs`
- `dreammaker/src/parser.rs`
- `dreammaker/src/preprocessor.rs`
- `dmm-tools/Cargo.toml`
- `dmm-tools/src/dmm.rs`
- `dmm-tools/src/dmm/read.rs`
- `dmm-tools/src/icon_cache.rs`
