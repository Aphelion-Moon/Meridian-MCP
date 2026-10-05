use super::ToolExecutionContext;
use crate::mcp::ToolResult;
use crate::memory_evidence::{MemoryCompareRequest, MemoryRequest};
use crate::state::ServerState;
use anyhow::Result;
pub(super) enum MemoryInput {
    Summary(MemoryRequest),
    Compare(MemoryCompareRequest),
}

pub async fn run(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: MemoryInput,
) -> Result<ToolResult> {
    let policy = context.policy().clone();
    let result = state
        .run_asset_job(move || match args {
            MemoryInput::Summary(request) => Ok(crate::result::projection(
                crate::memory_evidence::summarize(&policy, request)?,
                true,
                false,
            )),
            MemoryInput::Compare(request) => Ok(crate::result::projection(
                crate::memory_evidence::compare(&policy, request)?,
                true,
                false,
            )),
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
