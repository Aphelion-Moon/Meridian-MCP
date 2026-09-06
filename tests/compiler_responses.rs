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
        "meridian-compiler-response-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    root
}
fn compiler() -> &'static PathBuf {
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
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], vec![compiler().clone()]).unwrap(),
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
        assert_eq!(data["provenance_status"], "unverified");
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
