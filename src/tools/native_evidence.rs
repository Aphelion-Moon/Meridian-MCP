use super::ToolExecutionContext;
use crate::mcp::ToolResult;
use crate::state::ServerState;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::Value;

pub async fn summary(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let request: crate::native_evidence::model::NativeEvidenceRequest =
        serde_json::from_value(args)?;
    let evidence = evidence_context(context);
    state
        .run_asset_job(move || {
            let result = crate::native_evidence::summarize_run(&evidence, request)?;
            Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
        })
        .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareRequest {
    runs: Vec<crate::native_evidence::model::NativeEvidenceRequest>,
}

pub async fn compare(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let request: CompareRequest = serde_json::from_value(args)?;
    let evidence = evidence_context(context);
    let result = state
        .run_asset_job(move || {
            let result = crate::native_evidence::compare_runs(&evidence, request.runs)?;
            Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
        })
        .await;
    match result {
        Ok(result) => Ok(result),
        Err(error) if error.to_string().contains("evidence_identity_mismatch") => {
            Ok(ToolResult::structured_error(
                "evidence_identity_mismatch",
                error.to_string(),
                "Use runs with the same verified managed build and workload identity.",
            ))
        }
        Err(error) => Err(anyhow!(error)),
    }
}

fn evidence_context(
    context: &ToolExecutionContext,
) -> crate::native_evidence::NativeEvidenceContext {
    crate::native_evidence::NativeEvidenceContext {
        policy: context.policy().clone(),
        provenance: context.build_provenance_arc(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ServerLimits;
    use crate::{CapabilityMode, PathPolicy};
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn evidence_tools_share_blocking_job_admission() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![root.to_owned()], Vec::new()).unwrap(),
        );
        let state = Arc::new(ServerState::with_limits(ServerLimits {
            max_blocking_jobs: 1,
            ..Default::default()
        }));
        let worker_state = Arc::clone(&state);
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let worker = tokio::spawn(async move {
            worker_state
                .run_asset_job(move || {
                    let _ = started.send(());
                    let _ = released.recv();
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        let memory = json!({"evidence_path": root.join("Cargo.toml")});
        let mut bypassed = Vec::new();
        for (name, args) in [
            ("dm_native_evidence_summary", json!({"artifacts": []})),
            ("dm_native_evidence_compare", json!({"runs": []})),
            ("dm_memory_summary", memory.clone()),
            (
                "dm_memory_compare",
                json!({"baseline": memory, "current": memory}),
            ),
        ] {
            if tokio::time::timeout(
                Duration::from_millis(50),
                crate::tools::call_tool(&context, &state, name, args),
            )
            .await
            .is_ok()
            {
                bypassed.push(name);
            }
        }
        drop(release);
        worker.await.unwrap().unwrap();
        assert!(
            bypassed.is_empty(),
            "evidence tools bypassed admission: {bypassed:?}"
        );
    }
}
