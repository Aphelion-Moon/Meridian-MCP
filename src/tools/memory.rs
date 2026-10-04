use super::ToolExecutionContext;
use crate::mcp::ToolResult;
use crate::memory_evidence::{MemoryCompareRequest, MemoryRequest};
use crate::state::ServerState;
use anyhow::Result;
use serde_json::Value;

pub async fn run(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
    comparison: bool,
) -> Result<ToolResult> {
    let policy = context.policy().clone();
    let result = state
        .run_asset_job(move || {
            let value = if comparison {
                crate::memory_evidence::compare(
                    &policy,
                    serde_json::from_value::<MemoryCompareRequest>(args)?,
                )?
            } else {
                serde_json::to_value(crate::memory_evidence::summarize(
                    &policy,
                    serde_json::from_value::<MemoryRequest>(args)?,
                )?)?
            };
            Ok(ToolResult::text(serde_json::to_string_pretty(&value)?))
        })
        .await;
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let code = if error.to_string().starts_with("evidence_identity_mismatch") {
                "evidence_identity_mismatch"
            } else {
                "invalid_input"
            };
            Ok(ToolResult::structured_error(code, error.to_string(), "Use bounded schema-2 memory evidence from authorized roots and compatible recorded identities."))
        }
    }
}
