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
