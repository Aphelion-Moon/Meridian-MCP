use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "meridian-compiler-input-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    root
}

fn compiler() -> &'static PathBuf {
    static COMPILER: OnceLock<PathBuf> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let root = root("binary");
        let source = root.join("compiler.rs");
        std::fs::write(
            &source,
            r#"
            fn main() {
                let args: Vec<_> = std::env::args().skip(1).collect();
                let path = std::path::Path::new(args.last().unwrap());
                assert!(path.is_file());
                std::fs::write(path.with_extension("started"), "started").unwrap();
                for arg in &args { println!("ARG:{arg}"); }
                if !args.iter().any(|arg| arg == "-DNO_ARTIFACT") {
                    std::fs::write(path.with_extension("dmb"), "owned compiler artifact").unwrap();
                }
            }
        "#,
        )
        .unwrap();
        let binary = root.join(format!("compiler{}", std::env::consts::EXE_SUFFIX));
        let output = std::process::Command::new("rustc")
            .args(["+1.95.0", "--edition=2021"])
            .arg(source)
            .arg("-o")
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

fn context(root: &Path) -> ToolExecutionContext {
    ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.to_owned()], vec![compiler().clone()]).unwrap(),
    )
}

fn payload(result: &ToolResult) -> Value {
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap_or_else(|_| json!({"text":text}))
}

#[tokio::test]
async fn compiler_relative_dme_is_resolved_at_dispatch_and_artifact_is_created() {
    let root = root("relative");
    let working = root.join("working directory");
    let project = working.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let dme = project.join("fixture.dme");
    std::fs::write(&dme, "// fixture").unwrap();
    let result = call_tool(&context(&root), &ServerState::new(), "dm_compile", json!({"dme_path":"project/fixture.dme", "working_directory":working, "defines":["VALUE=7","-DFLAG"],"timeout_ms":u64::MAX,"idle_timeout_ms":u64::MAX})).await.unwrap();
    let data = payload(&result);
    let artifact = dme.with_extension("dmb").exists();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(data["success"], true, "{data}");
    assert!(artifact);
    assert_eq!(data["dmb_updated"], true);
    assert_eq!(data["timeout_ms"], 1_800_000);
    assert_eq!(data["idle_timeout_ms"], 900_000);
    assert!(data["stdout"]
        .as_str()
        .unwrap()
        .contains("ARG:-DVALUE=7\nARG:-DFLAG\n"));
    assert_eq!(
        Path::new(data["dme_argument"].as_str().unwrap()),
        Path::new("project").join("fixture.dme")
    );
}

#[tokio::test]
async fn compiler_missing_artifact_is_retained_as_a_failed_attempt() {
    let root = root("attempt");
    let workspace = root.join("workspace");
    let private_path = root.join("state");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&private_path).unwrap();
    let dme = workspace.join("fixture.dme");
    std::fs::write(&dme, "// fixture").unwrap();
    let policy = PathPolicy::new(vec![workspace], vec![compiler().clone()]).unwrap();
    let private = std::sync::Arc::new(
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
    );
    let store = meridian_mcp::BuildProvenanceStore::new(private.clone(), policy.clone());
    let key = store.artifact_key(&dme.with_extension("dmb")).unwrap();
    let context = ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy,
        meridian_mcp::RiftBuildAccess::Disabled,
        None,
        None,
        None,
        Some(private.clone()),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path":dme,"defines":["NO_ARTIFACT"],"timeout_ms":10000}),
    )
    .await
    .unwrap();
    let attempt = private.read_json::<Value>(&format!("attempts/{key}.json"));
    drop(context);
    drop(store);
    drop(private);
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(payload(&result)["success"], false);
    let attempt = attempt.expect("missing DMB failure was not recorded");
    assert_eq!(
        attempt["outcome"],
        json!({"status":"failed","code":"artifact_missing"})
    );
}

#[tokio::test]
async fn compiler_invalid_inputs_do_not_start_a_process() {
    let root = root("invalid");
    let dme = root.join("fixture.dme");
    std::fs::write(&dme, "// fixture").unwrap();
    let context = context(&root);
    let state = ServerState::new();
    let mut failures = Vec::new();
    for (key, value) in [
        ("defines", json!(["FLAG", 42])),
        ("defines", json!("FLAG")),
        ("defines", Value::Null),
        ("defines", json!(["FLAG\u{0}MORE"])),
        ("timeout_ms", json!(0)),
        ("timeout_ms", json!("1000")),
        ("timeout_ms", Value::Null),
        ("idle_timeout_ms", json!(0)),
        ("idle_timeout_ms", json!(999)),
        ("idle_timeout_ms", json!("1000")),
        ("capture_network", json!("true")),
        ("capture_network", Value::Null),
        ("working_directory", json!(9)),
        ("working_directory", Value::Null),
        ("working_directory", json!(dme)),
        ("compiler_path", json!(false)),
        ("fixture_manifest_path", json!(false)),
        ("unknown_compile_option", json!(true)),
    ] {
        let mut args = json!({"dme_path":dme,"timeout_ms":10000});
        args[key] = value.clone();
        let outcome = call_tool(&context, &state, "dm_compile", args).await;
        let started = dme.with_extension("started").exists();
        if started {
            std::fs::remove_file(dme.with_extension("started")).unwrap();
        }
        match outcome {
            Ok(result) if !started && payload(&result)["code"] == "invalid_input" => {}
            outcome => failures.push(format!(
                "{key}={value}: started={started}, outcome={outcome:?}"
            )),
        }
    }
    std::fs::remove_dir_all(root).unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn compiler_zero_exit_without_a_dmb_is_not_build_success() {
    let root = root("missing-artifact");
    let dme = root.join("fixture.dme");
    std::fs::write(&dme, "// fixture").unwrap();
    let result = call_tool(
        &context(&root),
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path":dme,"defines":["NO_ARTIFACT"],"timeout_ms":10000}),
    )
    .await
    .unwrap();
    let data = payload(&result);
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(data["exit_code"], 0);
    assert_eq!(data["dmb_exists"], false);
    assert_eq!(data["success"], false, "{data}");
    assert_eq!(result.is_error, Some(true));
}

#[tokio::test]
async fn compiler_relative_dme_cannot_escape_the_workspace() {
    let root = root("escape");
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(root.join("outside.dme"), "// fixture").unwrap();
    let result = call_tool(
        &context(&workspace),
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path":"../outside.dme","working_directory":workspace}),
    )
    .await
    .unwrap();
    let data = payload(&result);
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(data["code"], "path_outside_workspace", "{data}");
}
