#![cfg(windows)]

use meridian_mcp::result::ToolContent;
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy, RiftBuildAccess};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "meridian-rift-output-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
fn compiler() -> &'static PathBuf {
    meridian_mcp::process::initialize_runtime_owner().unwrap();
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let path = root().join("fixture.exe");
        let output = std::process::Command::new("rustc")
            .args([
                "+1.95.0",
                "--edition=2021",
                "tests/fixtures/rift_output.rs",
                "-o",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    })
}
async fn run(mode: &str, options: Value) -> (usize, Value, Option<Value>, bool) {
    let root = root();
    let workspace = root.join("workspace");
    let private_path = root.join("state");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&private_path).unwrap();
    let dme = workspace.join("tgstation.dme");
    std::fs::write(&dme, "/world\n    fps = 10\n").unwrap();
    std::fs::write(workspace.join("BUILD.cmd"), "@echo off\n").unwrap();
    std::fs::write(
        workspace.join("RIFT_BUILD.cmd"),
        "@echo off\r\n\"%DM_EXE%\"\r\nexit /b %ERRORLEVEL%\r\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("dependencies.sh"),
        "export BYOND_MAJOR=516\nexport BYOND_MINOR=1687\n",
    )
    .unwrap();
    std::fs::write(workspace.join("output-mode.txt"), mode).unwrap();
    if mode == "cache" {
        std::fs::write(workspace.join("tgstation.dmb"), b"rift fixture dmb").unwrap();
        std::fs::write(workspace.join("tgstation.rsc"), b"rift fixture rsc").unwrap();
    }
    let policy = PathPolicy::new(vec![workspace.clone()], vec![compiler().clone()]).unwrap();
    let private = Arc::new(
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
    );
    let store = meridian_mcp::BuildProvenanceStore::new(private.clone(), policy.clone());
    let context = ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy,
        RiftBuildAccess::Offline,
        None,
        None,
        None,
        Some(private.clone()),
    );
    let state = ServerState::new();
    let parsed = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":dme}),
    )
    .await
    .unwrap();
    assert_ne!(parsed.is_error, Some(true));
    let mut request = json!({"timeout_ms":10000});
    request
        .as_object_mut()
        .unwrap()
        .extend(options.as_object().unwrap().clone());
    let result = call_tool(&context, &state, "rift_compile", request)
        .await
        .unwrap();
    let ToolContent::Text { text } = &result.content[0];
    let length = text.len();
    let value = serde_json::from_str(text).unwrap();
    let key = store
        .artifact_key(&workspace.join("tgstation.dmb"))
        .unwrap();
    let attempt = private
        .read_json::<Value>(&format!("artifacts-v2/{key}/state.json"))
        .ok()
        .map(|state| state["attempt"].clone());
    let started = workspace.join("wrapper.marker").exists();
    drop(context);
    drop(store);
    drop(private);
    std::fs::remove_dir_all(root).unwrap();
    (length, value, attempt, started)
}

#[tokio::test]
async fn early_errors_and_wrapper_records_survive_tail_eviction() {
    for (mode, code) in [
        ("error", "build_failed"),
        ("malformed", "wrapper_result_invalid"),
        ("duplicate", "wrapper_result_invalid"),
        ("oversized", "output_analysis_incomplete"),
    ] {
        let (_, body, attempt, _) = run(mode, json!({})).await;
        assert_eq!(body["success"], false, "{mode}: {body}");
        assert_eq!(body["code"], code, "{mode}");
        assert_eq!(attempt.unwrap()["outcome"]["status"], "failed");
    }
}
#[tokio::test]
async fn early_cache_evidence_survives_tail_eviction() {
    let (_, body, _, _) = run("cache", json!({})).await;
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(body["evidence"], "valid_cache_hit");
    assert!(body["cache_evidence"]
        .as_str()
        .unwrap()
        .contains("Skipping 'dm'"));
}
#[tokio::test]
async fn response_controls_bound_logs_and_preserve_full_diagnostic_counts() {
    let (bytes, body, _, _) = run("flood", json!({})).await;
    assert!(bytes <= 512 * 1024, "reply bytes: {bytes}");
    assert_eq!(body["success"], true, "{body}");
    assert!(body["artifact_after"]["dmb"]["sha256"].is_string());
    for limit in [0, 2, 200] {
        let (bytes, body, _, _) = run(
            "many",
            json!({"include_output":false,"diagnostic_limit":limit}),
        )
        .await;
        assert!(bytes <= 512 * 1024);
        assert_eq!(body["success"], false);
        assert_eq!(body["diagnostic_summary"]["errors"], 30000);
        assert_eq!(body["diagnostics"].as_array().unwrap().len(), limit);
        assert!(body.get("stdout").is_none() && body.get("stderr").is_none());
    }
}
#[tokio::test]
async fn missing_artifact_failures_are_recorded() {
    let (_, body, attempt, _) = run("missing", json!({})).await;
    assert_eq!(body["success"], false, "{body}");
    assert_eq!(
        attempt.expect("missing failed attempt")["outcome"]["status"],
        "failed"
    );
}
#[tokio::test]
async fn invalid_output_options_never_start_the_wrapper() {
    for (key, value) in [
        ("include_output", json!("false")),
        ("include_output", Value::Null),
        ("output_max_bytes", json!(0)),
        ("output_max_bytes", json!(65537)),
        ("diagnostic_limit", json!(201)),
    ] {
        let mut args = json!({});
        args[key] = value;
        let (_, body, _, started) = run("quiet", args).await;
        assert_eq!(body["code"], "invalid_arguments");
        assert!(!started);
    }
}
