use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::analysis_snapshot::AnalysisSnapshot;
use crate::limits::ServerLimits;
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

    pub(super) fn respond<T: Serialize>(
        &self,
        snapshot: &AnalysisSnapshot,
        key: &str,
        matching: impl Iterator<Item = T>,
        total_count: usize,
        mut data: Map<String, Value>,
    ) -> Result<ToolResult> {
        if self.offset > total_count {
            return Err(anyhow!("Cursor offset exceeds the result count"));
        }
        // Reserve half the transport ceiling for metadata and enclosing JSON.
        // Account for full rows even in compact mode: no unbounded intermediate
        // materialization, and every successful page makes forward progress.
        let budget = ServerLimits::default().max_result_bytes / 2;
        let mut bytes = 0;
        let mut rows = Vec::new();
        let mut byte_limited = false;
        for row in matching.skip(self.offset).take(self.limit) {
            let value = serde_json::to_value(row)?;
            let serialized = serde_json::to_string_pretty(&value)?;
            let cost = serialized.len() + serialized.lines().count() * 8 + 2;
            if bytes + cost > budget {
                if rows.is_empty() {
                    return Err(anyhow!(
                        "A single language-query row exceeds the response byte budget"
                    ));
                }
                byte_limited = true;
                break;
            }
            bytes += cost;
            rows.push(value);
        }
        let count = rows.len();
        let end = self.offset + count;
        let has_more = end < total_count;
        if self.compact {
            // A row is losslessly reconstructed by overlaying it on `shared`.
            // Keep source positions in each row so the listing remains readable.
            let mut shared = if rows.len() > 1 {
                rows[0].as_object().cloned().unwrap_or_default()
            } else {
                Map::new()
            };
            shared.retain(|key, value| {
                key != "line"
                    && key != "column"
                    && rows[1..].iter().all(|row| row.get(key) == Some(value))
            });
            for row in &mut rows {
                if let Some(row) = row.as_object_mut() {
                    row.retain(|key, _| !shared.contains_key(key));
                }
            }
            data.insert("shared".into(), Value::Object(shared));
        }
        data.insert(key.to_owned(), Value::Array(rows));
        data.insert("count".into(), json!(count));
        data.insert("total_count".into(), json!(total_count));
        data.insert(
            "detail".into(),
            json!(if self.compact { "compact" } else { "full" }),
        );
        data.insert(
            "pagination".into(),
            json!({
                "offset":self.offset, "limit":self.limit,
                "next_cursor":has_more.then(|| self.cursor(end)),
            }),
        );
        let mut metadata = ToolMetadata::for_snapshot(snapshot);
        metadata.truncated = has_more;
        if has_more {
            metadata.truncation_reasons.push(
                if byte_limited {
                    "language_page_bytes"
                } else {
                    "language_page_limit"
                }
                .to_owned(),
            );
        }
        Ok(if self.compact {
            json_success_compact(metadata, data)
        } else {
            json_success(metadata, data)
        })
    }
}
