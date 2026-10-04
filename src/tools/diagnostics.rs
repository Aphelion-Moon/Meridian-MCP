use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use tracing::info;

use crate::analysis_snapshot::DiagnosticRecord;
use crate::mcp::ToolResult;
use crate::result::{json_success, ToolMetadata};
use crate::state::ServerState;

const DEFAULT_DIAGNOSTIC_LIMIT: usize = 50;

#[derive(Debug)]
struct DiagnosticQuery {
    file_path: Option<String>,
    severity: Option<String>,
    component: Option<String>,
    rule: Option<String>,
    configured: Option<bool>,
    cursor: usize,
    limit: usize,
}

#[derive(Debug, Serialize)]
struct DiagnosticSummary {
    total: usize,
    by_severity: BTreeMap<String, usize>,
    by_component: BTreeMap<String, usize>,
    by_rule: BTreeMap<String, usize>,
    configured: usize,
    unconfigured: usize,
}

struct DiagnosticQueryResult<'a> {
    diagnostics: Vec<&'a DiagnosticRecord>,
    summary: DiagnosticSummary,
    total_count: usize,
    next_cursor: Option<String>,
}

pub async fn check_errors(
    state: &ServerState,
    args: crate::parameters::CheckErrorsParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let query = DiagnosticQuery {
        file_path: args.file_path,
        severity: args.severity.map(|value| value.as_str().into()),
        component: args.component.map(|value| value.as_str().into()),
        rule: args.rule,
        configured: args.configured,
        cursor: args
            .cursor
            .as_deref()
            .map(str::parse::<usize>)
            .transpose()?
            .unwrap_or(0),
        limit: args.limit.unwrap_or(DEFAULT_DIAGNOSTIC_LIMIT as u64) as usize,
    };

    info!("Reading cached DreamChecker diagnostics...");

    let page = query_diagnostics(&snapshot.diagnostics, &query);
    let has_more = page.next_cursor.is_some();
    let count = page.diagnostics.len();
    let mut metadata = ToolMetadata::complete(Some(snapshot.generation));
    if has_more {
        metadata.truncated = true;
        metadata
            .truncation_reasons
            .push("diagnostic_page_limit".to_owned());
    }

    let result = json!({
        "analysis": {
            "source": "cached_snapshot",
            "environment": snapshot.environment_path,
            "state_generation": snapshot.generation,
            "recomputed": false,
            "refresh_with": "dm_parse_environment"
        },
        "filters": {
            "file_path": query.file_path,
            "severity": query.severity,
            "component": query.component,
            "rule": query.rule,
            "configured": query.configured
        },
        "summary": page.summary,
        "count": count,
        "total_count": page.total_count,
        "diagnostics": page.diagnostics,
        "pagination": {
            "cursor": query.cursor.to_string(),
            "limit": query.limit,
            "next_cursor": page.next_cursor,
            "has_more": has_more
        }
    });

    Ok(json_success(metadata, result))
}

fn query_diagnostics<'a>(
    diagnostics: &'a [DiagnosticRecord],
    query: &DiagnosticQuery,
) -> DiagnosticQueryResult<'a> {
    let normalized_file_filter = query.file_path.as_deref().map(normalize_path_for_match);
    let matching = diagnostics
        .iter()
        .filter(|diagnostic| {
            normalized_file_filter
                .as_deref()
                .is_none_or(|filter| normalize_path_for_match(&diagnostic.file).contains(filter))
                && query
                    .severity
                    .as_deref()
                    .is_none_or(|severity| diagnostic.severity == severity)
                && query
                    .component
                    .as_deref()
                    .is_none_or(|component| diagnostic.component == component)
                && query.rule.as_deref().is_none_or(|rule| {
                    diagnostic
                        .rule
                        .as_deref()
                        .is_some_and(|value| value == rule)
                })
                && query
                    .configured
                    .is_none_or(|configured| diagnostic.configured == configured)
        })
        .collect::<Vec<_>>();
    let summary = summarize_diagnostics(&matching);
    let total_count = matching.len();
    let page_end = query.cursor.saturating_add(query.limit).min(total_count);
    let diagnostics = matching
        .get(query.cursor..page_end)
        .unwrap_or_default()
        .to_vec();
    let next_cursor = (page_end < total_count).then(|| page_end.to_string());

    DiagnosticQueryResult {
        diagnostics,
        summary,
        total_count,
        next_cursor,
    }
}

fn summarize_diagnostics(diagnostics: &[&DiagnosticRecord]) -> DiagnosticSummary {
    let mut summary = DiagnosticSummary {
        total: diagnostics.len(),
        by_severity: BTreeMap::new(),
        by_component: BTreeMap::new(),
        by_rule: BTreeMap::new(),
        configured: 0,
        unconfigured: 0,
    };

    for diagnostic in diagnostics {
        *summary
            .by_severity
            .entry(diagnostic.severity.clone())
            .or_default() += 1;
        *summary
            .by_component
            .entry(diagnostic.component.clone())
            .or_default() += 1;
        *summary
            .by_rule
            .entry(
                diagnostic
                    .rule
                    .clone()
                    .unwrap_or_else(|| "unclassified".to_owned()),
            )
            .or_default() += 1;
        if diagnostic.configured {
            summary.configured += 1;
        } else {
            summary.unconfigured += 1;
        }
    }

    summary
}

fn normalize_path_for_match(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis_snapshot::{DiagnosticNoteRecord, DiagnosticRecord};

    fn diagnostic(rule: &str, severity: &str, component: &str, file: &str) -> DiagnosticRecord {
        DiagnosticRecord {
            rule: Some(rule.to_owned()),
            severity: severity.to_owned(),
            component: component.to_owned(),
            message: format!("{rule} message"),
            file: file.to_owned(),
            line: 1,
            column: 1,
            notes: vec![DiagnosticNoteRecord {
                message: "fixture note".to_owned(),
            }],
            configured: true,
        }
    }

    #[test]
    fn diagnostic_query_filters_summarizes_and_paginates_deterministically() {
        let diagnostics = vec![
            diagnostic("first", "error", "parser", "code\\first.dm"),
            diagnostic("second", "warning", "dreamchecker", "code\\second.dm"),
            diagnostic("third", "warning", "dreamchecker", "code\\second.dm"),
        ];
        let query = DiagnosticQuery {
            file_path: Some("code/second.dm".to_owned()),
            severity: Some("warning".to_owned()),
            component: Some("dreamchecker".to_owned()),
            rule: None,
            configured: Some(true),
            cursor: 1,
            limit: 1,
        };

        let result = query_diagnostics(&diagnostics, &query);

        assert_eq!(result.total_count, 2);
        assert_eq!(result.summary.total, 2);
        assert_eq!(result.summary.by_severity.get("warning"), Some(&2));
        assert_eq!(result.summary.by_component.get("dreamchecker"), Some(&2));
        assert_eq!(result.summary.by_rule.get("second"), Some(&1));
        assert_eq!(result.summary.by_rule.get("third"), Some(&1));
        assert_eq!(result.summary.configured, 2);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].rule.as_deref(), Some("third"));
        assert_eq!(result.next_cursor, None);
    }

    #[test]
    fn diagnostic_query_returns_next_cursor_when_more_results_remain() {
        let diagnostics = vec![
            diagnostic("first", "error", "parser", "code\\first.dm"),
            diagnostic("second", "warning", "dreamchecker", "code\\second.dm"),
        ];
        let query = DiagnosticQuery {
            file_path: None,
            severity: None,
            component: None,
            rule: None,
            configured: None,
            cursor: 0,
            limit: 1,
        };

        let result = query_diagnostics(&diagnostics, &query);

        assert_eq!(result.total_count, 2);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.next_cursor.as_deref(), Some("1"));
    }
}
