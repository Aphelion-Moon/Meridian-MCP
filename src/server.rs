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
    execution: ToolExecutionContext,
    state: Arc<ServerState>,
    catalog: Arc<[Tool]>,
    legacy_catalog: Arc<[Tool]>,
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
            tools::runtime::stop(&self.state, crate::parameters::StopParams::default()).await?;
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
        let catalog = execution
            .definitions()
            .into_iter()
            .map(|definition| to_sdk_tool(definition, config.rift_build_access()))
            .collect::<Vec<_>>();
        let legacy_catalog = catalog
            .iter()
            .cloned()
            .map(|mut tool| {
                tool.output_schema = None;
                tool
            })
            .collect::<Vec<_>>();
        Ok(Self {
            legacy_catalog: Arc::from(legacy_catalog),
            catalog: Arc::from(catalog),
            execution,
            state: Arc::new(ServerState::new()),
        })
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.catalog
            .iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }
    fn tools(&self) -> Vec<Tool> {
        self.catalog.to_vec()
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
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let structured = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2025_06_18);
        let result = ListToolsResult::with_all_items(if structured {
            self.tools()
        } else {
            self.legacy_catalog.to_vec()
        });
        let modern = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        Ok(if modern {
            result
                .with_ttl_ms(60_000)
                .with_cache_scope(rmcp::model::CacheScope::Private)
        } else {
            result
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog.iter().find(|tool| tool.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = tools::call_tool(
            &self.execution,
            self.state.as_ref(),
            &request.name,
            arguments,
        )
        .await
        .unwrap_or_else(crate::result::tool_error);
        let structured = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2025_06_18);
        let result = enforce_output_limit(&request.name, result, structured);
        let sdk = sdk_result_for_version(result, structured);
        Ok(CallToolResponse::Complete(sdk))
    }
}

fn enforce_output_limit(
    name: &str,
    result: DomainToolResult,
    structured: bool,
) -> DomainToolResult {
    let Some(contract) = crate::all_contracts()
        .iter()
        .find(|contract| contract.name == name)
    else {
        return result;
    };
    let output_bytes = crate::result::encoded_bytes(
        &sdk_result_for_version(result.clone(), structured),
        usize::MAX,
    )
    .unwrap_or(usize::MAX);
    if output_bytes <= contract.max_output_bytes {
        return result;
    }
    if let Some(outcome) = result.outcome {
        let mut bounded = crate::result::projection(
            crate::result::ReducedMutation {
                outcome: *outcome,
                truncated: true,
                response_omissions: vec![
                    "optional response details omitted to preserve the operation outcome".into(),
                ],
            },
            false,
            result.is_error == Some(true),
        );
        bounded.meta = result.meta;
        assert!(
            crate::result::encoded_bytes(
                &sdk_result_for_version(bounded.clone(), structured),
                contract.max_output_bytes
            )
            .is_some(),
            "mutation core exceeds declared response budget"
        );
        return bounded;
    }
    let mut bounded = structured_error(
        ToolErrorCode::LimitExceeded,
        "tool output exceeded its declared transport limit",
        Some("Narrow the request or use the tool's pagination controls.".to_owned()),
        serde_json::json!({
            "tool": name,
            "output_bytes": output_bytes,
            "max_output_bytes": contract.max_output_bytes,
        }),
    );
    bounded.meta = result.meta;
    bounded
}

fn sdk_result_for_version(result: DomainToolResult, structured: bool) -> CallToolResult {
    let mut result = to_sdk_result(result);
    if !structured {
        result.structured_content = None;
    }
    result
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
                    && !external_network
                    && !contract.effects.project_behavior,
            )
            .destructive(contract.effects.destructive)
            .open_world(external_network || contract.effects.project_behavior)
    });
    let mut tool = Tool::new(definition.name, definition.description, input_schema);
    tool.output_schema = definition.output_schema.as_object().cloned().map(Arc::new);
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
    let mut sdk = if result.is_error == Some(true) {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    sdk.structured_content = result.structured_content;
    sdk.meta = result.meta.map(|meta| {
        serde_json::Map::from_iter([(
            "analysis".to_owned(),
            serde_json::to_value(meta.analysis).expect("analysis identity serialization"),
        )])
        .into()
    });
    sdk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn semantic_error_identity_survives_sdk_conversion_and_output_replacement() {
        let state = ServerState::new();
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/language/fixture.dme");
        let context = ToolExecutionContext::new(
            crate::CapabilityMode::Analysis,
            PathPolicy::new(vec![fixture.parent().unwrap().to_owned()], vec![]).unwrap(),
        );
        crate::tools::call_tool(
            &context,
            &state,
            "dm_parse_environment",
            serde_json::json!({"dme_path":fixture}),
        )
        .await
        .unwrap();
        let analysis = state.snapshot().await.unwrap().identity();
        let result = DomainToolResult::error("legacy error text").with_analysis(analysis.clone());
        let sdk = serde_json::to_value(to_sdk_result(result)).unwrap();
        assert_eq!(sdk["isError"], true);
        assert_eq!(sdk["content"][0]["text"], "legacy error text");
        assert_eq!(
            sdk["_meta"]["analysis"],
            serde_json::to_value(&analysis).unwrap()
        );
        let error: anyhow::Error = crate::result::SemanticCallError {
            error: anyhow::anyhow!("legacy failed query"),
            analysis: analysis.clone(),
        }
        .into();
        let sdk = serde_json::to_value(to_sdk_result(crate::result::tool_error(error))).unwrap();
        assert_eq!(sdk["content"][0]["text"], "legacy failed query");
        assert_eq!(
            sdk["_meta"]["analysis"]["snapshot_id"],
            analysis.snapshot_id
        );
        let limited = enforce_output_limit(
            "dm_stop",
            DomainToolResult::text("x".repeat(300000)).with_analysis(analysis.clone()),
            true,
        );
        assert_eq!(limited.is_error, Some(true));
        assert_eq!(limited.meta.unwrap().analysis, analysis);
    }

    #[test]
    fn sdk_annotations_follow_contract_effects() {
        let definition = ToolDefinition {
            name: "dm_stop".to_owned(),
            description: "stop".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: serde_json::json!({"type":"object"}),
        };
        let tool = to_sdk_tool(definition, crate::RiftBuildAccess::Disabled);
        let annotations = tool.annotations.expect("contract annotations");

        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
    }

    #[test]
    fn project_code_effects_are_advisory_and_independent_of_rift_network_access() {
        for name in ["dm_topic", "dm_debug_evaluate"] {
            let definition = tools::get_tool_definitions()
                .into_iter()
                .find(|tool| tool.name == name)
                .unwrap();
            let annotations = to_sdk_tool(definition, crate::RiftBuildAccess::Disabled)
                .annotations
                .unwrap();
            assert_eq!(annotations.read_only_hint, Some(false));
            assert_eq!(annotations.destructive_hint, Some(true));
            assert_eq!(annotations.open_world_hint, Some(true));
        }
    }

    #[test]
    fn oversized_tool_results_are_replaced_by_bounded_errors() {
        let oversized = DomainToolResult::text("x".repeat(300_000));
        let bounded = enforce_output_limit("dm_stop", oversized, true);

        assert_eq!(bounded.is_error, Some(true));
        let DomainContent::Text { text } = &bounded.content[0];
        let payload: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["code"], "limit_exceeded");
        assert!(text.len() <= 262_144);
    }
    #[test]
    fn sdk_body_budget_keeps_installed_batch_outcomes_with_escaping_and_duplication() {
        let core = crate::result::MutationOutcome {
            operation_ran: true,
            cleanup_complete: false,
            recovery_required: true,
            outputs: (0..512)
                .map(|index| crate::result::OutputMutation {
                    request_index: index,
                    installed: index % 2 == 0,
                    path_preview: Some("\\u{1}".repeat(42)),
                    sha256: Some("a".repeat(64)),
                    cleanup_complete: false,
                    backup_preview: Some("\\u{1}".repeat(42)),
                })
                .collect(),
            ..Default::default()
        };
        let result = crate::result::projection(
            serde_json::json!({"optional":"\\u{1}🛰".repeat(300000)}),
            true,
            true,
        )
        .with_outcome(core.clone());
        let bounded = enforce_output_limit("dm_render_maps", result, true);
        let sdk = to_sdk_result(bounded);
        assert!(crate::result::encoded_bytes(&sdk, 1_048_576).is_some());
        assert_eq!(sdk.is_error, Some(true));
        let structured = sdk.structured_content.as_ref().unwrap();
        assert_eq!(structured["outcome"], serde_json::to_value(core).unwrap());
        let rmcp::model::ContentBlock::Text(text) = &sdk.content[0] else {
            panic!("compatibility text")
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text.text).unwrap(),
            *structured
        );
    }
    #[test]
    fn legacy_body_cap_excludes_omitted_native_duplication() {
        let projection = crate::result::projection(
            serde_json::json!({"content":"x".repeat(600_000)}),
            false,
            false,
        );
        let legacy = enforce_output_limit("dm_get_type", projection.clone(), false);
        assert_ne!(legacy.is_error, Some(true));
        let sdk = sdk_result_for_version(legacy, false);
        assert!(sdk.structured_content.is_none());
        assert!(crate::result::encoded_bytes(&sdk, 1_048_576).is_some());
        let modern = enforce_output_limit("dm_get_type", projection, true);
        assert_eq!(modern.is_error, Some(true));
        assert_eq!(modern.structured_content.unwrap()["code"], "limit_exceeded");
    }
}
