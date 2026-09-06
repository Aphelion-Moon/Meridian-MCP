use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

use crate::mcp::ToolResult;
use crate::search::{SearchIndex, SearchRequest, SymbolKind};
use crate::source::{SourceExcerpt, MAX_SOURCE_LINES};
use crate::state::ServerState;

const DEFAULT_RESULT_LIMIT: usize = 10;
const MAX_RESULT_LIMIT: usize = 50;
const DEFAULT_SOURCE_LINES: usize = 40;

pub(crate) async fn search_context(state: &ServerState, args: Value) -> Result<ToolResult> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .ok_or_else(|| anyhow!("Missing or empty query argument"))?;

    let kind = parse_kind(optional_nonempty_string(&args, "kind")?.unwrap_or("all"))?;
    let type_prefix = optional_nonempty_string(&args, "type_prefix")?;
    let file_filter = optional_nonempty_string(&args, "file_filter")?;
    let limit = bounded_usize(&args, "limit", DEFAULT_RESULT_LIMIT, 1, MAX_RESULT_LIMIT)?;
    let (include_source, max_source_lines) = source_options(&args, DEFAULT_SOURCE_LINES)?;

    let snapshot = match state.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return Ok(ToolResult::error(
                "No search index loaded. Call dm_parse_environment first.",
            ));
        }
    };
    let index = &snapshot.search_index;

    let request = SearchRequest {
        query,
        kind,
        type_prefix,
        file_filter,
        limit,
    };
    let execution = index.search(&request);
    let results: Vec<Value> = execution
        .hits
        .iter()
        .map(|hit| {
            let document = hit.document;
            let mut result = Map::from_iter([
                (
                    "score".to_string(),
                    json!((hit.score * 1_000.0).round() / 1_000.0),
                ),
                ("kind".to_string(), json!(document.kind.as_str())),
                ("symbol".to_string(), json!(document.symbol)),
                ("name".to_string(), json!(document.name)),
                ("type_path".to_string(), json!(document.type_path)),
                (
                    "implementation_owner".to_string(),
                    json!(document.implementation_owner),
                ),
                (
                    "declaration_owner".to_string(),
                    json!(document.declaration_owner),
                ),
                ("parent".to_string(), json!(document.parent)),
                ("file".to_string(), json!(document.file)),
                ("line".to_string(), json!(document.line)),
                ("column".to_string(), json!(document.column)),
                ("docs".to_string(), json!(document.docs)),
                ("parameters".to_string(), json!(document.parameters)),
                ("override_index".to_string(), json!(document.override_index)),
                ("override_count".to_string(), json!(document.override_count)),
            ]);

            if include_source {
                add_source_fields(&mut result, document.source.as_ref(), max_source_lines);
            }

            Value::Object(result)
        })
        .collect();

    let response = json!({
        "query": query,
        "query_terms": SearchIndex::query_terms(query),
        "indexed_documents": index.len(),
        "count": results.len(),
        "results": results,
        "state_generation": snapshot.generation,
        "source_origin": "analysis_snapshot",
        "source_line_limit": max_source_lines,
        "retrieval": {
            "mode": "lexical",
            "algorithm": "bm25",
            "candidates_considered": execution.candidates_considered,
            "documents_scored": execution.documents_scored,
        },
    });
    Ok(ToolResult::text(serde_json::to_string_pretty(&response)?))
}

fn parse_kind(kind: &str) -> Result<Option<SymbolKind>> {
    match kind {
        "all" => Ok(None),
        "type" => Ok(Some(SymbolKind::Type)),
        "proc" => Ok(Some(SymbolKind::Proc)),
        "var" => Ok(Some(SymbolKind::Var)),
        _ => Err(anyhow!(
            "Invalid kind '{kind}'. Expected all, type, proc, or var."
        )),
    }
}

pub(super) fn source_options(args: &Value, default_lines: usize) -> Result<(bool, usize)> {
    let include_source = match args.get("include_source") {
        None => true,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| anyhow!("include_source must be a boolean"))?,
    };
    let max_source_lines =
        bounded_usize(args, "max_source_lines", default_lines, 1, MAX_SOURCE_LINES)?;
    Ok((include_source, max_source_lines))
}

fn optional_nonempty_string<'a>(args: &'a Value, name: &str) -> Result<Option<&'a str>> {
    args.get(name)
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("{name} must be a non-empty string"))
        })
        .transpose()
}

fn bounded_usize(
    args: &Value,
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize> {
    super::bounded_u64(args, name, default as u64, minimum as u64, maximum as u64)
        .map(|value| value as usize)
}

pub(super) fn add_source_fields(
    result: &mut Map<String, Value>,
    source: Option<&SourceExcerpt>,
    maximum: usize,
) {
    result.extend(Map::from_iter([
        (
            "source".into(),
            json!(source.map(|source| source.render(maximum))),
        ),
        (
            "source_start_line".into(),
            json!(source.map(|source| source.start_line)),
        ),
        (
            "source_start_column".into(),
            json!(source.map(|source| source.start_column)),
        ),
        (
            "source_total_lines".into(),
            json!(source.map(|source| source.total_lines)),
        ),
        (
            "source_truncated".into(),
            json!(source.map(|source| source.truncated(maximum))),
        ),
        (
            "source_boundary".into(),
            json!(source.map(|source| source.boundary)),
        ),
    ]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn context_search_before_parsing_returns_actionable_tool_error() {
        let state = crate::state::ServerState::new();

        let result = search_context(&state, serde_json::json!({"query": "air"}))
            .await
            .expect("tool call should serialize an expected state error");
        let value = serde_json::to_value(result).expect("tool result should serialize");

        assert_eq!(value["isError"], true);
        assert!(value["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("dm_parse_environment")));
    }
}
