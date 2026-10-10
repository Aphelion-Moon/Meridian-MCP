use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::analysis_snapshot::AnalysisSnapshot;
use crate::result::{json_success, json_success_compact, ToolMetadata, ToolResult};

pub(super) struct Page {
    offset: usize,
    limit: usize,
    compact: bool,
    cursor: crate::cursor::Cursor,
}

impl Page {
    pub(super) fn new(
        snapshot: &AnalysisSnapshot,
        limit: Option<u64>,
        detail: &str,
        cursor: Option<&str>,
        query: Value,
    ) -> Result<Self> {
        let limit = limit.unwrap_or(100) as usize;
        let compact = detail == "compact";
        let mut page = Self {
            offset: 0,
            limit,
            compact,
            cursor: crate::cursor::Cursor::new(snapshot, detail, json!(["source_order_v1", query])),
        };
        if let Some(cursor) = cursor {
            page.offset = page.cursor.decode(Some(cursor))?;
        }
        Ok(page)
    }

    fn cursor(&self, offset: usize) -> String {
        self.cursor.encode(offset)
    }

    pub(super) fn respond<T: crate::outputs::language::ProjectRow, D: Serialize>(
        &self,
        snapshot: &AnalysisSnapshot,
        matching: impl Iterator<Item = T>,
        total_count: usize,
        build: impl FnOnce(Vec<T::Output>, crate::outputs::language::PageData<T::Output>) -> D,
    ) -> Result<ToolResult> {
        use crate::outputs::language::PageRow;
        if self.offset > total_count {
            return Err(anyhow!("Cursor offset exceeds the result count"));
        }
        let mut budget = crate::outputs::budget::Budget::default();
        let mut rows = Vec::new();
        for row in matching.skip(self.offset).take(self.limit) {
            if !rows.is_empty() && crate::result::encoded_bytes(&row, budget.bytes).is_none() {
                break;
            }
            let mut row_budget = crate::outputs::budget::Budget {
                bytes: budget.bytes,
                source_lines: 0,
            };
            let projected = row.project(&mut row_budget);
            budget.bytes = budget.bytes.saturating_sub(
                crate::result::encoded_bytes(&projected, usize::MAX)
                    .expect("projected row serialization"),
            );
            rows.push(projected);
        }
        let count = rows.len();
        let end = self.offset + count;
        let has_more = end < total_count;
        let shared = self.compact.then(|| T::Output::factor(&mut rows));
        let data = build(
            rows,
            crate::outputs::language::PageData {
                count,
                total_count,
                detail: if self.compact { "compact" } else { "full" },
                pagination: crate::outputs::language::Pagination {
                    offset: self.offset,
                    limit: self.limit,
                    next_cursor: has_more.then(|| self.cursor(end)),
                },
                shared,
            },
        );
        let mut metadata = ToolMetadata::for_snapshot(snapshot);
        metadata.truncated = has_more;
        if has_more {
            metadata.truncation_reasons.push(
                if count < self.limit {
                    "language_page_bytes"
                } else {
                    "language_page_limit"
                }
                .into(),
            );
        }
        Ok(if self.compact {
            json_success_compact(metadata, data)
        } else {
            json_success(metadata, data)
        })
    }
}
