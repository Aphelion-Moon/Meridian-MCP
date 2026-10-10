use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "meridian-compiler-response-{}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    root
}
fn compiler() -> &'static PathBuf {
    #[cfg(windows)]
    meridian_mcp::process::initialize_runtime_owner().unwrap();
    #[cfg(unix)]
    meridian_mcp::process::initialize_runtime_owner_with_executable(std::path::Path::new(env!(
        "CARGO_BIN_EXE_meridian-mcp"
    )))
    .unwrap();
    static COMPILER: OnceLock<PathBuf> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let binary = root().join(format!("compiler{}", std::env::consts::EXE_SUFFIX));
        let output = std::process::Command::new("rustc")
            .args([
                "+1.95.0",
                "--edition=2021",
                "tests/fixtures/compiler_output.rs",
                "-o",
            ])
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        binary
    })
}
fn payload(result: &ToolResult) -> (&str, Value) {
    let ToolContent::Text { text } = &result.content[0];
    (text, serde_json::from_str(text).unwrap())
}
async fn compile(mode: &str, extra: Value) -> ToolResult {
    let root = root();
    let dme = root.join("fixture.dme");
    std::fs::write(&dme, "// output fixture").unwrap();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler().clone()]).unwrap();
    let private_path = root.with_extension("private");
    std::fs::create_dir_all(&private_path).unwrap();
    let private = std::sync::Arc::new(
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
    );
    let context = ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy,
        meridian_mcp::RiftBuildAccess::Disabled,
        None,
        None,
        None,
        Some(private),
    );
    let mut args =
        json!({"dme_path":dme,"defines":[format!("OUTPUT_MODE={mode}")],"timeout_ms":10000});
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let result = call_tool(&context, &ServerState::new(), "dm_compile", args)
        .await
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(private_path).unwrap();
    result
}

#[tokio::test]
async fn compiler_large_responses_keep_status_and_explicit_truncation() {
    for mode in ["dual", "unicode", "giant"] {
        let result = compile(mode, json!({})).await;
        let (text, data) = payload(&result);
        assert!(
            text.len() <= 512 * 1024,
            "{mode}: {} reply bytes",
            text.len()
        );
        assert_eq!(data["dmb_exists"], true, "{mode}");
        assert_eq!(data["success"], mode != "giant", "{mode}");
        assert!(data["artifact_after"]["sha256"].is_string());
        assert_eq!(
            data["provenance_status"],
            if mode == "giant" {
                "stale"
            } else {
                "unverified"
            }
        );
        assert!(
            data["output_summary"]["stdout"]["omitted_utf8_bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
        if mode == "dual" {
            assert_eq!(data["diagnostic_summary"]["capture_complete"], false);
        }
        if mode == "giant" {
            assert_eq!(data["diagnostic_summary"]["errors"], 1);
            assert_eq!(data["errors"][0]["line"], 7);
            assert_eq!(data["errors"][0]["message_truncated"], true);
        }
    }
}

#[tokio::test]
async fn compiler_diagnostic_limits_do_not_change_failure_or_counts() {
    for limit in [0, 2, 200] {
        let result = compile(
            "diagnostics",
            json!({"diagnostic_limit":limit,"include_output":false}),
        )
        .await;
        let (_, data) = payload(&result);
        assert_eq!(data["success"], false);
        assert_eq!(data["diagnostic_summary"]["errors"], 600);
        assert_eq!(data["diagnostic_summary"]["warnings"], 10);
        assert_eq!(data["errors"].as_array().unwrap().len(), limit);
        assert_eq!(data["warnings"].as_array().unwrap().len(), limit.min(10));
        assert!(data.get("stdout").is_none() && data.get("stderr").is_none());
        assert_eq!(data["diagnostic_summary"]["capture_complete"], true);
    }
}

#[tokio::test]
async fn compiler_output_controls_preserve_short_text_and_bound_unicode() {
    let result = compile("quiet", json!({})).await;
    let (_, data) = payload(&result);
    assert_eq!(data["stdout"], "compiler note\n");
    assert_eq!(data["output_summary"]["stdout"]["omitted_utf8_bytes"], 0);
    let result = compile("unicode", json!({"output_max_bytes":127})).await;
    let (_, data) = payload(&result);
    assert_eq!(data["success"], true);
    assert!(data["stdout"].as_str().unwrap().len() <= 127);
    assert!(
        data["output_summary"]["stdout"]["omitted_utf8_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[tokio::test]
async fn compiler_maximum_output_request_survives_json_escape_expansion() {
    let result = compile("controls", json!({"output_max_bytes":65536})).await;
    let (text, data) = payload(&result);
    assert!(text.len() <= 512 * 1024);
    assert_eq!(data["success"], true);
    for name in ["stdout", "stderr"] {
        let returned = data[name].as_str().unwrap();
        assert!(serde_json::to_vec(returned).unwrap().len() <= 65536);
        assert_eq!(data["output_summary"][name]["available_utf8_bytes"], 100000);
        assert_eq!(
            data["output_summary"][name]["returned_utf8_bytes"],
            returned.len()
        );
        assert_eq!(
            data["output_summary"][name]["omitted_utf8_bytes"],
            100000 - returned.len()
        );
    }
}

#[tokio::test]
async fn compiler_response_controls_reject_invalid_values_before_execution() {
    let root = root();
    let dme = root.join("fixture.dme");
    std::fs::write(&dme, "// fixture").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], vec![compiler().clone()]).unwrap(),
    );
    let mut failures = Vec::new();
    for (name, value) in [
        ("include_output", json!("false")),
        ("include_output", Value::Null),
        ("diagnostic_limit", json!(201)),
        ("diagnostic_limit", json!(-1)),
        ("output_max_bytes", json!(0)),
        ("output_max_bytes", json!(65537)),
        ("output_max_bytes", json!("8192")),
    ] {
        let mut args = json!({"dme_path":dme,"timeout_ms":10000});
        args[name] = value;
        let result = call_tool(&context, &ServerState::new(), "dm_compile", args)
            .await
            .unwrap();
        let (_, data) = payload(&result);
        if data["code"] != "invalid_input" || dme.with_extension("started").exists() {
            failures.push(name);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn early_diagnostics_survive_tail_eviction_and_still_fail_the_build() {
    let result = compile("early_error", json!({"include_output":false})).await;
    let (_, data) = payload(&result);
    assert_eq!(data["exit_code"], 0);
    assert_eq!(data["dmb_exists"], true);
    assert_eq!(data["success"], false);
    assert_eq!(data["compiler_succeeded"], false);
    assert_eq!(data["diagnostic_summary"]["errors"], 1);
    assert_eq!(data["diagnostic_summary"]["warnings"], 1);
    assert_eq!(data["errors"][0]["line"], 7);
    assert_eq!(data["warnings"][0]["line"], 8);
    assert_eq!(data["diagnostic_summary"]["scope"], "observed_output");
    assert_eq!(data["diagnostic_summary"]["analysis_complete"], true);
    assert_eq!(data["diagnostic_summary"]["capture_complete"], false);
}

#[tokio::test]
async fn diagnostic_totals_cover_all_observed_lines_with_bounded_returned_rows() {
    for limit in [0, 2, 200] {
        let result = compile(
            "many_diagnostics",
            json!({"diagnostic_limit":limit,"include_output":false}),
        )
        .await;
        let (text, data) = payload(&result);
        assert!(text.len() <= 512 * 1024);
        assert_eq!(data["success"], false);
        assert_eq!(data["diagnostic_summary"]["errors"], 30_000);
        assert_eq!(data["diagnostic_summary"]["warnings"], 20_000);
        assert_eq!(data["diagnostic_summary"]["analysis_complete"], true);
        for (name, total) in [("errors", 30_000), ("warnings", 20_000)] {
            assert_eq!(data[name].as_array().unwrap().len(), limit);
            assert_eq!(
                data["diagnostic_summary"][format!("{name}_detail")]["omitted"],
                total - limit
            );
            if limit > 0 {
                assert_eq!(data[name][0]["line"], 1);
            }
        }
    }
}

#[tokio::test]
async fn incomplete_diagnostic_analysis_cannot_report_a_successful_build() {
    let result = compile("overlong_line", json!({"include_output":false})).await;
    let (_, data) = payload(&result);
    assert_eq!(data["exit_code"], 0);
    assert_eq!(data["success"], false);
    assert_eq!(data["diagnostic_summary"]["analysis_complete"], false);
    assert_eq!(data["diagnostic_summary"]["oversized_lines"], 1);
    assert!(data["diagnostic_analysis_error"].is_string());
}

#[tokio::test]
async fn an_evicted_error_cannot_create_verified_provenance() {
    let root = root();
    let workspace = root.join("workspace");
    let private_path = root.join("private");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&private_path).unwrap();
    let dme = workspace.join("failure_after_success.dme");
    std::fs::write(&dme, "/world\n    fps = 10\n").unwrap();
    let policy = PathPolicy::new(vec![workspace], vec![compiler().clone()]).unwrap();
    let private = std::sync::Arc::new(
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
    );
    let store = meridian_mcp::BuildProvenanceStore::new(private.clone(), policy.clone());
    let context = ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy,
        meridian_mcp::RiftBuildAccess::Disabled,
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
    let previous = call_tool(
        &context,
        &state,
        "dm_compile",
        json!({"dme_path":dme,"timeout_ms":10000,"include_output":false}),
    )
    .await
    .unwrap();
    let (_, previous) = payload(&previous);
    assert_eq!(previous["success"], true);
    assert_eq!(previous["provenance_status"], "verified");
    let result = call_tool(
        &context,
        &state,
        "dm_compile",
        json!({"dme_path":dme,"timeout_ms":10000,"include_output":false}),
    )
    .await
    .unwrap();
    let (_, data) = payload(&result);
    assert_eq!(data["success"], false);
    assert_eq!(data["provenance_status"], "stale");
    assert_eq!(data["build_record_id"], previous["build_record_id"]);
    let key = store.artifact_key(&dme.with_extension("dmb")).unwrap();
    let attempt = private
        .read_json::<Value>(&format!("artifacts-v2/{key}/state.json"))
        .unwrap();
    assert_eq!(
        attempt["attempt"]["outcome"],
        json!({"status":"failed","code":"compiler_failed"})
    );
    drop(context);
    drop(store);
    drop(private);
    std::fs::remove_dir_all(root).unwrap();
}
