use anyhow::Result;
use serde_json::json;

use super::ToolExecutionContext;
use crate::mcp::ToolResult;
use crate::state::ServerState;
use crate::{CapabilityMode, RiftBuildAccess};

pub async fn status(context: &ToolExecutionContext, state: &ServerState) -> Result<ToolResult> {
    let snapshot = state.active_snapshot().await;
    let state_generation = state.state_generation().await;
    let runtime = state.runtime().await.status_summary();
    let analysis = snapshot.as_ref().map(|snapshot| {
        let project_root = snapshot
            .project_profile
            .as_ref()
            .map(|profile| profile.root())
            .or_else(|| snapshot.environment_path.parent());
        json!({
            "snapshot_id": snapshot.snapshot_id,
            "generation": snapshot.generation,
            "environment": snapshot.environment_path,
            "source": "cached_snapshot",
            "completeness": "complete",
            "disk_state": "unknown",
            "parsed": true,
            "state_generation": snapshot.generation,
            "environment_path": snapshot.environment_path,
            "project_root": project_root,
            "spacemandmm_revision": snapshot.spacemandmm_revision,
            "spacemandmm_local_patch": crate::capabilities::SPACEMANDMM_LOCAL_PATCH,
            "spacemandmm_local_patch_sha256": crate::capabilities::SPACEMANDMM_LOCAL_PATCH_SHA256,
        })
    });
    let analysis = analysis.unwrap_or_else(|| {
        json!({
            "parsed": false,
            "state_generation": state_generation,
            "environment_path": null,
            "project_root": null,
            "spacemandmm_revision": null,
        })
    });

    Ok(ToolResult::text(
        serde_json::to_string_pretty(&json!({
            "mcp_build": crate::build_identity::current(),
            "mode": match context.mode() {
                CapabilityMode::Analysis => "analysis",
                CapabilityMode::Development => "development",
            },
            "optional_capabilities": {
                "rift_build": match context.rift_build_access() {
                    RiftBuildAccess::Disabled => "disabled",
                    RiftBuildAccess::Offline => "offline",
                    RiftBuildAccess::Network => "network",
                },
                "documentation": context.dmdoc_helper().is_some(),
                "debugger": context.debugger().is_some(),
                "tracy": context.tracy().is_some(),
            },
            "containment": context.policy().status(),
            "private_state": {
                "ready": context.private_state().is_some(),
                "contents_exposed": false,
                "runtime_integrity_recovery": context.integrity_recovery(),
            },
            "analysis": analysis,
            "runtime": runtime,
        }))
        .expect("server status serialization cannot fail"),
    ))
}
