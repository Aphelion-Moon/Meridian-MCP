use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy, RiftBuildAccess};
use serde_json::json;

fn message(result: &ToolResult) -> &str {
    match &result.content[0] {
        ToolContent::Text { text } => text,
    }
}

fn payload(result: &ToolResult) -> serde_json::Value {
    serde_json::from_str(message(result)).expect("tool policy errors should be structured JSON")
}

#[tokio::test]
async fn rift_compile_cannot_broaden_the_startup_network_ceiling() {
    let root =
        std::env::temp_dir().join(format!("meridian-mcp-rift-ceiling-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolExecutionContext::with_rift_build(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
        RiftBuildAccess::Offline,
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "rift_compile",
        json!({"network_mode": "allow"}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    #[cfg(windows)]
    assert!(message(&result).contains("network_mode_denied"));
    #[cfg(not(windows))]
    assert!(message(&result).contains("unsupported_platform"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn rift_compile_rejects_zero_duration_limits() {
    let root =
        std::env::temp_dir().join(format!("meridian-mcp-rift-timeout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolExecutionContext::with_rift_build(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
        RiftBuildAccess::Offline,
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "rift_compile",
        json!({"timeout_ms": 0}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    #[cfg(windows)]
    assert!(message(&result).contains("invalid_arguments"));
    #[cfg(not(windows))]
    assert!(message(&result).contains("unsupported_platform"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn analysis_mode_rejects_active_tools() {
    let root = std::env::temp_dir().join(format!("meridian-mcp-active-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let result = call_tool(&context, &ServerState::new(), "dm_compile", json!({}))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(message(&result).contains("tool_not_available"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn analysis_mode_policy_error_uses_the_shared_error_shape() {
    let root =
        std::env::temp_dir().join(format!("meridian-mcp-active-shape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );

    let result = call_tool(&context, &ServerState::new(), "dm_compile", json!({}))
        .await
        .unwrap();
    let value = payload(&result);

    assert_eq!(value["code"], "tool_not_available");
    assert_eq!(value["details"]["tool"], "dm_compile");
    assert_eq!(value["details"]["mode"], "analysis");
    assert!(value["recovery"].is_string());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn path_policy_tool_error_reports_effective_roots_and_source() {
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-active-policy-context-{}",
        std::process::id()
    ));
    let outside = std::env::temp_dir().join(format!(
        "meridian-mcp-active-policy-outside-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&outside);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&outside, "#include \"fixture.dm\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_parse_environment",
        json!({"dme_path": outside}),
    )
    .await
    .unwrap();
    let value = payload(&result);

    assert_eq!(value["code"], "path_outside_workspace");
    assert_eq!(
        value["details"]["containment_mode"],
        "immutable_startup_roots"
    );
    assert_eq!(
        value["details"]["policy_source"],
        "server_startup_configuration"
    );
    assert_eq!(
        value["details"]["effective_roots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn development_mode_rejects_unlisted_compilers_and_implicit_overwrite() {
    let root = std::env::temp_dir().join(format!("meridian-mcp-active-dev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dme = root.join("fixture.dme");
    let compiler = root.join("unlisted.exe");
    let dmm = root.join("fixture.dmm");
    let png = root.join("fixture.png");
    for path in [&dme, &compiler, &dmm, &png] {
        std::fs::write(path, "fixture").unwrap();
    }
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let compiler_result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme, "compiler_path": compiler}),
    )
    .await
    .unwrap();
    assert!(message(&compiler_result).contains("executable_not_allowed"));
    let render_result = call_tool(
        &context,
        &ServerState::new(),
        "dm_render_map",
        json!({"dmm_path": dmm, "output_path": png}),
    )
    .await
    .unwrap();
    assert!(message(&render_result).contains("output_exists"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn standard_runtime_rejects_unverified_artifacts_before_process_discovery() {
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-runtime-provenance-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dmb = root.join("fixture.dmb");
    std::fs::write(&dmb, "unmanaged fixture").unwrap();
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    let private_path = root.with_extension("private");
    std::fs::create_dir_all(&private_path).unwrap();
    let store = std::sync::Arc::new(
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
    );
    let context = ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy,
        RiftBuildAccess::Disabled,
        None,
        None,
        None,
        Some(store),
    );

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_run",
        json!({"dmb_path": dmb, "require_verified_provenance": true}),
    )
    .await
    .unwrap();
    let value = payload(&result);

    assert_eq!(result.is_error, Some(true));
    assert_eq!(value["message"], "build_provenance_unavailable");
    assert_eq!(value["details"]["provenance"]["status"], "unverified");
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(private_path).unwrap();
}
