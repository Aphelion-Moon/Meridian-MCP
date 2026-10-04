use anyhow::{anyhow, Result};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::OnceLock;

fn hasher() -> &'static RandomState {
    static HASHER: OnceLock<RandomState> = OnceLock::new();
    HASHER.get_or_init(RandomState::new)
}

/// The query includes its tool, normalized filters and ordering version.
pub(crate) struct Cursor {
    binding: u64,
}
impl Cursor {
    pub(crate) fn new(
        snapshot: &crate::analysis_snapshot::AnalysisSnapshot,
        detail: &str,
        query: serde_json::Value,
    ) -> Self {
        Self {
            binding: hasher().hash_one((&snapshot.snapshot_id, detail, query.to_string())),
        }
    }
    pub(crate) fn encode(&self, offset: usize) -> String {
        format!(
            "v2:{offset}:{:016x}",
            hasher().hash_one((self.binding, offset))
        )
    }
    pub(crate) fn decode(&self, token: Option<&str>) -> Result<usize> {
        let Some(token) = token else { return Ok(0) };
        let offset = token
            .split(':')
            .nth(1)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| anyhow!("Invalid query cursor"))?;
        if token != self.encode(offset) {
            return Err(anyhow!("Cursor does not match this snapshot, query, filters, ordering or detail; restart from the first page"));
        }
        Ok(offset)
    }
}
