use crate::mcp::ToolDefinition;
use crate::result::{structured_error, DomainContent, DomainToolResult, ToolErrorCode};
use crate::state::ServerState;
use crate::tools::{self, ToolExecutionContext};
use crate::{PathPolicy, ServerConfig};
use anyhow::Result;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;
use std::sync::Arc;

#[cfg(all(test, any(windows, target_os = "linux")))]
#[path = "runtime_ownership_tests.rs"]
mod runtime_ownership_tests;

#[derive(Clone)]
pub struct MeridianServer {
    config: Arc<ServerConfig>,
    execution: ToolExecutionContext,
    state: Arc<ServerState>,
}

impl MeridianServer {
    /// Finalize the owned runtime after transport shutdown, with a bounded wait.
    pub async fn shutdown(&self) -> Result<()> {
        self.execution.cancel_owned_requests();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            if self.state.debugger().await.is_some() {
                tools::call_tool(
                    &self.execution,
                    &self.state,
                    "dm_debug_stop",
                    serde_json::json!({}),
                )
                .await?;
            }
            let tracy_active = self
                .state
                .runtime()
                .await
                .execution_lease
                .as_ref()
                .is_some_and(|lease| lease.kind() == "tracy")
                || self.state.tracy_capture().await.integrity_journal.is_some();
            if tracy_active {
                tools::call_tool(
                    &self.execution,
                    &self.state,
                    "dm_tracy_stop",
                    serde_json::json!({}),
                )
                .await?;
            }
            tools::runtime::stop(&self.state, serde_json::json!({})).await?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "runtime shutdown exceeded five seconds; process-owner containment remains active"
            )
        })?
    }

    pub fn new(config: ServerConfig) -> Result<Self> {
        let debugger = (config.debugger_access() == crate::DebuggerAccess::Auxtools)
            .then(|| crate::spaceman::debugger::validate_installation(config.compiler_allowlist()))
            .transpose()?;
        let policy = PathPolicy::from_effective_roots(
            config.effective_roots().to_vec(),
            config.compiler_allowlist().to_vec(),
        )?;
        let dmdoc_helper = config
            .helper_manifest()
            .map(crate::spaceman::docs::optional_verified_dmdoc_helper)
            .transpose()?
            .flatten();
        let tracy = (config.tracy_access() == crate::TracyAccess::Byond)
            .then(|| {
                crate::tracy::TracyInstallation::validate(
                    config
                        .helper_manifest()
                        .expect("Tracy config requires a manifest"),
                )
            })
            .transpose()?;
        let private_state = config
            .state_directory()
            .map(|path| {
                crate::PrivateStateStore::open(path, config.effective_roots()).map(Arc::new)
            })
            .transpose()?;
        let execution = ToolExecutionContext::with_features_and_state(
            config.mode(),
            policy,
            config.rift_build_access(),
            dmdoc_helper,
            debugger,
            tracy,
            private_state,
        );
        Ok(Self {
            config: Arc::new(config),
            execution,
            state: Arc::new(ServerState::new()),
        })
    }

    pub fn tool_names(&self) -> Vec<String> {
        tools::get_tool_definitions_for_runtime(
            self.config.mode(),
            self.config.rift_build_access(),
            self.execution.dmdoc_helper().is_some(),
            self.execution.debugger().is_some(),
            self.execution.tracy().is_some(),
        )
        .into_iter()
        .map(|definition| definition.name)
        .collect()
    }

    fn tools(&self) -> Vec<Tool> {
        tools::get_tool_definitions_for_runtime(
            self.config.mode(),
            self.config.rift_build_access(),
            self.execution.dmdoc_helper().is_some(),
            self.execution.debugger().is_some(),
            self.execution.tracy().is_some(),
        )
        .into_iter()
        .map(|definition| to_sdk_tool(definition, self.config.rift_build_access()))
        .collect()
    }
}

impl ServerHandler for MeridianServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
			.with_server_info(Implementation::new("meridian-mcp", env!("CARGO_PKG_VERSION")))
			.with_instructions("Use text search for literal/file discovery and cross-language exploration. Call dm_parse_environment before tools that inspect parsed DreamMaker source, and reparse after changes. With a loaded snapshot, inspect known symbols directly; use dm_search_context for ranked discovery. For language listings, use detail=compact and follow pagination.next_cursor when complete results are needed. MCP analysis does not replace repository builds.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = tools::call_tool(
            &self.execution,
            self.state.as_ref(),
            &request.name,
            arguments,
        )
        .await
        .unwrap_or_else(|error| DomainToolResult::error(error.to_string()));
        let result = enforce_output_limit(&request.name, result);
        Ok(CallToolResponse::Complete(to_sdk_result(result)))
    }
}

fn enforce_output_limit(name: &str, result: DomainToolResult) -> DomainToolResult {
    let Some(contract) = crate::all_contracts()
        .iter()
        .find(|contract| contract.name == name)
    else {
        return result;
    };
    let output_bytes = result
        .content
        .iter()
        .map(|content| match content {
            DomainContent::Text { text } => text.len(),
        })
        .sum::<usize>();
    if output_bytes <= contract.max_output_bytes {
        return result;
    }

    structured_error(
        ToolErrorCode::LimitExceeded,
        "tool output exceeded its declared transport limit",
        Some("Narrow the request or use the tool's pagination controls.".to_owned()),
        serde_json::json!({
            "tool": name,
            "output_bytes": output_bytes,
            "max_output_bytes": contract.max_output_bytes,
        }),
    )
}

fn to_sdk_tool(definition: ToolDefinition, rift_build: crate::RiftBuildAccess) -> Tool {
    let input_schema = definition
        .input_schema
        .as_object()
        .cloned()
        .unwrap_or_default();
    let contract = crate::all_contracts()
        .iter()
        .find(|contract| contract.name == definition.name);
    let annotations = contract.map(|contract| {
        let external_network =
            contract.effects.network_external && rift_build == crate::RiftBuildAccess::Network;
        ToolAnnotations::new()
            .read_only(
                !contract.effects.writes_files
                    && !contract.effects.spawns_process
                    && !contract.effects.network_loopback
                    && !external_network,
            )
            .destructive(contract.effects.destructive)
            .open_world(external_network)
    });
    let mut tool = Tool::new(definition.name, definition.description, input_schema);
    tool.annotations = annotations;
    tool
}

fn to_sdk_result(result: DomainToolResult) -> CallToolResult {
    let content = result
        .content
        .into_iter()
        .map(|content| match content {
            DomainContent::Text { text } => ContentBlock::text(text),
        })
        .collect();
    if result.is_error == Some(true) {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_annotations_follow_contract_effects() {
        let definition = ToolDefinition {
            name: "dm_stop".to_owned(),
            description: "stop".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let tool = to_sdk_tool(definition, crate::RiftBuildAccess::Disabled);
        let annotations = tool.annotations.expect("contract annotations");

        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
    }

    #[test]
    fn oversized_tool_results_are_replaced_by_bounded_errors() {
        let oversized = DomainToolResult::text("x".repeat(300_000));
        let bounded = enforce_output_limit("dm_stop", oversized);

        assert_eq!(bounded.is_error, Some(true));
        let DomainContent::Text { text } = &bounded.content[0];
        let payload: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["code"], "limit_exceeded");
        assert!(text.len() <= 262_144);
    }
}
