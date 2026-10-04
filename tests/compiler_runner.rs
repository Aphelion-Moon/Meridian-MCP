use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn controlled_compiler() -> &'static std::path::PathBuf {
    static COMPILER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    COMPILER.get_or_init(|| {
        let path = std::env::temp_dir().join(format!(
            "meridian-provenance-compiler-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = std::process::Command::new("rustup")
            .args([
                "run",
                "1.95.0",
                "rustc",
                "--edition=2021",
                "tests/fixtures/provenance_compiler.rs",
                "-o",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        path
    })
}

async fn provenance_case(case: &str) -> (Value, std::path::PathBuf) {
    let (root, dme) = compiler_fixture(case);
    let private_path = root.with_extension("private");
    std::fs::create_dir_all(&private_path).unwrap();
    let compiler = controlled_compiler().clone();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();
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
        Some(private),
    );
    std::fs::write(root.join("source.dm"), "/world\n\tfps = 10\n").unwrap();
    std::fs::write(&dme, "#include \"source.dm\"\n").unwrap();
    if case == "scalar-define" {
        std::fs::write(root.join("source.dm"), "#define MERIDIAN_FIXTURE_PROTOCOL 4\n/** Fixture public proc.\n * Arguments:\n * * payload - fixture value\n */\n/proc/fixture(payload)\n\treturn length(payload)\n").unwrap();
    }
    if case == "define" {
        std::fs::write(
            &dme,
            "#ifdef ALTERNATE\n#include \"alternate.dm\"\n#else\n#include \"source.dm\"\n#endif\n",
        )
        .unwrap();
        std::fs::write(root.join("alternate.dm"), "/world\n\tfps = 20\n").unwrap();
    }
    let state = ServerState::new();
    let parsed = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path": dme}),
    )
    .await
    .unwrap();
    assert_ne!(parsed.is_error, Some(true));
    if case == "new-include" {
        std::fs::write(&dme, "#include \"source.dm\"\n#include \"added.dm\"\n").unwrap();
        std::fs::write(root.join("added.dm"), "/datum/added\n").unwrap();
    }
    if case == "resource" {
        std::fs::write(
            root.join("source.dm"),
            "/datum\n\tvar/asset = 'compiler-only.dmi'\n",
        )
        .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        root.join("compiler.address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let request = json!({"dme_path": dme, "compiler_path": compiler, "defines": if case == "define" {vec!["ALTERNATE"]} else {vec![]}, "timeout_ms": 30000});
    let build = call_tool(&context, &state, "dm_compile", request);
    let mutate = async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut started = [0; 7];
        stream.read_exact(&mut started).await.unwrap();
        let in_progress = store
            .evaluate_launch(&dme.with_extension("dmb"), false)
            .unwrap();
        assert!(
            !in_progress.allowed,
            "a writer must publish its managed attempt before starting"
        );
        assert!(in_progress
            .reasons
            .iter()
            .any(|reason| reason.code == "build_in_progress_or_interrupted"));
        if case == "during-compile" {
            std::fs::write(root.join("source.dm"), "/world\n\tfps = 99\n").unwrap();
        }
        if case == "configuration-during" {
            std::fs::write(root.join("SpacemanDMM.toml"), "[environment]\n").unwrap();
        }
        stream.write_all(b"x").await.unwrap();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(40), async {
        tokio::join!(build, mutate)
    })
    .await
    .unwrap();
    let result = payload(&result.unwrap());
    assert_eq!(result["success"], true, "{result}");
    if result["provenance_status"] == "verified" {
        assert_eq!(
            store
                .evaluate_launch(&dme.with_extension("dmb"), true)
                .unwrap()
                .status,
            meridian_mcp::ProvenanceStatus::Verified
        );
        std::fs::write(root.join("source.dm"), "// edit after verified compile\n").unwrap();
        assert_eq!(
            store
                .evaluate_launch(&dme.with_extension("dmb"), false)
                .unwrap()
                .status,
            meridian_mcp::ProvenanceStatus::Stale
        );
    }
    std::fs::remove_dir_all(private_path).unwrap();
    (result, root)
}

#[tokio::test]
async fn scalar_constant_defines_and_standalone_doc_comments_can_verify() {
    let (result, root) = provenance_case("scalar-define").await;
    assert_eq!(result["provenance_status"], "verified", "{result}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn compiler_resources_and_new_configuration_cannot_receive_verified_provenance() {
    for case in ["resource", "configuration-during"] {
        let (result, root) = provenance_case(case).await;
        assert_ne!(result["provenance_status"], "verified", "{case}: {result}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn effective_define_branch_is_not_verified_from_the_active_parser_closure() {
    let (result, root) = provenance_case("define").await;
    assert_ne!(result["provenance_status"], "verified", "{result}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn changing_source_after_compiler_start_cannot_promote_posthoc_bytes() {
    let (result, root) = provenance_case("during-compile").await;
    assert_ne!(result["provenance_status"], "verified", "{result}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn an_include_added_after_parse_must_be_in_the_build_identity() {
    let (result, root) = provenance_case("new-include").await;
    // Until the compiler closure is independently proved, a stale parse cannot establish it.
    assert_ne!(result["provenance_status"], "verified", "{result}");
    std::fs::remove_dir_all(root).unwrap();
}

fn payload(result: &ToolResult) -> Value {
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).expect("compiler result should be JSON")
}

fn compiler_fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-compiler-{name}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dme = root.join("fixture.dme");
    std::fs::write(&dme, "// fixture").unwrap();
    (root, dme)
}

#[tokio::test]
async fn omitted_compiler_rejects_an_empty_startup_allowlist_before_process_start() {
    let (root, dme) = compiler_fixture("empty-allowlist");
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme, "timeout_ms": 1}),
    )
    .await
    .unwrap();
    let payload = payload(&result);

    assert_eq!(result.is_error, Some(true));
    assert_eq!(payload["code"], "compiler_not_configured");
    assert!(!root.join("fixture.dmb").exists());
    assert!(payload.get("termination").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn omitted_compiler_uses_the_sole_startup_allowlisted_executable() {
    let (root, dme) = compiler_fixture("sole-allowlisted");
    let compiler = std::env::current_exe().unwrap();
    let canonical_compiler = compiler.canonicalize().unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], vec![compiler]).unwrap(),
    );

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme, "timeout_ms": 10_000}),
    )
    .await
    .unwrap();
    let payload = payload(&result);

    assert_eq!(
        payload["compiler"],
        canonical_compiler.display().to_string()
    );
    assert_eq!(payload["termination"], "exited");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn omitted_compiler_does_not_probe_a_different_conventional_installation() {
    let (root, dme) = compiler_fixture("configured-over-conventional");
    let configured = root.join("configured-compiler.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &configured).unwrap();
    let canonical_configured = configured.canonicalize().unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], vec![configured]).unwrap(),
    );

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme, "timeout_ms": 10_000}),
    )
    .await
    .unwrap();
    let payload = payload(&result);

    assert_eq!(
        payload["compiler"],
        canonical_configured.display().to_string()
    );
    assert_eq!(payload["termination"], "exited");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn omitted_compiler_rejects_an_ambiguous_startup_allowlist_before_process_start() {
    let (root, dme) = compiler_fixture("ambiguous-allowlist");
    let first = std::env::current_exe().unwrap();
    let second = root.join("second-compiler.exe");
    std::fs::copy(&first, &second).unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], vec![first, second]).unwrap(),
    );

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme, "timeout_ms": 1}),
    )
    .await
    .unwrap();
    let payload = payload(&result);

    assert_eq!(result.is_error, Some(true));
    assert_eq!(payload["code"], "compiler_ambiguous");
    assert!(!root.join("fixture.dmb").exists());
    assert!(payload.get("termination").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn explicit_and_implicit_denied_compilers_share_the_policy_outcome() {
    let (root, dme) = compiler_fixture("denied-selection");
    let denied = root.join("removed-compiler.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &denied).unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], vec![denied.clone()]).unwrap(),
    );
    std::fs::remove_file(&denied).unwrap();

    let explicit = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme, "compiler_path": denied}),
    )
    .await
    .unwrap();
    let implicit = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path": dme}),
    )
    .await
    .unwrap();

    assert_eq!(payload(&explicit)["code"], "executable_not_allowed");
    assert_eq!(payload(&implicit)["code"], "executable_not_allowed");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn direct_compile_reports_bounded_output_artifacts_and_optional_audit() {
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-compiler-runner-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dme = root.join("fixture.dme");
    let dmb = root.join("fixture.dmb");
    std::fs::write(&dme, "// fixture").unwrap();
    std::fs::write(&dmb, "pre-existing artifact").unwrap();
    let compiler = std::env::current_exe().unwrap();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();
    let context = ToolExecutionContext::new(CapabilityMode::Development, policy);

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({
            "dme_path": dme,
            "compiler_path": compiler,
            "working_directory": root,
            "capture_network": true,
            "timeout_ms": 10_000,
            "idle_timeout_ms": 5_000
        }),
    )
    .await
    .unwrap();
    let payload = payload(&result);

    assert_eq!(payload["termination"], "exited");
    assert_eq!(payload["network_audit"]["requested"], true);
    assert_eq!(payload["network_audit"]["capture_complete"], false);
    assert!(payload["stdout_truncated_bytes"].as_u64().is_some());
    assert!(payload["stderr_truncated_bytes"].as_u64().is_some());
    assert!(payload["artifact_before"]["sha256"].is_string());
    assert!(payload["artifact_after"]["sha256"].is_string());
    assert_eq!(payload["dmb_exists"], true);
    assert_eq!(payload["dme_argument"], "fixture.dme");
    std::fs::remove_dir_all(root).unwrap();
}
