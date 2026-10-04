use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::mcp::ToolResult;
use crate::search::{SearchIndex, SearchRequest, SymbolKind};
use crate::source::SourceExcerpt;
use crate::state::ServerState;

const DEFAULT_RESULT_LIMIT: usize = 10;
const DEFAULT_SOURCE_LINES: usize = 40;

pub(crate) async fn search_context(
    state: &ServerState,
    args: crate::parameters::SearchContextParams,
) -> Result<ToolResult> {
    let query = args.query.trim();
    let kind = match args
        .kind
        .unwrap_or(crate::parameters::SearchContextKind::All)
    {
        crate::parameters::SearchContextKind::All => None,
        crate::parameters::SearchContextKind::Type => Some(SymbolKind::Type),
        crate::parameters::SearchContextKind::Proc => Some(SymbolKind::Proc),
        crate::parameters::SearchContextKind::Var => Some(SymbolKind::Var),
    };
    let type_prefix = args.type_prefix.as_deref();
    let file_filter = args.file_filter.as_deref();
    let limit = args.limit.unwrap_or(DEFAULT_RESULT_LIMIT as u64) as usize;
    let (include_source, max_source_lines) = (
        args.include_source.unwrap_or(true),
        args.max_source_lines.unwrap_or(DEFAULT_SOURCE_LINES as u64) as usize,
    );

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

        let result = search_context(
            &state,
            crate::parameters::decode(serde_json::json!({"query": "air"}))
                .expect("valid fixture request"),
        )
        .await
        .expect("tool call should serialize an expected state error");
        let value = serde_json::to_value(result).expect("tool result should serialize");

        assert_eq!(value["isError"], true);
        assert!(value["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("dm_parse_environment")));
    }
}
