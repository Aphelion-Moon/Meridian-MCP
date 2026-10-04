use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn initialize_owner() {
    #[cfg(windows)]
    meridian_mcp::process::initialize_runtime_owner().unwrap();
    #[cfg(unix)]
    meridian_mcp::process::initialize_runtime_owner_with_executable(std::path::Path::new(env!(
        "CARGO_BIN_EXE_meridian-mcp"
    )))
    .unwrap();
}

fn writer_context(root: &std::path::Path, policy: PathPolicy) -> ToolExecutionContext {
    initialize_owner();
    let private_path = root.with_extension("private");
    std::fs::create_dir_all(&private_path).unwrap();
    let store = std::sync::Arc::new(
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
    );
    ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy,
        meridian_mcp::RiftBuildAccess::Disabled,
        None,
        None,
        None,
        Some(store),
    )
}

fn remove_fixture(root: std::path::PathBuf) {
    let private = root.with_extension("private");
    std::fs::remove_dir_all(root).unwrap();
    if private.exists() {
        std::fs::remove_dir_all(private).unwrap();
    }
}

fn controlled_compiler() -> &'static std::path::PathBuf {
    initialize_owner();
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

fn output_compiler() -> &'static std::path::PathBuf {
    // Executable hashing is part of the compile deadline. Use a small fixture
    // instead of spending that budget on this integration-test harness.
    initialize_owner();
    static COMPILER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    COMPILER.get_or_init(|| {
        let path = std::env::temp_dir().join(format!(
            "meridian-output-compiler-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = std::process::Command::new("rustup")
            .args([
                "run",
                "1.95.0",
                "rustc",
                "--edition=2021",
                "tests/fixtures/compiler_output.rs",
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_final_publication_preserves_compiler_effects_without_verified_success() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (root, dme) = compiler_fixture("publication-expired");
    let compiler = controlled_compiler().clone();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();
    let context = writer_context(&root, policy.clone());
    let private_path = root.with_extension("private");
    let private =
        meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap();
    std::fs::write(root.join("source.dm"), "/world\n\tfps = 10\n").unwrap();
    std::fs::write(&dme, "#include \"source.dm\"\n").unwrap();
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        root.join("compiler.address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let request = json!({"dme_path":dme,"compiler_path":compiler,"timeout_ms":3000});
    let build =
        tokio::spawn(async move { call_tool(&context, &state, "dm_compile", request).await });
    let (mut stream, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
    stream.read_exact(&mut [0; 7]).await.unwrap();
    // begin_attempt has committed before the compiler barrier. Terminal
    // publication waits on the existing operation lock, which an independent
    // thread releases so synchronous state waits cannot starve its timer.
    let publication = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(private_path.join(".meridian-mcp.lock"))
        .unwrap();
    publication.lock().unwrap();
    let barrier = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(3500));
        drop(publication);
    });
    stream.write_all(b"x").await.unwrap();
    let mut remainder = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_end(&mut remainder),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(dme.with_extension("dmb").is_file());
    barrier.join().unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), build)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let value = payload(&result);
    assert_eq!(result.is_error, Some(true), "{value}");
    assert_eq!(value["success"], false, "{value}");
    assert_eq!(value["compiler_succeeded"], true, "{value}");
    assert_eq!(value["timed_out"], true, "{value}");
    assert_eq!(value["termination"], "wall_timeout", "{value}");
    assert_eq!(value["process_termination"], "exited", "{value}");
    assert_eq!(value["exit_code"], 0, "{value}");
    assert_eq!(value["dmb_updated"], true, "{value}");
    assert_eq!(
        value["finalization_interruption"], "request_timed_out",
        "{value}"
    );
    assert_ne!(value["provenance_status"], "verified", "{value}");
    let records = private.list_records("artifacts-v2", 16).unwrap();
    let state: Value = serde_json::from_slice(&std::fs::read(&records[0]).unwrap()).unwrap();
    assert_eq!(
        state["attempt"]["outcome"]["status"], "unverified",
        "{state}"
    );
    assert_eq!(
        state["attempt"]["outcome"]["code"], "request_timed_out",
        "{state}"
    );
    assert!(state["record"].is_null(), "{state}");
    assert!(
        state["attempt"]["retained_dmb_sha256"].is_string(),
        "{state}"
    );
    assert!(
        !state["attempt"]["observed_inputs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{state}"
    );
    let scopes = private.list_records("execution-v1", 16).unwrap();
    let scope: Value = serde_json::from_slice(&std::fs::read(&scopes[0]).unwrap()).unwrap();
    assert_eq!(scope["active"], false, "{scope}");
    remove_fixture(root);
}

#[tokio::test]
async fn shared_store_excludes_a_second_compiler_before_it_can_write() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (root, dme) = compiler_fixture("execution-exclusion");
    std::fs::write(root.join("source.dm"), "/world\n\tfps = 10\n").unwrap();
    let private_path = root.with_extension("private");
    std::fs::create_dir_all(&private_path).unwrap();
    let compiler = controlled_compiler().clone();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();
    let second = ToolExecutionContext::with_features_and_state(
        CapabilityMode::Development,
        policy.clone(),
        meridian_mcp::RiftBuildAccess::Disabled,
        None,
        None,
        None,
        Some(std::sync::Arc::new(
            meridian_mcp::PrivateStateStore::open(&private_path, policy.effective_roots()).unwrap(),
        )),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        root.join("compiler.address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let first = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "compiler_execution_host_fixture", "--nocapture"])
        .env("MERIDIAN_EXECUTION_FIXTURE_ROOT", &root)
        .env("MERIDIAN_EXECUTION_FIXTURE_COMPILER", &compiler)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (mut stream, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
    let mut started = [0; 7];
    stream.read_exact(&mut started).await.unwrap();
    let rejected = call_tool(
        &second,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path":dme,"compiler_path":compiler,"timeout_ms":75}),
    )
    .await;
    stream.write_all(b"x").await.unwrap();
    let first_result = first.wait_with_output().unwrap();
    let result = payload(&rejected.unwrap());
    assert_eq!(result["code"], "execution_busy", "{result}");
    assert!(
        first_result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first_result.stdout),
        String::from_utf8_lossy(&first_result.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(private_path).unwrap();
}

#[tokio::test]
async fn compiler_execution_host_fixture() {
    let Some(root) = std::env::var_os("MERIDIAN_EXECUTION_FIXTURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let compiler =
        std::path::PathBuf::from(std::env::var_os("MERIDIAN_EXECUTION_FIXTURE_COMPILER").unwrap());
    let context = writer_context(
        &root,
        PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap(),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_compile",
        json!({"dme_path":root.join("fixture.dme"),"compiler_path":compiler,"timeout_ms":10000}),
    )
    .await
    .unwrap();
    assert_eq!(payload(&result)["success"], true, "{result:?}");
}

#[tokio::test]
async fn dropping_a_compiler_request_completes_cleanup_before_scope_reuse() {
    use tokio::io::AsyncReadExt;
    let (root, dme) = compiler_fixture("dropped-execution");
    std::fs::write(root.join("source.dm"), "/world\n\tfps = 10\n").unwrap();
    let compiler = controlled_compiler().clone();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();
    let context = writer_context(&root, policy.clone());
    let private = meridian_mcp::PrivateStateStore::open(
        &root.with_extension("private"),
        policy.effective_roots(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        root.join("compiler.address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let first_dme = dme.clone();
    let task_context = context.clone();
    let first = tokio::spawn(async move {
        call_tool(
            &task_context,
            &ServerState::new(),
            "dm_compile",
            json!({"dme_path":first_dme,"timeout_ms":10000}),
        )
        .await
    });
    let (mut stream, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
    stream.read_exact(&mut [0; 7]).await.unwrap();
    let records = private.list_records("execution-v1", 20).unwrap();
    let record = records
        .iter()
        .find(|path| path.file_name().is_some_and(|name| name == "state.json"))
        .unwrap()
        .strip_prefix(private.root())
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(private.read_json::<Value>(record).unwrap()["active"], true);
    first.abort();
    let _ = first.await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while private.read_json::<Value>(record).unwrap()["active"] == true {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropped compiler did not finish owned cleanup");
    assert!(
        !dme.with_extension("dmb").exists(),
        "cancelled compiler wrote after cleanup"
    );
    let second = tokio::spawn(async move {
        call_tool(
            &context,
            &ServerState::new(),
            "dm_compile",
            json!({"dme_path":dme,"timeout_ms":10000}),
        )
        .await
    });
    let (mut stream, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
    stream.read_exact(&mut [0; 7]).await.unwrap();
    assert_eq!(private.read_json::<Value>(record).unwrap()["active"], true);
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while private.read_json::<Value>(record).unwrap()["active"] == true {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the reused scope did not finish owned cleanup");
    drop(private);
    remove_fixture(root);
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
    let compiler = output_compiler().clone();
    let canonical_compiler = compiler.canonicalize().unwrap();
    let context = writer_context(
        &root,
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
    remove_fixture(root);
}

#[tokio::test]
async fn omitted_compiler_does_not_probe_a_different_conventional_installation() {
    let (root, dme) = compiler_fixture("configured-over-conventional");
    let configured = root.join("configured-compiler.exe");
    std::fs::copy(output_compiler(), &configured).unwrap();
    let canonical_configured = configured.canonicalize().unwrap();
    let context = writer_context(
        &root,
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
    remove_fixture(root);
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
    let compiler = output_compiler().clone();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();
    let context = writer_context(&root, policy);

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
    remove_fixture(root);
}
