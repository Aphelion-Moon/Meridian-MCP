use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, FixtureInputRole, FixtureManifest, PathPolicy};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn checked_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provenance")
}

fn temporary_fixture(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "meridian-mcp-manifest-{name}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    for file in [
        "fixture.dme",
        "fixture.dm",
        "generated_bindings.dm",
        "native_module.bin",
        "service.bin",
    ] {
        std::fs::copy(checked_fixture().join(file), directory.join(file)).unwrap();
    }
    directory
}

fn document() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(checked_fixture().join("fixture-manifest.json")).unwrap(),
    )
    .unwrap()
}

fn write_document(directory: &Path, document: &Value) -> PathBuf {
    let path = directory.join("fixture-manifest.json");
    std::fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    path
}

fn payload(result: &ToolResult) -> Value {
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).expect("fixture result should be JSON")
}

#[test]
fn valid_manifest_is_contained_hashed_and_deterministic() {
    let root = checked_fixture();
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    let first = FixtureManifest::load(&policy, &root.join("fixture-manifest.json")).unwrap();
    let second = FixtureManifest::load(&policy, &root.join("fixture-manifest.json")).unwrap();

    assert_eq!(first.identity_sha256, second.identity_sha256);
    assert_eq!(first.identity_sha256.len(), 64);
    assert_eq!(first.inputs.len(), 4);
    assert_eq!(first.inputs[0].role, FixtureInputRole::GeneratedBinding);
    assert!(first.inputs.iter().all(|input| input
        .canonical_path
        .starts_with(root.canonicalize().unwrap())));
}

#[test]
fn fixture_input_limits_accept_exact_boundaries_and_reject_excess_bytes() {
    use meridian_mcp::limits::ServerLimits;

    let root = temporary_fixture("input-byte-limits");
    std::fs::write(root.join("fixture.dm"), b"text").unwrap();
    std::fs::write(root.join("native_module.bin"), b"binary!!").unwrap();
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    let mut document = document();
    document["inputs"] = json!([
        {"path": "fixture.dm", "role": "source"},
        {"path": "native_module.bin", "role": "native_module"},
    ]);
    let manifest = write_document(&root, &document);
    let exact = ServerLimits {
        max_fixture_text_file_bytes: 4,
        max_fixture_binary_file_bytes: 8,
        max_fixture_input_bytes: 12,
        ..Default::default()
    };
    let bounded = FixtureManifest::load_with_limits(&policy, &manifest, &exact).unwrap();
    let default = FixtureManifest::load(&policy, &manifest).unwrap();
    assert_eq!(bounded.identity_sha256, default.identity_sha256);
    assert_eq!(
        bounded.inputs.iter().map(|input| input.size).sum::<u64>(),
        12
    );
    for (limits, message) in [
        (
            ServerLimits {
                max_fixture_text_file_bytes: 3,
                ..exact.clone()
            },
            "source input exceeds",
        ),
        (
            ServerLimits {
                max_fixture_binary_file_bytes: 7,
                ..exact.clone()
            },
            "native_module input exceeds",
        ),
        (
            ServerLimits {
                max_fixture_input_bytes: 11,
                ..exact.clone()
            },
            "total byte limit",
        ),
    ] {
        let error = FixtureManifest::load_with_limits(&policy, &manifest, &limits).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn fixture_sync_and_compile_enforce_server_input_limits() {
    let root = checked_fixture();
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::with_limits(meridian_mcp::limits::ServerLimits {
        max_fixture_text_file_bytes: 1,
        ..Default::default()
    });
    let checked = call_tool(
        &context,
        &state,
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": root.join("fixture-manifest.json")}),
    )
    .await
    .unwrap();
    let checked = payload(&checked);
    assert_eq!(checked["classification"], "invalid");
    assert_eq!(checked["validation_complete"], false);
    assert_eq!(checked["issues"][0]["code"], "fixture_manifest_invalid");
    assert!(checked["issues"][0]["message"]
        .as_str()
        .unwrap()
        .contains("1-byte limit"));
    assert!(state.active_snapshot().await.is_none());

    let error = call_tool(
        &context,
        &state,
        "dm_compile",
        json!({
            "dme_path": root.join("fixture.dme"),
            "fixture_manifest_path": root.join("fixture-manifest.json"),
        }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("1-byte limit"), "{error}");
}

#[test]
fn invalid_paths_roles_fields_and_missing_files_fail_closed() {
    let cases = [
        ("traversal", json!("../escape"), None),
        ("absolute", json!("C:/escape"), None),
        ("glob", json!("*.dm"), None),
        ("url", json!("https://example.invalid/file"), None),
        ("missing", json!("missing.bin"), None),
        (
            "role",
            json!("fixture.dm"),
            Some(json!("executable_command")),
        ),
    ];
    for (name, path, role) in cases {
        let directory = temporary_fixture(name);
        let mut document = document();
        document["inputs"][0]["path"] = path;
        if let Some(role) = role {
            document["inputs"][0]["role"] = role;
        }
        let manifest = write_document(&directory, &document);
        let policy = PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap();
        assert!(
            FixtureManifest::load(&policy, &manifest).is_err(),
            "case {name}"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    let directory = temporary_fixture("duplicate");
    let mut duplicate_document = document();
    duplicate_document["inputs"][1]["path"] = duplicate_document["inputs"][0]["path"].clone();
    let manifest = write_document(&directory, &duplicate_document);
    let policy = PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap();
    assert!(FixtureManifest::load(&policy, &manifest).is_err());
    std::fs::remove_dir_all(directory).unwrap();

    let directory = temporary_fixture("unknown-field");
    let mut unknown_field_document = document();
    unknown_field_document["command"] = json!("compile");
    let manifest = write_document(&directory, &unknown_field_document);
    let policy = PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap();
    assert!(FixtureManifest::load(&policy, &manifest).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fixture_sync_reports_a_missing_generated_proc() {
    let directory = temporary_fixture("missing-generated-proc");
    let binding = directory.join("generated_bindings.dm");
    let contents = std::fs::read_to_string(&binding).unwrap();
    let proc_start = contents.find("/** Accept one technical").unwrap();
    std::fs::write(&binding, &contents[..proc_start]).unwrap();
    let manifest = write_document(&directory, &document());
    let policy = PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap();
    let context = ToolExecutionContext::new(CapabilityMode::Analysis, policy);

    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest}),
    )
    .await
    .unwrap();
    let payload = payload(&result);

    assert_eq!(payload["classification"], "invalid");
    assert_eq!(payload["issues"][0]["code"], "required_proc_missing");
    assert_eq!(
        payload["issues"][0]["path"],
        "/proc/meridian_fixture_state_batch"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fixture_tokens_accept_utf8_and_latin1_source() {
    for (name, comment, tokens) in [
        (
            "utf8-token",
            &b"\n// caf\xc3\xa9\n"[..],
            vec!["caf\u{e9}", "caf\u{e9}"],
        ),
        (
            "latin1-token",
            &b"\n// caf\xe9\n"[..],
            vec!["caf\u{e9}", "caf\u{e9}"],
        ),
        (
            "mixed-encoding-lines",
            &b"\n// caf\xe9\r\n\n// snowman: \xe2\x98\x83\r\n"[..],
            vec!["caf\u{e9}\n\n// snowman: \u{2603}", "caf\u{e9}", "\u{2603}"],
        ),
    ] {
        let directory = temporary_fixture(name);
        let binding = directory.join("generated_bindings.dm");
        let mut bytes = std::fs::read(&binding).unwrap();
        bytes.extend_from_slice(comment);
        std::fs::write(&binding, bytes).unwrap();
        let mut document = document();
        let mut required_tokens = vec!["#define MERIDIAN_FIXTURE_PROTOCOL 4"];
        required_tokens.extend(tokens);
        document["required_tokens"] = json!(required_tokens);
        let manifest = write_document(&directory, &document);
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
        );
        let state = ServerState::new();
        let parsed = call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path": directory.join("fixture.dme")}),
        )
        .await
        .unwrap();
        assert_ne!(parsed.is_error, Some(true), "{name}");
        assert_eq!(payload(&parsed)["error_count"], 0, "{name}");
        let before = state.snapshot().await.unwrap();
        let checked = call_tool(
            &context,
            &state,
            "dm_check_fixture_sync",
            json!({"fixture_manifest_path": manifest}),
        )
        .await;
        assert!(Arc::ptr_eq(&before, &state.snapshot().await.unwrap()));
        std::fs::remove_dir_all(directory).unwrap();
        let checked = checked.expect("parser-supported source must be searchable for tokens");
        assert_ne!(checked.is_error, Some(true), "{name}");
        assert_eq!(payload(&checked)["classification"], "verified", "{name}");
        assert_eq!(payload(&checked)["issues"], json!([]), "{name}");
    }
}

#[tokio::test]
async fn fixture_tokens_preserve_membership_roles_and_missing_order() {
    let directory = temporary_fixture("token-membership");
    std::fs::write(
        directory.join("configuration.txt"),
        b"\xef\xbb\xbfFILE\nBANNER\r\nCONFIG_TOKEN\r\nLONE\rCR\nEND\r",
    )
    .unwrap();
    std::fs::write(directory.join("native_module.bin"), b"\xffONLY_NATIVE").unwrap();
    std::fs::write(directory.join("service.bin"), b"\xffONLY_SERVICE").unwrap();
    let mut document = document();
    document["inputs"].as_array_mut().unwrap().push(json!({
        "path": "configuration.txt", "role": "configuration",
    }));
    document["required_tokens"] = json!([
        "CONFIG_TOKEN",
        "TOKEN",
        "BANNER\nCONFIG_TOKEN",
        "BANNER\r\nCONFIG_TOKEN",
        "\u{feff}FILE",
        "LONE\rCR",
        "END\r",
        "#define MERIDIAN_FIXTURE_PROTOCOL 4",
        "MISSING",
        "ONLY_NATIVE",
        "MISSING",
        "ONLY_SERVICE",
        // Configuration sorts immediately before the generated binding. This
        // token would exist if the scanner concatenated those files.
        "END\r#define MERIDIAN_FIXTURE_PROTOCOL 4",
    ]);
    let manifest = write_document(&directory, &document);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
    );
    let checked = call_tool(
        &context,
        &ServerState::new(),
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest}),
    )
    .await;
    std::fs::remove_dir_all(directory).unwrap();
    let checked = checked.unwrap();
    assert_ne!(checked.is_error, Some(true));
    assert_eq!(payload(&checked)["classification"], "invalid");
    assert_eq!(
        payload(&checked)["issues"],
        json!([
            {"code": "required_token_missing", "path": "MISSING"},
            {"code": "required_token_missing", "path": "ONLY_NATIVE"},
            {"code": "required_token_missing", "path": "MISSING"},
            {"code": "required_token_missing", "path": "ONLY_SERVICE"},
            {"code": "required_token_missing", "path": "END\r#define MERIDIAN_FIXTURE_PROTOCOL 4"},
        ]),
    );
}

async fn fixture_sync_after_edit(
    name: &str,
    file: &str,
    replacement: &str,
) -> anyhow::Result<Value> {
    let directory = temporary_fixture(name);
    let manifest = write_document(&directory, &document());
    // Establish a reusable baseline without timing-dependent sleeps. The edit
    // below must invalidate a previously settled, otherwise usable snapshot.
    for file in ["fixture.dme", "fixture.dm", "generated_bindings.dm"] {
        std::fs::File::options()
            .write(true)
            .open(directory.join(file))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(60))
            .unwrap();
    }
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::new();
    let parsed = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path": directory.join("fixture.dme")}),
    )
    .await
    .unwrap();
    assert_ne!(parsed.is_error, Some(true));
    let active = state.snapshot().await.unwrap();
    assert!(active.source_fingerprint.is_reusable());
    let baseline = call_tool(
        &context,
        &state,
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest}),
    )
    .await
    .unwrap();
    assert_eq!(payload(&baseline)["classification"], "verified");

    std::fs::write(directory.join(file), replacement).unwrap();
    let checked = call_tool(
        &context,
        &state,
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest}),
    )
    .await;
    let after = state.snapshot().await.unwrap();
    assert_eq!(after.generation, active.generation);
    assert!(
        Arc::ptr_eq(&active, &after),
        "validation must preserve active analysis"
    );
    let result = checked.map(|checked| payload(&checked));
    if let Ok(result) = &result {
        assert_eq!(result["provenance_status"], "unverified");
    }
    std::fs::remove_dir_all(directory).unwrap();
    result
}

#[tokio::test]
async fn fixture_sync_detects_proc_removed_after_active_parse() {
    let checked = fixture_sync_after_edit(
        "removed-after-parse",
        "generated_bindings.dm",
        "#define MERIDIAN_FIXTURE_PROTOCOL 4\n",
    )
    .await
    .unwrap();
    assert_eq!(checked["classification"], "invalid");
    assert_eq!(checked["issues"][0]["code"], "required_proc_missing");
}

#[tokio::test]
async fn fixture_sync_detects_signature_changed_after_active_parse() {
    let checked = fixture_sync_after_edit(
        "signature-after-parse",
        "generated_bindings.dm",
        "#define MERIDIAN_FIXTURE_PROTOCOL 4\n/proc/meridian_fixture_state_batch(changed_payload)\n\treturn length(changed_payload)\n",
    )
    .await
    .unwrap();
    assert_eq!(checked["classification"], "invalid");
    assert_eq!(
        checked["issues"][0]["code"],
        "required_proc_arguments_mismatch"
    );
    assert_eq!(
        checked["issues"][0]["expected_arguments"],
        json!(["payload"])
    );
    assert_eq!(
        checked["issues"][0]["actual_arguments"],
        json!(["changed_payload"])
    );
}

#[tokio::test]
async fn fixture_sync_detects_changed_dme_outside_declared_inputs() {
    let checked = fixture_sync_after_edit(
        "dme-after-parse",
        "fixture.dme",
        "#include \"fixture.dm\"\n",
    )
    .await
    .unwrap();
    // The binding still exists and satisfies the text token, but the current
    // DME no longer includes it. Manifest input hashes alone cannot catch this.
    assert_eq!(checked["classification"], "invalid");
    assert_eq!(checked["issues"][0]["code"], "required_proc_missing");
}

#[tokio::test]
async fn fixture_sync_rejects_incomplete_parse_after_active_parse() {
    let error = fixture_sync_after_edit(
        "missing-include-after-parse",
        "fixture.dme",
        "#include \"fixture.dm\"\n#include \"generated_bindings.dm\"\n#include \"missing.dm\"\n",
    )
    .await
    .expect_err("an unreadable include must not validate against the old tree");
    assert!(error
        .to_string()
        .contains("fixture DreamMaker parse failed"));
}

#[cfg(unix)]
#[test]
fn symlink_inputs_are_rejected() {
    use std::os::unix::fs::symlink;
    let directory = temporary_fixture("symlink");
    std::fs::remove_file(directory.join("native_module.bin")).unwrap();
    symlink(
        directory.join("service.bin"),
        directory.join("native_module.bin"),
    )
    .unwrap();
    let manifest = write_document(&directory, &document());
    let policy = PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap();
    assert!(FixtureManifest::load(&policy, &manifest).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fixture_sync_bounds_large_arguments_without_losing_classification() {
    let directory = temporary_fixture("large-arguments");
    let mut document = document();
    document["required_procs"][0]["arguments"] = json!(vec!["a".repeat(4096); 500]);
    let manifest = write_document(&directory, &document);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest}),
    )
    .await
    .unwrap();
    let ToolContent::Text { text } = &result.content[0];
    assert!(
        text.len() < 1_048_576,
        "classification must survive the transport cap"
    );
    let value = payload(&result);
    assert_eq!(value["classification"], "invalid");
    assert_eq!(value["validation_complete"], true);
    assert_eq!(value["issues_summary"]["total"], 1);
    assert_eq!(
        value["issues"][0]["code"],
        "required_proc_arguments_mismatch"
    );
    assert!(value["issues"][0].get("expected_arguments").is_none());
    assert_eq!(value["issues"][0]["expected_arguments_omitted"], 500);
    assert_eq!(value["issues"][0]["actual_arguments"], json!(["payload"]));
    assert_eq!(value["truncated"], true);
    assert_eq!(value["provenance_status"], "unverified");
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fixture_sync_bounds_escaped_issues_and_counts_every_requirement() {
    let directory = temporary_fixture("escaped-issues");
    let mut document = document();
    // The input fits the manifest cap, but JSON escaping amplifies each row.
    let tokens = (0..200)
        .map(|index| format!("{index}:{}", "\u{0001}".repeat(3000)))
        .collect::<Vec<_>>();
    document["required_tokens"] = json!(tokens);
    let manifest = write_document(&directory, &document);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::new();
    let result = call_tool(
        &context,
        &state,
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest, "issue_limit": 200}),
    )
    .await
    .unwrap();
    let ToolContent::Text { text } = &result.content[0];
    assert!(text.len() < 1_048_576);
    let value = payload(&result);
    assert_eq!(value["classification"], "invalid");
    assert_eq!(value["validation_complete"], true);
    assert_eq!(value["issues_summary"]["total"], 200);
    let returned = value["issues"].as_array().unwrap();
    assert!(!returned.is_empty() && returned.len() < 200);
    for (index, issue) in returned.iter().enumerate() {
        assert_eq!(issue["path"], tokens[index]);
    }
    assert_eq!(value["issues_summary"]["returned"], returned.len());
    assert_eq!(value["issues_summary"]["omitted"], 200 - returned.len());
    assert_eq!(value["truncated"], true);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fixture_sync_summary_only_still_evaluates_requirements() {
    let directory = temporary_fixture("summary-only");
    let mut document = document();
    document["required_procs"][0]["arguments"] = json!(["wrong"]);
    document["required_tokens"] = json!(["MISSING_A", "MISSING_B"]);
    let manifest = write_document(&directory, &document);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_check_fixture_sync",
        json!({"fixture_manifest_path": manifest, "issue_limit": 0}),
    )
    .await
    .unwrap();
    let value = payload(&result);
    assert_eq!(value["classification"], "invalid");
    assert_eq!(value["validation_complete"], true);
    assert_eq!(value["issues"], json!([]));
    assert_eq!(value["issues_summary"]["total"], 3);
    assert_eq!(value["issues_summary"]["omitted"], 3);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fixture_sync_rejects_invalid_issue_limits() {
    let root = checked_fixture();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    for limit in [json!(-1), json!(201), json!(1.5), json!("2"), json!(null)] {
        let result = call_tool(
            &context,
            &ServerState::new(),
            "dm_check_fixture_sync",
            json!({"fixture_manifest_path": root.join("fixture-manifest.json"), "issue_limit": limit}),
        )
        .await;
        assert!(result.is_err(), "invalid limit accepted: {limit}");
    }
}

#[cfg(unix)]
#[test]
fn manifest_preserves_case_distinct_input_paths() {
    let directory = temporary_fixture("case-distinct-inputs");
    std::fs::write(directory.join("case.dm"), "// lower case\n").unwrap();
    std::fs::write(directory.join("CASE.dm"), "// upper case\n").unwrap();
    let mut document = document();
    document["inputs"].as_array_mut().unwrap().extend([
        json!({"path": "case.dm", "role": "source"}),
        json!({"path": "CASE.dm", "role": "source"}),
    ]);
    let manifest = write_document(&directory, &document);
    let policy = PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap();
    let result = FixtureManifest::load(&policy, &manifest);
    std::fs::remove_dir_all(directory).unwrap();
    let fixture = result.expect("case-distinct files are separate inputs on Unix");
    assert_eq!(fixture.inputs.len(), 6);
    assert_ne!(
        fixture
            .inputs
            .iter()
            .find(|input| input.relative_path == "case.dm")
            .unwrap()
            .sha256,
        fixture
            .inputs
            .iter()
            .find(|input| input.relative_path == "CASE.dm")
            .unwrap()
            .sha256
    );
}
