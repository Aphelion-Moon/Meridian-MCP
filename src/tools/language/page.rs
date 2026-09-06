use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::OnceLock;

use crate::analysis_snapshot::AnalysisSnapshot;
use crate::limits::ServerLimits;
use crate::result::{json_success, json_success_compact, ToolMetadata, ToolResult};

pub(super) struct Page {
    offset: usize,
    limit: usize,
    compact: bool,
    binding: u64,
}

fn cursor_hash() -> &'static RandomState {
    // A process-local seed prevents reusing cursors after a server restart.
    // Cursors provide consistency, not authorization; path policy still applies.
    static HASHER: OnceLock<RandomState> = OnceLock::new();
    HASHER.get_or_init(RandomState::new)
}

impl Page {
    pub(super) fn new(
        snapshot: &AnalysisSnapshot,
        args: &Value,
        query: Value,
        maximum: usize,
    ) -> Result<Self> {
        let limit = crate::tools::bounded_u64(args, "limit", 100, 1, maximum as u64)? as usize;
        let compact = match args.get("detail") {
            None => false,
            Some(Value::String(detail)) if detail == "full" => false,
            Some(Value::String(detail)) if detail == "compact" => true,
            _ => return Err(anyhow!("detail must be full or compact")),
        };
        let mut page = Self {
            offset: 0,
            limit,
            compact,
            binding: cursor_hash().hash_one((
                snapshot.instance_id,
                snapshot.generation,
                query.to_string(),
            )),
        };
        if let Some(value) = args.get("cursor") {
            let cursor = value
                .as_str()
                .ok_or_else(|| anyhow!("cursor must be an opaque string from the previous page"))?;
            let offset = cursor
                .split(':')
                .nth(1)
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("Invalid language-query cursor"))?;
            if cursor != page.cursor(offset) {
                return Err(anyhow!("Cursor does not match this query or parsed snapshot; restart from the first page"));
            }
            page.offset = offset;
        }
        Ok(page)
    }

    fn cursor(&self, offset: usize) -> String {
        format!(
            "v1:{offset}:{:016x}",
            cursor_hash().hash_one((self.binding, offset))
        )
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
        let mut metadata = ToolMetadata::complete(Some(snapshot.generation));
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
