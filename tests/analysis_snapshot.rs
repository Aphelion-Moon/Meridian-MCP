use meridian_mcp::result::ToolContent;
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn cyclic_parent_types_do_not_wedge_parse_admission() {
    const WORKER_ROOT: &str = "MERIDIAN_CYCLE_TEST_ROOT";
    let root = match std::env::var_os(WORKER_ROOT) {
        Some(root) => std::path::PathBuf::from(root),
        None => {
            let (root, _, _) = fixture();
            let mut worker = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "cyclic_parent_types_do_not_wedge_parse_admission",
                    "--nocapture",
                ])
                .env(WORKER_ROOT, &root)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let status = loop {
                if let Some(status) = worker.try_wait().unwrap() {
                    break Some(status);
                }
                if std::time::Instant::now() >= deadline {
                    worker.kill().unwrap();
                    worker.wait().unwrap();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            std::fs::remove_dir_all(root).unwrap();
            assert!(
                status.is_some_and(|status| status.success()),
                "cyclic parent test failed or left a parser worker running"
            );
            return;
        }
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        );
        let state = ServerState::new();
        let good = root.join("one.dme");
        let bad = root.join("two.dme");
        let parsed = call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":good}),
        )
        .await
        .unwrap();
        assert_ne!(parsed.is_error, Some(true));
        for source in [
            "/datum/a\n\tparent_type = /datum/a\n",
            "/datum/a\n\tparent_type = /datum/b\n/datum/b\n\tparent_type = /datum/a\n",
        ] {
            let before = state.snapshot().await.unwrap();
            std::fs::write(&bad, source).unwrap();
            let rejected = call_tool(
                &context,
                &state,
                "dm_parse_environment",
                json!({"dme_path":bad,"timeout_ms":2000}),
            )
            .await
            .unwrap();
            assert_eq!(payload(rejected)["code"], "invalid_input");
            assert!(Arc::ptr_eq(&before, &state.snapshot().await.unwrap()));
            let recovered = call_tool(
                &context,
                &state,
                "dm_parse_environment",
                json!({"dme_path":good,"force":true,"timeout_ms":2000}),
            )
            .await
            .unwrap();
            assert_ne!(recovered.is_error, Some(true));
            // The next cycle must remain a hard failure with diagnostics hidden.
            std::fs::write(
                root.join("SpacemanDMM.toml"),
                "[display]\nerror_level = \"disabled\"\n",
            )
            .unwrap();
        }
    });
}

fn settle_file(path: &std::path::Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
        .unwrap();
}

#[tokio::test]
async fn external_source_edit_invalidates_snapshot() {
    let (root, _, _) = fixture();
    let allowed = root.join("allowed");
    let sibling = root.join("sibling");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    let external = sibling.join("external.dm");
    let dme = allowed.join("fixture.dme");
    std::fs::write(&dme, "#include \"../sibling/external.dm\"\n").unwrap();
    std::fs::write(&external, "/datum/external\n\tvar/value = 1\n").unwrap();
    settle_file(&dme);
    settle_file(&external);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![allowed, sibling], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let args = json!({"dme_path":dme});
    let first = payload(
        call_tool(&context, &state, "dm_parse_environment", args.clone())
            .await
            .unwrap(),
    );
    let unchanged = payload(
        call_tool(&context, &state, "dm_parse_environment", args.clone())
            .await
            .unwrap(),
    );
    assert_eq!(unchanged["reused"], true);
    assert_eq!(unchanged["state_generation"], first["state_generation"]);
    std::fs::write(&external, "/datum/external\n\tvar/value = 2\n").unwrap();
    let changed = payload(
        call_tool(&context, &state, "dm_parse_environment", args)
            .await
            .unwrap(),
    );
    assert_eq!(changed["reused"], false);
    assert_eq!(changed["state_generation"], 2);
    let value = payload(
        call_tool(
            &context,
            &state,
            "dm_get_var",
            json!({"type_path":"/datum/external", "var_name":"value"}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(value["constant"], "Float(2.0)");
    std::fs::remove_file(&external).unwrap();
    let failed = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":dme}),
    )
    .await
    .unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert_eq!(state.snapshot().await.unwrap().generation, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn configuration_discovery_transitions_refresh_diagnostics() {
    let (root, dme, _) = fixture();
    std::fs::write(&dme, "/proc/check()\n\tvar/tmp/value\n\treturn value\n").unwrap();
    settle_file(&dme);
    let config = root.join("SpacemanDMM.toml");
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let args = json!({"dme_path":dme});
    call_tool(&context, &state, "dm_parse_environment", args.clone())
        .await
        .unwrap();
    let baseline = state.snapshot().await.unwrap().diagnostics.to_vec();
    for (generation, severity) in [(2, Some("warning")), (3, Some("error")), (4, None)] {
        if let Some(severity) = severity {
            std::fs::write(
                &config,
                format!("[diagnostics]\ntmp_no_effect = \"{severity}\"\n"),
            )
            .unwrap();
            settle_file(&config);
        } else {
            std::fs::remove_file(&config).unwrap();
        }
        let result = payload(
            call_tool(&context, &state, "dm_parse_environment", args.clone())
                .await
                .unwrap(),
        );
        assert_eq!(result["reused"], false, "transition {generation}: {result}");
        assert_eq!(result["state_generation"], generation);
        let snapshot = state.snapshot().await.unwrap();
        if let Some(severity) = severity {
            let diagnostic = snapshot
                .diagnostics
                .iter()
                .find(|d| d.rule.as_deref() == Some("tmp_no_effect"))
                .unwrap();
            assert_eq!(diagnostic.severity, severity);
            assert!(diagnostic.configured);
        } else {
            assert_eq!(&*snapshot.diagnostics, baseline);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn proc_excerpt_remains_in_its_snapshot_after_disk_edit() {
    let (root, dme, _) = fixture();
    std::fs::write(&dme, "/proc/check()\n\treturn \"snapshot text\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":dme}),
    )
    .await
    .unwrap();
    std::fs::write(
        &dme,
        "// moved source\n/proc/check()\n\treturn \"live text\"\n",
    )
    .unwrap();
    let result = payload(
        call_tool(
            &context,
            &state,
            "dm_get_proc",
            json!({"type_path":"", "proc_name":"check"}),
        )
        .await
        .unwrap(),
    );
    assert!(result["overrides"][0]["source"]
        .as_str()
        .unwrap()
        .contains("snapshot text"));
    assert!(!result.to_string().contains("live text"));
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = loop {
        let candidate = std::env::temp_dir().join(format!(
            "meridian-mcp-analysis-snapshot-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        // Process IDs repeat. Preserve leftover fixtures without inheriting
        // their configuration or treating existing paths as ours to remove.
        match std::fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("could not create analysis fixture: {error}"),
        }
    };
    let one = root.join("one.dme");
    let two = root.join("two.dme");
    std::fs::write(&one, "/datum/snapshot_one\n").unwrap();
    std::fs::write(&two, "/datum/snapshot_two\n").unwrap();
    (root, one, two)
}

fn payload(result: meridian_mcp::result::ToolResult) -> Value {
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn supervised_reuse_preserves_generation_and_timing_contract() {
    let (root, one, _) = fixture();
    std::fs::write(
        &one,
        "/datum/snapshot_one\n\tvar/value = 1\n\tproc/check()\n\t\treturn value\n",
    )
    .unwrap();
    settle_file(&one);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let first = payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":one}),
        )
        .await
        .unwrap(),
    );
    let reused = payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":one}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(reused["reused"], true);
    let snapshot_id = first["analysis"]["snapshot_id"]
        .as_str()
        .expect("a published parse must identify its snapshot");
    assert_eq!(reused["analysis"]["snapshot_id"], snapshot_id);
    assert_eq!(reused["state_generation"], first["state_generation"]);
    assert_eq!(reused["timings_ms"].as_object().unwrap().len(), 3);
    for stage in ["queue_wait", "reuse_validation", "total"] {
        assert!(reused["timings_ms"][stage].is_u64());
    }
    let forced = payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":one, "force":true}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(forced["reused"], false);
    assert_eq!(forced["state_generation"], 2);
    assert_ne!(forced["analysis"]["snapshot_id"], snapshot_id);
    let known = call_tool(&context, &state, "dm_get_type", json!({"type_path":"/datum/snapshot_one", "expected_snapshot":forced["analysis"]["snapshot_id"]})).await.unwrap();
    assert_eq!(
        known.meta.as_ref().unwrap().analysis.snapshot_id,
        forced["analysis"]["snapshot_id"].as_str().unwrap()
    );
    assert_eq!(payload(known)["analysis"], forced["analysis"]);
    for (tool, args) in [
        (
            "dm_get_proc",
            json!({"type_path":"/datum/snapshot_one","proc_name":"check"}),
        ),
        (
            "dm_get_var",
            json!({"type_path":"/datum/snapshot_one","var_name":"value"}),
        ),
        ("dm_list_types", json!({"prefix":"/datum/snapshot_one"})),
        ("dm_search_symbols", json!({"query":"snapshot_one"})),
        ("dm_search_context", json!({"query":"snapshot_one"})),
        ("dm_check_errors", json!({})),
        (
            "dm_get_definition",
            json!({"type_path":"/datum/snapshot_one"}),
        ),
        ("dm_document_symbols", json!({"file_path":one})),
        (
            "dm_find_references",
            json!({"type_path":"/datum/snapshot_one","member_name":"value"}),
        ),
        (
            "dm_find_implementations",
            json!({"type_path":"/datum/snapshot_one"}),
        ),
    ] {
        let result = call_tool(&context, &state, tool, args).await.unwrap();
        assert_eq!(
            result.meta.as_ref().unwrap().analysis.snapshot_id,
            forced["analysis"]["snapshot_id"].as_str().unwrap(),
            "{tool}"
        );
        assert_eq!(payload(result)["analysis"], forced["analysis"], "{tool}");
    }
    let missing = call_tool(
        &context,
        &state,
        "dm_get_type",
        json!({"type_path":"/datum/absent"}),
    )
    .await
    .unwrap();
    assert_eq!(missing.is_error, Some(true));
    let ToolContent::Text { text } = &missing.content[0];
    assert_eq!(text, "Type not found: /datum/absent");
    assert_eq!(
        missing.meta.unwrap().analysis.snapshot_id,
        forced["analysis"]["snapshot_id"].as_str().unwrap()
    );
    let stale = payload(
        call_tool(
            &context,
            &state,
            "dm_get_type",
            json!({
                "type_path":"/datum/snapshot_one", "expected_snapshot":snapshot_id
            }),
        )
        .await
        .unwrap(),
    );
    assert_eq!(stale["code"], "stale_generation");
    let restarted = ServerState::new();
    let restart = payload(
        call_tool(
            &context,
            &restarted,
            "dm_parse_environment",
            json!({"dme_path":one}),
        )
        .await
        .unwrap(),
    );
    assert_ne!(restart["analysis"]["snapshot_id"], snapshot_id);
    for stage in [
        "queue_wait",
        "preprocess_parse",
        "dreamchecker",
        "search_documents",
        "analysis_indexes",
        "fingerprint",
        "total",
    ] {
        assert!(forced["timings_ms"][stage].is_u64());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn held_snapshot_survives_a_new_parse_generation() {
    let (root, one, two) = fixture();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::new();

    let first = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({ "dme_path": one.clone() }),
    )
    .await
    .unwrap();
    assert_eq!(payload(first)["state_generation"], 1);
    let held = state.snapshot().await.unwrap();

    let second = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({ "dme_path": two }),
    )
    .await
    .unwrap();
    assert_eq!(payload(second)["state_generation"], 2);
    let active = state.snapshot().await.unwrap();

    assert_eq!(held.generation, 1);
    assert_eq!(active.generation, 2);
    assert!(held.environment_path.ends_with("one.dme"));
    assert!(active.environment_path.ends_with("two.dme"));
    assert!(held.objtree.find("/datum/snapshot_one").is_some());
    assert_eq!(held.source_inputs(), &[one.canonicalize().unwrap()]);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn failed_parse_preserves_the_complete_active_generation() {
    let (root, one, _) = fixture();
    let broken = root.join("broken.dme");
    std::fs::create_dir(&broken).unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::new();
    call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({ "dme_path": one }),
    )
    .await
    .unwrap();
    let before = state.snapshot().await.unwrap();

    let failed = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({ "dme_path": broken }),
    )
    .await
    .unwrap();
    let failed = payload(failed);
    let after = state.snapshot().await.unwrap();

    assert_eq!(failed["details"]["state_preserved"], true);
    assert_eq!(failed["details"]["state_generation"], 1);
    assert_eq!(
        failed["details"]["analysis"]["snapshot_id"],
        before.snapshot_id
    );
    assert!(failed["details"]["requested_environment"]
        .as_str()
        .unwrap()
        .ends_with("broken.dme"));
    assert_eq!(before.generation, after.generation);
    assert_eq!(before.environment_path, after.environment_path);
    let missing = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":root.join("absent.dme")}),
    )
    .await
    .unwrap();
    assert_eq!(
        missing.meta.as_ref().unwrap().analysis.snapshot_id,
        before.snapshot_id
    );
    let missing = payload(missing);
    assert_eq!(missing["details"]["state_preserved"], true);
    assert!(missing["details"]["requested_environment"]
        .as_str()
        .unwrap()
        .ends_with("absent.dme"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn type_and_diagnostic_cursors_reject_foreign_queries_and_restarts() {
    let (root, one, _) = fixture();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":one}),
    )
    .await
    .unwrap();
    let first = payload(
        call_tool(&context, &state, "dm_list_types", json!({"limit":1}))
            .await
            .unwrap(),
    );
    let cursor = first["pagination"]["next_cursor"].as_str().unwrap();
    assert!(cursor.starts_with("v2:"));
    let continued = payload(
        call_tool(
            &context,
            &state,
            "dm_list_types",
            json!({"limit":2,"cursor":cursor}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(continued["pagination"]["cursor"], "1");
    assert!(call_tool(
        &context,
        &state,
        "dm_list_types",
        json!({"prefix":"/datum","cursor":cursor})
    )
    .await
    .is_err());
    assert!(call_tool(
        &context,
        &state,
        "dm_check_errors",
        json!({"cursor":cursor})
    )
    .await
    .is_err());
    assert_eq!(
        payload(
            call_tool(&context, &state, "dm_list_types", json!({"cursor":"1"}))
                .await
                .unwrap()
        )["code"],
        "invalid_input"
    );
    let restarted = ServerState::new();
    call_tool(
        &context,
        &restarted,
        "dm_parse_environment",
        json!({"dme_path":one}),
    )
    .await
    .unwrap();
    assert!(call_tool(
        &context,
        &restarted,
        "dm_list_types",
        json!({"cursor":cursor})
    )
    .await
    .is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloned_snapshot_read_does_not_block_installing_the_next_generation() {
    let (root, one, two) = fixture();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let state = Arc::new(ServerState::new());
    call_tool(
        &context,
        state.as_ref(),
        "dm_parse_environment",
        json!({ "dme_path": one }),
    )
    .await
    .unwrap();
    let held = state.snapshot().await.unwrap();
    let reader = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        held.objtree.iter_types().count()
    });

    call_tool(
        &context,
        state.as_ref(),
        "dm_parse_environment",
        json!({ "dme_path": two }),
    )
    .await
    .unwrap();

    assert!(reader.await.unwrap() > 0);
    assert_eq!(state.snapshot().await.unwrap().generation, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn transitive_include_escape_preserves_generation() {
    let (root, _, _) = fixture();
    let allowed = root.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let good = allowed.join("good.dme");
    let escape = allowed.join("fixture.dme");
    std::fs::write(&good, "/datum/previous\n").unwrap();
    std::fs::write(&escape, "#include \"../external.dm\"\n").unwrap();
    std::fs::write(
        root.join("external.dm"),
        "/datum/audit_external\n\tproc/read_marker()\n\t\treturn \"AUDIT_OUTSIDE_ROOT\"\n",
    )
    .unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![allowed], vec![]).unwrap(),
    );
    let state = ServerState::new();
    call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":good}),
    )
    .await
    .unwrap();
    let result = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":escape}),
    )
    .await
    .unwrap();
    assert!(
        result.is_error == Some(true),
        "outside include was accepted: {:?}",
        payload(result)
    );
    let snapshot = state.snapshot().await.unwrap();
    assert_eq!(snapshot.generation, 1);
    assert!(snapshot.objtree.find("/datum/audit_external").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn nested_conditional_escape_is_denied_with_suppressed_diagnostics() {
    let (root, _, _) = fixture();
    let allowed = root.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::write(root.join("external.dm"), "/datum/audit_external\n").unwrap();
    std::fs::write(
        allowed.join("nested.dm"),
        "#if 1\n#include \"../external.dm\"\n#endif\n",
    )
    .unwrap();
    std::fs::write(
        allowed.join("SpacemanDMM.toml"),
        "[display]\nerror_level = \"off\"\n",
    )
    .unwrap();
    let dme = allowed.join("fixture.dme");
    std::fs::write(&dme, "#include \"nested.dm\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![allowed], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let result = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":dme}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(state.active_snapshot().await.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn explicitly_authorized_sibling_supports_parse_inspection_and_search() {
    let (root, _, _) = fixture();
    let allowed = root.join("allowed");
    let sibling = root.join("sibling");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(
        sibling.join("external.dm"),
        "/datum/audit_external\n\tproc/read_marker()\n\t\treturn \"AUDIT_OUTSIDE_ROOT\"\n",
    )
    .unwrap();
    let dme = allowed.join("fixture.dme");
    std::fs::write(&dme, "#include \"../sibling/external.dm\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![allowed, sibling], vec![]).unwrap(),
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
    let inspected = call_tool(
        &context,
        &state,
        "dm_get_proc",
        json!({"type_path":"/datum/audit_external","proc_name":"read_marker"}),
    )
    .await
    .unwrap();
    assert!(payload(inspected)
        .to_string()
        .contains("AUDIT_OUTSIDE_ROOT"));
    let searched = call_tool(
        &context,
        &state,
        "dm_search_context",
        json!({"query":"AUDIT_OUTSIDE_ROOT"}),
    )
    .await
    .unwrap();
    assert!(payload(searched).to_string().contains("AUDIT_OUTSIDE_ROOT"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn junction_or_symlink_include_cannot_escape_startup_roots() {
    let (root, _, _) = fixture();
    let allowed = root.join("allowed");
    let external = root.join("external");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("external.dm"), "/datum/audit_external\n").unwrap();
    let link = allowed.join("linked");
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&external)
        .output()
        .unwrap()
        .status
        .success());
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &link).unwrap();
    let dme = allowed.join("fixture.dme");
    std::fs::write(&dme, "#include \"linked/external.dm\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![allowed], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let result = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":dme}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(state.active_snapshot().await.is_none());
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn linked_configuration_is_rejected_when_symlinks_are_available() {
    let (root, _, _) = fixture();
    let allowed = root.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let external = root.join("external.toml");
    std::fs::write(&external, "[display]\nerror_level = \"off\"\n").unwrap();
    let link = allowed.join("SpacemanDMM.toml");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&external, &link);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&external, &link);
    if let Err(error) = linked {
        assert!(
            error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314),
            "unexpected symlink failure: {error}"
        );
        eprintln!("SKIP: host cannot create file symlinks: {error}");
        std::fs::remove_dir_all(root).unwrap();
        return;
    }
    let dme = allowed.join("fixture.dme");
    std::fs::write(&dme, "/datum/audit\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![allowed], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let result = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":dme}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(state.active_snapshot().await.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn builtin_proc_inspection_has_no_filesystem_source() {
    let (root, one, _) = fixture();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path":one}),
    )
    .await
    .unwrap();
    let inspected = call_tool(
        &context,
        &state,
        "dm_get_proc",
        json!({"type_path":"/list", "proc_name":"Add"}),
    )
    .await
    .unwrap();
    assert_ne!(inspected.is_error, Some(true));
    let result = payload(inspected);
    assert!(result["overrides"][0]["source"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}
