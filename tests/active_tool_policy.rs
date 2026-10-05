use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy, RiftBuildAccess, ToolProfile};
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
async fn default_startup_profile_is_reported_in_typed_status() {
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(
            vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))],
            vec![],
        )
        .unwrap(),
    );
    let result = call_tool(&context, &ServerState::new(), "dm_server_status", json!({}))
        .await
        .unwrap();
    assert_eq!(payload(&result)["tool_profile"], "all");
    assert_eq!(payload(&result), result.structured_content.unwrap());
}

#[tokio::test]
async fn startup_profiles_reject_hidden_calls_before_decoding_and_preserve_other_gates() {
    let policy = PathPolicy::new(
        vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))],
        vec![],
    )
    .unwrap();
    for (profile, mode, hidden) in [
        (
            ToolProfile::Code,
            CapabilityMode::Development,
            "dm_dmi_info",
        ),
        (ToolProfile::Code, CapabilityMode::Development, "dm_topic"),
        (
            ToolProfile::Assets,
            CapabilityMode::Development,
            "dm_get_type",
        ),
        (
            ToolProfile::Assets,
            CapabilityMode::Development,
            "rift_compile",
        ),
        (
            ToolProfile::Runtime,
            CapabilityMode::Development,
            "dm_get_var",
        ),
        (ToolProfile::Runtime, CapabilityMode::Analysis, "dm_run"),
        (
            ToolProfile::Code,
            CapabilityMode::Development,
            "dm_generate_docs",
        ),
        (
            ToolProfile::Runtime,
            CapabilityMode::Development,
            "dm_debug_launch",
        ),
        (
            ToolProfile::Runtime,
            CapabilityMode::Development,
            "dm_tracy_launch",
        ),
    ] {
        let context = ToolExecutionContext::new(mode, policy.clone()).with_tool_profile(profile);
        let state = ServerState::new();
        let rejected = call_tool(&context, &state, hidden, json!({"unknown":true}))
            .await
            .unwrap();
        assert_eq!(rejected.is_error, Some(true));
        assert_eq!(
            payload(&rejected)["code"],
            "tool_not_available",
            "{profile:?} {hidden}"
        );
        assert_eq!(
            payload(&rejected)["details"]["tool_profile"],
            serde_json::to_value(profile).unwrap()
        );
        let status = call_tool(&context, &state, "dm_server_status", json!({}))
            .await
            .unwrap();
        assert_eq!(
            payload(&status)["tool_profile"],
            serde_json::to_value(profile).unwrap()
        );
        assert_eq!(
            status.structured_content.as_ref().unwrap()["tool_profile"],
            payload(&status)["tool_profile"]
        );
    }
    let assets = ToolExecutionContext::new(CapabilityMode::Analysis, policy)
        .with_tool_profile(ToolProfile::Code)
        .with_tool_profile(ToolProfile::Assets);
    let admitted = call_tool(&assets, &ServerState::new(), "dm_dmi_info", json!({}))
        .await
        .unwrap();
    assert_eq!(payload(&admitted)["code"], "invalid_input");
}

#[cfg(unix)]
#[tokio::test]
async fn non_unicode_canonical_paths_cannot_change_during_typed_dispatch() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let root = std::env::temp_dir().join(format!("meridian-wire-path-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let native = root.join(std::ffi::OsString::from_vec(b"map-\xff.dmm".to_vec()));
    std::fs::write(&native, "native path A").unwrap();
    std::fs::write(native.to_string_lossy().as_ref(), "different path B").unwrap();
    let alias = root.join("alias.dmm");
    symlink(&native, &alias).unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_map_info",
        json!({"dmm_path":alias}),
    )
    .await
    .unwrap();
    let error = payload(&result);
    assert_eq!(error["code"], "unsupported_path_encoding", "{error}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn strict_requests_reject_malformed_fields_before_path_or_state_access() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root], Vec::new()).unwrap(),
    );
    for (name, args, field) in [
        (
            "dm_memory_compare",
            json!({"baseline":["evidence.json",null,null,0],"current":{"evidence_path":"evidence.json"}}),
            "baseline",
        ),
        (
            "dm_native_evidence_summary",
            json!({"artifacts":[["performance_csv","fixture.csv"]]}),
            "artifacts[0]",
        ),
        (
            "dm_server_status",
            json!({"unexpected": true}),
            "unexpected",
        ),
        (
            "dm_parse_environment",
            json!({"dme_path":"missing.dme", "force":"false"}),
            "force",
        ),
        (
            "dm_get_proc",
            json!({"type_path":"/datum", "proc_name":"New", "include_source":null}),
            "include_source",
        ),
        (
            "dm_search_context",
            json!({"query":"door", "kind":"unknown"}),
            "kind",
        ),
        (
            "dm_compare_dmi_states",
            json!({"left_dmi_path":"missing.dmi", "left_state":"a", "right_dmi_path":"missing.dmi", "right_state":"b", "left_duplicate_index":4294967296_u64}),
            "left_duplicate_index",
        ),
    ] {
        let result = call_tool(&context, &ServerState::new(), name, args)
            .await
            .unwrap();
        let error = payload(&result);
        assert_eq!(error["code"], "invalid_input", "{name}: {error}");
        assert_eq!(error["details"]["field"], field, "{name}: {error}");
    }
}

#[tokio::test]
async fn canonical_evidence_aliases_are_rejected_before_artifact_reads() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_native_evidence_summary",
        json!({
            "artifacts":[
                {"kind":"performance_csv","path":root.join("Cargo.toml")},
                {"kind":"performance_csv","path":root.join("src/../Cargo.toml")}
            ]
        }),
    )
    .await
    .unwrap();
    let error = payload(&result);
    assert_eq!(error["code"], "invalid_input", "{error}");
    assert_eq!(error["details"]["field"], "artifacts[1].path", "{error}");
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
