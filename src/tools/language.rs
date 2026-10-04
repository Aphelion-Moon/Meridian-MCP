use anyhow::{anyhow, Result};
use serde_json::{json, Map};
use std::path::Path;

use crate::index::{ReferenceHit, ReferenceKind, SymbolId};
use crate::mcp::ToolResult;
use crate::spaceman::language::ResolvedReference;
use crate::state::ServerState;
use dreammaker::objtree::{ObjectTree, SymbolId as UpstreamSymbolId};
use dreammaker::Location;

mod page;
use page::Page;

fn resolve_symbol(
    objtree: &ObjectTree,
    owner: &str,
    member: Option<&str>,
) -> Result<(SymbolId, UpstreamSymbolId, Location)> {
    let ty = objtree
        .find(owner)
        .ok_or_else(|| anyhow!("Type not found: {owner}"))?;
    let Some(member) = member else {
        return Ok((
            SymbolId::Type {
                path: ty.path.as_str().into(),
            },
            ty.id,
            ty.location,
        ));
    };
    // Resolve both kinds along semantic ancestors, rather than scanning every
    // declaration in the repository to recover an already known owner.
    let variable = ty.iter_parent_types().find_map(|parent| {
        let declaration = parent.get().vars.get(member)?.declaration.as_ref()?;
        Some((parent.get().path.as_str(), declaration))
    });
    let procedure = ty.iter_parent_types().find_map(|parent| {
        let declaration = parent.get().procs.get(member)?.declaration.as_ref()?;
        Some((parent.get().path.as_str(), declaration))
    });
    match (variable, procedure) {
        (Some(_), Some(_)) => Err(anyhow!(
            "Ambiguous member {owner}/{member}: both variable and procedure declarations exist"
        )),
        (Some((owner, declaration)), None) => Ok((
            SymbolId::Var {
                owner: owner.into(),
                name: member.into(),
            },
            declaration.id,
            declaration.location,
        )),
        (None, Some((owner, declaration))) => Ok((
            SymbolId::Proc {
                owner: owner.into(),
                name: member.into(),
                override_index: 0,
            },
            declaration.id,
            declaration.location,
        )),
        (None, None) => Err(anyhow!("Member not found: {owner}/{member}")),
    }
}

pub async fn document_symbols(
    state: &ServerState,
    args: crate::parameters::DocumentSymbolsParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let file = args.file_path.as_str();
    let page = Page::new(
        &snapshot,
        args.limit,
        args.detail
            .as_ref()
            .map(|detail| detail.as_str())
            .unwrap_or("full"),
        args.cursor.as_deref(),
        json!(["document_symbols", file]),
    )?;
    let symbols = snapshot.language_index.document_symbols(Path::new(file));
    page.respond(
        &snapshot,
        "symbols",
        symbols.iter(),
        symbols.len(),
        Map::new(),
    )
}

pub async fn find_implementations(
    state: &ServerState,
    args: crate::parameters::FindImplementationsParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let owner = args.type_path.as_str();
    let member = args.member_name.as_deref();
    let (symbol, _, _) = resolve_symbol(&snapshot.objtree, owner, member)?;
    if !snapshot.language_index.hierarchy_is_valid() {
        return Err(anyhow!("Cannot query implementations of an invalid semantic type hierarchy; inspect parser diagnostics."));
    }
    let page = Page::new(
        &snapshot,
        args.limit,
        args.detail
            .as_ref()
            .map(|detail| detail.as_str())
            .unwrap_or("full"),
        args.cursor.as_deref(),
        json!(["implementations", owner, member]),
    )?;
    let implementations = snapshot
        .language_index
        .implementation_iter(owner, member)
        .filter(|hit| match (&symbol, &hit.symbol) {
            (SymbolId::Type { .. }, SymbolId::Type { .. }) => true,
            (SymbolId::Var { owner, .. }, SymbolId::Var { .. })
            | (SymbolId::Proc { owner, .. }, SymbolId::Proc { .. }) => {
                hit.declaration_owner == *owner
            }
            _ => false,
        });
    let count = implementations.clone().count();
    page.respond(
        &snapshot,
        "implementations",
        implementations,
        count,
        Map::new(),
    )
}

pub async fn find_references(
    state: &ServerState,
    args: crate::parameters::FindReferencesParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let owner = args.type_path.as_str();
    let member = args.member_name.as_deref();
    let (symbol, upstream_symbol, location) = resolve_symbol(&snapshot.objtree, owner, member)?;
    let include_declaration = args.include_declaration.unwrap_or(false);
    let kind = args.kind.map(|kind| match kind {
        crate::parameters::FindReferencesKind::Call => ReferenceKind::Call,
        crate::parameters::FindReferencesKind::Read => ReferenceKind::Read,
        crate::parameters::FindReferencesKind::Write => ReferenceKind::Write,
        crate::parameters::FindReferencesKind::TypePath => ReferenceKind::TypePath,
        crate::parameters::FindReferencesKind::MacroExpansion => ReferenceKind::MacroExpansion,
        crate::parameters::FindReferencesKind::Declaration => ReferenceKind::Declaration,
    });
    let page = Page::new(
        &snapshot,
        args.limit,
        args.detail
            .as_ref()
            .map(|detail| detail.as_str())
            .unwrap_or("full"),
        args.cursor.as_deref(),
        json!(["references", owner, member, kind, include_declaration]),
    )?;
    let uses = snapshot.reference_table.references(upstream_symbol);
    let declaration = include_declaration.then_some(ResolvedReference {
        location,
        kind: ReferenceKind::Declaration,
    });
    let split = if include_declaration {
        uses.partition_point(|hit| hit.location < location)
    } else {
        0
    };
    let matching = uses[..split]
        .iter()
        .copied()
        .chain(declaration)
        .chain(uses[split..].iter().copied())
        .filter(|hit| kind.is_none_or(|kind| hit.kind == kind));
    let count = matching.clone().count();
    let references = matching.map(|reference| ReferenceHit {
        symbol: symbol.clone(),
        kind: reference.kind,
        file: snapshot
            .context
            .file_path(reference.location.file)
            .display()
            .to_string(),
        line: reference.location.line,
        column: reference.location.column,
    });
    let skipped_dynamic = snapshot.reference_table.skipped_dynamic();
    page.respond(
        &snapshot,
        "references",
        references,
        count,
        Map::from_iter([
            ("skipped_dynamic".into(), json!(skipped_dynamic)),
            ("skipped_dynamic_scope".into(), json!("environment")),
        ]),
    )
}
