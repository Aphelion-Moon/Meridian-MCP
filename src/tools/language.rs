use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

use crate::index::{ReferenceHit, ReferenceKind, SymbolId};
use crate::limits::ServerLimits;
use crate::mcp::ToolResult;
use crate::spaceman::language::ResolvedReference;
use crate::state::ServerState;
use dreammaker::objtree::{ObjectTree, SymbolId as UpstreamSymbolId};
use dreammaker::Location;

mod page;
use page::Page;

fn member_name(args: &Value) -> Result<Option<&str>> {
    args.get("member_name")
        .map(|value| {
            value
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| anyhow!("member_name must be a non-empty string"))
        })
        .transpose()
}

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

pub async fn document_symbols(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let file = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing file_path argument"))?;
    let maximum = ServerLimits::default().max_document_symbols;
    let page = Page::new(&snapshot, &args, json!(["document_symbols", file]), maximum)?;
    let symbols = snapshot.language_index.document_symbols(Path::new(file));
    page.respond(
        &snapshot,
        "symbols",
        symbols.iter(),
        symbols.len(),
        Map::new(),
    )
}

pub async fn find_implementations(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let owner = args
        .get("type_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing type_path argument"))?;
    let member = member_name(&args)?;
    let (symbol, _, _) = resolve_symbol(&snapshot.objtree, owner, member)?;
    if !snapshot.language_index.hierarchy_is_valid() {
        return Err(anyhow!("Cannot query implementations of an invalid semantic type hierarchy; inspect parser diagnostics."));
    }
    let maximum = ServerLimits::default().max_reference_results;
    let page = Page::new(
        &snapshot,
        &args,
        json!(["implementations", owner, member]),
        maximum,
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

pub async fn find_references(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let owner = args
        .get("type_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing type_path argument"))?;
    let member = member_name(&args)?;
    let (symbol, upstream_symbol, location) = resolve_symbol(&snapshot.objtree, owner, member)?;
    let maximum = ServerLimits::default().max_reference_results;
    let include_declaration = args
        .get("include_declaration")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| anyhow!("include_declaration must be a boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    let kind = args
        .get("kind")
        .map(|value| match value.as_str() {
            Some("call") => Ok(ReferenceKind::Call),
            Some("read") => Ok(ReferenceKind::Read),
            Some("write") => Ok(ReferenceKind::Write),
            Some("type_path") => Ok(ReferenceKind::TypePath),
            Some("macro_expansion") => Ok(ReferenceKind::MacroExpansion),
            Some("declaration") => Ok(ReferenceKind::Declaration),
            _ => Err(anyhow!("Invalid reference kind")),
        })
        .transpose()?;
    let page = Page::new(
        &snapshot,
        &args,
        json!(["references", owner, member, kind, include_declaration]),
        maximum,
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
