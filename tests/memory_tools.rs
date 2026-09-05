use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};

mod tempfile {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    pub struct TempDir(PathBuf);
    impl TempDir {
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn tempdir() -> std::io::Result<TempDir> {
        let path = std::env::temp_dir().join(format!(
            "meridian-memory-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(TempDir(path))
    }
}

fn evidence() -> Value {
    json!({
        "schema": 2,
        "meridian_mcp_build": {"build_id": "fixture-build"},
        "experiment_identity": {"experiment_id": "run", "executable": {"executable_id": "exe"}, "workload": {"workload_id": "work"}},
        "phase": "steady",
        "memory_series": [{
            "identity": {"pid": 42, "started_at_identity": 7, "role": "dream_daemon"},
            "operating_system": "windows", "sampling_interval_ms": 500, "missed_samples": 0,
            "samples": [
                {"monotonic_offset_ms": 0, "metric_kind": "private_bytes", "unit": "bytes", "observed_value": 100},
                {"monotonic_offset_ms": 500, "metric_kind": "private_bytes", "unit": "bytes", "observed_value": 400},
                {"monotonic_offset_ms": 1000, "metric_kind": "private_bytes", "unit": "bytes", "observed_value": 200},
                {"monotonic_offset_ms": 1500, "metric_kind": "private_bytes", "unit": "bytes", "observed_value": 9999}
            ]
        }]
    })
}

async fn invoke(name: &str, args: Value, root: &std::path::Path) -> Value {
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.to_owned()], Vec::new()).unwrap(),
    );
    let result = call_tool(&context, &ServerState::new(), name, args)
        .await
        .unwrap();
    let content = serde_json::to_value(result).unwrap();
    serde_json::from_str(content["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn summary_uses_half_open_windows_and_reports_growth_without_leak_claims() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("capture.json");
    std::fs::write(&path, evidence().to_string()).unwrap();
    let result = invoke(
        "dm_memory_summary",
        json!({"evidence_path": path, "begin_ms": 0, "end_ms": 1500, "sample_limit": 2}),
        root.path(),
    )
    .await;
    assert_eq!(result["schema"], 1, "{result}");
    let metric = &result["series"][0]["metrics"][0];
    assert_eq!(metric["sample_count"], 3);
    assert_eq!(metric["first_bytes"], 100);
    assert_eq!(metric["last_bytes"], 200);
    assert_eq!(metric["peak_bytes"], 400);
    assert_eq!(metric["net_change_bytes"], 100);
    assert_eq!(metric["net_bytes_per_second"], 100.0);
    assert_eq!(metric["largest_rise_bytes"], 300);
    assert_eq!(metric["largest_fall_bytes"], 200);
    assert_eq!(metric["samples"].as_array().unwrap().len(), 2);
    assert_eq!(metric["samples_truncated"], true);
    assert_eq!(result["allocation_attribution"], "unavailable");
    assert_eq!(result["identity_verification"], "recorded_not_verified");
    assert!(result["evidence_sha256"].as_str().unwrap().len() == 64);
}

#[tokio::test]
async fn comparison_rejects_changed_workload_even_when_identifier_is_reused() {
    let root = tempfile::tempdir().unwrap();
    let baseline = root.path().join("baseline.json");
    let current = root.path().join("current.json");
    let mut doc = evidence();
    std::fs::write(&baseline, doc.to_string()).unwrap();
    doc["experiment_identity"]["workload"]["seed"] = json!("different");
    std::fs::write(&current, doc.to_string()).unwrap();
    let result = invoke(
        "dm_memory_compare",
        json!({"baseline": {"evidence_path": baseline}, "current": {"evidence_path": current}}),
        root.path(),
    )
    .await;
    assert_eq!(result["code"], "evidence_identity_mismatch", "{result}");
}

#[tokio::test]
async fn comparison_keeps_metric_units_and_process_roles_separate() {
    let root = tempfile::tempdir().unwrap();
    let baseline = root.path().join("baseline.json");
    let current = root.path().join("current.json");
    let doc = evidence();
    std::fs::write(&baseline, doc.to_string()).unwrap();
    let mut next = doc.clone();
    next["memory_series"][0]["identity"]["pid"] = json!(84);
    next["memory_series"][0]["samples"][1]["observed_value"] = json!(500);
    std::fs::write(&current, next.to_string()).unwrap();
    let args = json!({"baseline": {"evidence_path": baseline, "end_ms": 1500}, "current": {"evidence_path": current, "end_ms": 1500}});
    let result = invoke("dm_memory_compare", args.clone(), root.path()).await;
    assert_eq!(
        result["comparisons"][0]["peak_delta_bytes"], 100,
        "{result}"
    );
    next["memory_series"][0]["operating_system"] = json!("linux");
    for sample in next["memory_series"][0]["samples"].as_array_mut().unwrap() {
        sample["metric_kind"] = json!("rss_bytes");
    }
    std::fs::write(&current, next.to_string()).unwrap();
    let result = invoke("dm_memory_compare", args, root.path()).await;
    assert_eq!(result["code"], "evidence_identity_mismatch", "{result}");
}

#[tokio::test]
async fn malformed_samples_and_unauthorized_paths_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("capture.json");
    let mut doc = evidence();
    doc["memory_series"][0]["samples"][1]["monotonic_offset_ms"] = json!(0);
    std::fs::write(&path, doc.to_string()).unwrap();
    let result = invoke(
        "dm_memory_summary",
        json!({"evidence_path": path}),
        root.path(),
    )
    .await;
    assert_eq!(result["code"], "invalid_input", "{result}");
    let outside = tempfile::tempdir().unwrap();
    let result = invoke(
        "dm_memory_summary",
        json!({"evidence_path": path}),
        outside.path(),
    )
    .await;
    assert_eq!(result["code"], "path_outside_workspace", "{result}");
}

#[tokio::test]
async fn single_sample_has_no_rate_and_gaps_are_visible() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("capture.json");
    let mut doc = evidence();
    doc["memory_series"][0]["missed_samples"] = json!(3);
    std::fs::write(&path, doc.to_string()).unwrap();
    let result = invoke(
        "dm_memory_summary",
        json!({"evidence_path": path, "begin_ms": 500, "end_ms": 501}),
        root.path(),
    )
    .await;
    assert_eq!(
        result["series"][0]["metrics"][0]["net_bytes_per_second"],
        Value::Null
    );
    assert_eq!(result["series"][0]["missed_samples"], 3);
    assert_eq!(
        result["series"][0]["metrics"][0]["sample_count"], 1,
        "{result}"
    );
}

#[tokio::test]
async fn input_limits_unknown_fields_and_duplicate_roles_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("capture.json");
    let mut doc = evidence();
    let duplicate = doc["memory_series"][0].clone();
    doc["memory_series"].as_array_mut().unwrap().push(duplicate);
    std::fs::write(&path, doc.to_string()).unwrap();
    assert_eq!(
        invoke(
            "dm_memory_summary",
            json!({"evidence_path": path}),
            root.path()
        )
        .await["code"],
        "invalid_input"
    );
    std::fs::write(&path, evidence().to_string()).unwrap();
    for args in [
        json!({"evidence_path": path, "sample_limit": 101}),
        json!({"evidence_path": path, "begin_ms": 1000, "end_ms": 500}),
        json!({"evidence_path": path, "unexpected": true}),
    ] {
        assert_eq!(
            invoke("dm_memory_summary", args, root.path()).await["code"],
            "invalid_input"
        );
    }
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        invoke(
            "dm_memory_summary",
            json!({"evidence_path": path}),
            root.path()
        )
        .await["code"],
        "invalid_input"
    );
}

#[tokio::test]
async fn phase_changes_require_explicit_opt_in_and_empty_windows_are_not_zero_usage() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("baseline.json");
    let other = root.path().join("other.json");
    let mut doc = evidence();
    std::fs::write(&path, doc.to_string()).unwrap();
    doc["phase"] = json!("after_cleanup");
    doc["memory_series"][0]["samples"][2]["observed_value"] = json!(50);
    std::fs::write(&other, doc.to_string()).unwrap();
    let mut args =
        json!({"baseline": {"evidence_path": path}, "current": {"evidence_path": other}});
    assert_eq!(
        invoke("dm_memory_compare", args.clone(), root.path()).await["code"],
        "evidence_identity_mismatch"
    );
    args["allow_different_phases"] = json!(true);
    let result = invoke("dm_memory_compare", args, root.path()).await;
    assert_eq!(
        result["warnings"][0], "different_phases_descriptive_only",
        "{result}"
    );
    let empty = invoke(
        "dm_memory_summary",
        json!({"evidence_path": path, "begin_ms": 2000}),
        root.path(),
    )
    .await;
    assert_eq!(empty["series"][0]["metrics"], json!([]));
    assert!(empty["warnings"]
        .as_array()
        .unwrap()
        .contains(&json!("no_samples_in_selected_window")));
    let negative = invoke(
        "dm_memory_summary",
        json!({"evidence_path": other, "begin_ms": 500, "end_ms": 1500}),
        root.path(),
    )
    .await;
    assert_eq!(
        negative["series"][0]["metrics"][0]["net_change_bytes"],
        -350
    );
}
