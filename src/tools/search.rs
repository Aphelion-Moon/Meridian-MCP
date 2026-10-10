use anyhow::Result;

use crate::mcp::ToolResult;
use crate::search::{SearchIndex, SearchRequest, SymbolKind};
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
    let mut budget = crate::outputs::budget::Budget::default();
    let mut results = Vec::new();
    let mut complete = true;
    for hit in &execution.hits {
        if budget.bytes < 256 {
            complete = false;
            break;
        }
        let document = hit.document;
        let mut omissions = crate::outputs::Omissions::default();
        let docs = budget.document(Some(&document.docs), "docs", &mut omissions);
        let mut parameters = Vec::new();
        for parameter in document
            .parameters
            .iter()
            .take(crate::outputs::budget::MEMBER_WORK)
        {
            if budget.bytes < 64 {
                break;
            }
            budget.bytes = budget.bytes.saturating_sub(16);
            parameters.push(budget.text(parameter, 4096, "parameters", &mut omissions));
        }
        if parameters.len() < document.parameters.len() {
            omissions
                .fields
                .insert("parameters".into(), "aggregate response/work limit".into());
        }
        let source = if include_source {
            budget.source(document.source.as_ref(), max_source_lines)
        } else {
            crate::outputs::SourceFields::default()
        };
        results.push(crate::outputs::SearchHit {
            score: (hit.score * 1000.0).round() / 1000.0,
            kind: document.kind.as_str(),
            symbol: document.symbol.clone(),
            name: document.name.clone(),
            type_path: document.type_path.clone(),
            implementation_owner: document.implementation_owner.clone(),
            declaration_owner: document.declaration_owner.clone(),
            parent: document.parent.clone(),
            file: document.file.clone(),
            line: document.line,
            column: document.column,
            docs,
            parameters,
            override_index: document.override_index,
            override_count: document.override_count,
            source,
            field_omissions: omissions,
        });
    }
    let count = results.len();
    crate::result::analysis_text(
        &snapshot,
        crate::outputs::SearchContextData {
            query: query.into(),
            query_terms: SearchIndex::query_terms(query),
            indexed_documents: index.len(),
            count,
            results,
            state_generation: snapshot.generation,
            source_origin: "analysis_snapshot",
            source_line_limit: max_source_lines,
            retrieval: crate::outputs::RetrievalStats {
                mode: "lexical",
                algorithm: "bm25",
                candidates_considered: execution.candidates_considered,
                documents_scored: execution.documents_scored,
            },
            evaluation_complete: complete,
        },
    )
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
