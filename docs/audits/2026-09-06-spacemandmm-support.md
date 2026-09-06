# SpacemanDMM support audit

This initial audit covers upstream revision `351ddc0ffb2439876d4565ce5130bb6b027ee605`, Meridian's `meridian-read-policy-v2` patch, and MCP source based on commit `0445bac0f1984053486f0e3af3f9c6da91a9e49c` plus the repairs committed as `288cf49`. It does not qualify a different upstream revision or replace BYOND compile/runtime evidence.

The subsequent [language-query audit and measurements](2026-09-06-language-queries/README.md) cover semantic descendants, variable assignments, declaration references, bounded compact pages and index storage. The broader [follow-up workplan](2026-09-06-functional-performance-followup.md) tracks remaining work.

Meridian integrates the parser, checker, icon and map libraries, packages the documentation generator, and supplies typed source and debugger tools. **This is agent-facing integration, not complete LSP/editor or legacy extools parity.** Registry coverage means every audited entry has an explicit disposition; excluded entries do not count as implemented functionality.

## Findings and repairs

1. **Inherited variables failed inspection.** `dm_get_var` used the current type's local variable map, although upstream exposes semantic parent lookup. It now resolves the effective value and declaration independently, including explicit `parent_type`, inherited type annotations, and declaration documentation. `value_owner`, `declaration_owner`, `declaration_location`, `inherited`, and `state_generation` make that distinction observable. `declared` retains its meaning: the requested type declares the variable. This is static initialization data, not a live object's value.
2. **Capability audit could pass without inspecting upstream.** CI omitted `-UpstreamPath`. Its optional source check also used fixed feature lists that could miss additions. The replacement inventories the actual source tables, rejects missing and stale mappings, verifies the pinned checkout is unchanged, and compares serialized debugger variants, fields and types in declaration order. Without upstream source, the command now explicitly labels its narrower registry/vendor-only result.
3. **Dependency and equivalence claims were too broad.** `dap-types` is not a linked dependency; Meridian owns its protocol model. The registry now says so. Type-definition and hover/completion/signature records describe the exact-symbol workflow and its cursor-related limitations. CLI switches that are not exposed are recorded explicitly.
4. **Documentation generation lacked a normal CI execution gate.** CI built the helper but did not exercise HTML generation. A new stdio fixture verifies generated type/member documentation, `dmdoc.index_file`, source preservation, overwrite rejection and clean shutdown.

## Support matrix

| Upstream function | Meridian interface and evidence | Boundary |
| --- | --- | --- |
| DreamMaker preprocessing, object tree, constants, builtins and proc AST | `dm_parse_environment`, exact type/proc/var inspection; parser, snapshot, inheritance and read-policy fixtures | Uses the active on-disk DME include graph. Reparse after edits. No unsaved editor buffers. |
| DreamChecker analysis and configuration | Checker runs during parse; `dm_check_errors` returns the cached structured snapshot, including all four severities and configuration provenance | Static checker findings are not DreamMaker results. Alternate CLI `-c` and `--parse-only` are not MCP inputs. |
| Workspace/document symbols, definitions, references and implementations | MCP symbol/reference indexes, exact definitions and semantic proc resolution; language and proc fixtures | Dynamic dispatch cannot always be resolved; inspect reported uncertainty. No promise of complete dynamic call graphs. |
| Hover, completion, signature help and type definition | Exact inspection, source snippets, declared variable types and search | Adapted workflow, not cursor-position inference, completion ranking or active-parameter selection. |
| LSP lifecycle, text synchronization, folding, colors, links and tracing | Explicit registry exclusions | Meridian serves MCP. It does not run an editor session or implement these presentation features. |
| DMI metadata, pixels, duplicate analysis and extraction | `dm_dmi_info`, comparison/duplicate/audit tools and `dm_extract_dmi`; bounded decoding and pixel-equivalence fixtures | Hotspot metadata is a TODO in the pinned upstream parser and is reported unsupported. No sprite editor. |
| DMM/TGM parsing, map statistics, coordinate queries and differences | Map tools use `dmm-tools`; map/grid/diff fixtures | Source analysis and map rendering do not establish in-game behavior. |
| Render passes, minimaps and batch rendering | Typed bounded rendering through `dmm-tools`; real PNG and batch preflight fixtures | Raw CLI arguments, external `pngcrush`/`optipng`, and CLI thread controls are not exposed. |
| `dmdoc` HTML generation | Exact-revision, hash-verified fixed helper; new stdio fixture | Development mode and installed helper required. Custom index comes from `SpacemanDMM.toml`; CLI dry-run is not exposed. |
| Auxtools debugger launch, breakpoints, runtime exceptions, inspection, stepping, events and stop | Owned Windows session and typed MCP debugger; source wire-layout comparison plus portable protocol fixtures | Live BYOND/helper/platform qualification remains separate. Arbitrary attach is excluded. |
| Legacy extools / disassembly | Explicit exclusion; pinned auxtools branch rejects disassembly | The extools backend is not integrated. Restart is not implemented in the pinned debugger. |
| Workspace support crates | `interval-tree` and `builtins-proc-macro` are transitive dependencies; DAP transport is superseded | The prototype `spaceman-dmm` editor is commented out of the upstream workspace. |

The source inventory contains **128 entries**: 10 active crates, 17 LSP requests, 9 notifications, 13 provider fields, 21 debugger adapter requests, 5 map commands, 12 documentation/checker CLI switches, and 41 auxtools enum variants. All are assigned to 50 registry records, including explicit exclusions. The mirrored debugger model is checked against all 12 upstream serialized enum/struct definitions. This inventory is intentionally tied to the pinned Rust source layout; an unrecognized required block fails the check.

## Verification

- Source audit: passed against the exact clean upstream checkout, including vendor hashes and debugger wire layouts.
- Audit regression checks: 143 passed, covering removal of every mapping, added upstream capabilities, stale mappings, wire order/type changes, missing source layout and LF/CRLF parity.
- Rust 1.95.0 (`59807616e`): formatting and warnings-denied all-target/all-feature Clippy passed. Full test rerun: **406 passed, 0 failed, 4 ignored**. The ignored fixture entry points/scale gate retain their existing meanings.
- Documentation helper: the freshly built debug MCP passed the stdio HTML/index/source-preservation/overwrite fixture with the manifest-verified `dmdoc` helper.
- Real Meridian-Rift: the maintained analysis script passed on clean commit `7462a6942b2e71a3ea13c00169f65f575cb281b7`, using the debug MCP with the inheritance repair. It exercised full parsing, diagnostics, human `Initialize` ownership, three variable cases, DMI/map inspection and render-pass discovery. The duffel-bag query resolved `Float(4.0)` from `/obj/item/storage/backpack`, declared by `/obj/item`. The checkout remained clean.
- Initial attempts: the linker exhausted disk space; deleting only the repository's disposable incremental cache allowed rebuilding. One full test attempt then failed the existing Windows runtime-lifecycle cancellation fixture with `IdentityUnavailable`; the isolated test and subsequent full suite passed. No runtime-ownership code was changed. An initial real-repository run completed tool assertions but failed to record evidence because of Git ownership checks; the recorded rerun passed with trust restricted to the two checkouts.
- No hosted Ubuntu/Windows CI run or live BYOND debugger session is claimed by this local audit. Installed Codex MCP registration is unchanged; a tested local binary does not update an already-running MCP process.

The [verification record](2026-09-06-spacemandmm-support-evidence.json) preserves the local results and their scope.

Reproduce from a checkout of the exact upstream revision:

```powershell
./scripts/audit-spacemandmm-capabilities.ps1 -Check -UpstreamPath $env:SPACEMANDMM_SOURCE
./scripts/test-spacemandmm-capabilities.ps1 -UpstreamPath $env:SPACEMANDMM_SOURCE
cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.95.0 test --locked --all-features
./scripts/test-spacemandmm-docs.ps1 -BinaryPath ./target/debug/meridian-mcp.exe -HelperManifestPath $env:MERIDIAN_MCP_HELPER_MANIFEST
```

On Windows, run Cargo in a Visual Studio developer shell. Set the two environment variables to the pinned upstream checkout and verified helper manifest before running the commands. CI performs the same capability and helper checks independently on Windows and Ubuntu with a freshly built release binary.

See the [implementation workplan](../superpowers/plans/2026-09-06-spacemandmm-support-audit.md), [capability registry](../../spacemandmm-capabilities.json), and [compatibility gates](../compatibility.md).
