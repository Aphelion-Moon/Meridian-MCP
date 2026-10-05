use anyhow::Result;

use super::ToolExecutionContext;
use crate::mcp::ToolResult;
use crate::state::ServerState;
use crate::{CapabilityMode, RiftBuildAccess};

pub async fn status(context: &ToolExecutionContext, state: &ServerState) -> Result<ToolResult> {
    let snapshot = state.active_snapshot().await;
    let state_generation = state.state_generation().await;
    let runtime = state.runtime().await.status_summary();
    use crate::outputs::*;
    let analysis = match snapshot {
        Some(snapshot) => AnalysisStatus::Parsed(ParsedStatus {
            identity: snapshot.identity(),
            parsed: true,
            environment_path: snapshot.environment_path.clone(),
            project_root: snapshot
                .project_profile
                .as_ref()
                .map(|p| p.root())
                .or_else(|| snapshot.environment_path.parent())
                .map(std::path::Path::to_owned),
            spacemandmm_revision: snapshot.spacemandmm_revision,
            spacemandmm_local_patch: crate::capabilities::SPACEMANDMM_LOCAL_PATCH,
            spacemandmm_local_patch_sha256: crate::capabilities::SPACEMANDMM_LOCAL_PATCH_SHA256,
        }),
        None => AnalysisStatus::Unparsed(UnparsedStatus {
            parsed: false,
            state_generation,
            environment_path: None,
            project_root: None,
            spacemandmm_revision: None,
        }),
    };
    Ok(crate::result::projection(
        ServerStatusOutput {
            mcp_build: crate::build_identity::current().clone(),
            mode: match context.mode() {
                CapabilityMode::Analysis => "analysis",
                CapabilityMode::Development => "development",
            },
            optional_capabilities: OptionalCapabilities {
                rift_build: match context.rift_build_access() {
                    RiftBuildAccess::Disabled => "disabled",
                    RiftBuildAccess::Offline => "offline",
                    RiftBuildAccess::Network => "network",
                },
                documentation: context.dmdoc_helper().is_some(),
                debugger: context.debugger().is_some(),
                tracy: context.tracy().is_some(),
            },
            containment: context.policy().status(),
            private_state: PrivateStateStatus {
                ready: context.private_state().is_some(),
                contents_exposed: false,
                runtime_integrity_recovery: context.integrity_recovery().to_vec(),
            },
            analysis,
            runtime,
        },
        true,
        false,
    ))
}
