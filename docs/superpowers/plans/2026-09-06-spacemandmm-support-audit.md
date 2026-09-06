# SpacemanDMM support audit and repairs

**Goal:** Verify the pinned upstream capability surface against working Meridian-MCP tools, repair functional gaps, and make omissions fail CI.

**Architecture:** Preserve the pinned libraries and typed MCP interface. Follow semantic inheritance for variable queries. Discover upstream handlers, advertised capabilities, CLI commands and wire types from source, then require explicit registry mappings and wire compatibility.

**Tech stack:** Rust 1.95.0, PowerShell 7, SpacemanDMM `351ddc0ffb2439876d4565ce5130bb6b027ee605` with `meridian-read-policy-v2`.

**Scope:** User-authorized audit and support repairs in the existing checkout. Preserve the exploration comparison artifacts. No commits, installation changes, upstream upgrade, new worktree or delegated agents. Editor transport and legacy extools are evaluated and documented separately from agent-facing support.

- [x] Inspect pinned workspace, LSP request/notification tables and providers, map CLI, docs/checker CLI, DMI implementation, and debugger protocol.
- [x] Add failing regression coverage for inherited values, overridden values with inherited declarations, explicit `parent_type`, builtins, nulls and missing symbols; fix `src/tools/parse.rs` using semantic ancestry.
- [x] Replace the audit's fixed feature checklist with source-derived inventories; check both additions and removals and exact serialized debugger field order. Add mutation tests for unmapped features and protocol drift.
- [x] Correct dependency/equivalence claims in `spacemandmm-capabilities.json`; require the source audit and its regression tests in CI. Add a documentation helper stdio fixture to both platforms.
- [x] Run focused tests, source audit, format, strict Clippy and full tests with the exact toolchain; exercise real Meridian-Rift inheritance through the newly built stdio server.
- [x] Write the support matrix and evidence limitations in a repository-local audit report and link it from the maintained documentation. Leave changes uncommitted.

Results: [support audit](../../audits/2026-09-06-spacemandmm-support.md) and [verification record](../../audits/2026-09-06-spacemandmm-support-evidence.json). Hosted CI, live debugger qualification and release installation remain separate gates.
