use meridian_mcp::tracy_artifact::{
    compare_metadata, reserve_trace_set, validate_capture_result, ComparisonMode, RawRange,
    TraceMetadata, TraceSetError,
};
use meridian_mcp::tracy_experiment::{
    ExecutableIdentity, ExperimentIdentity, HelperIdentity, WorkloadIdentity,
};
use meridian_mcp::PathPolicy;
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "meridian-trace-set-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn metadata_fixture() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    use sha2::{Digest, Sha256};
    let root = fixture();
    let trace = root.join("sample.tracy");
    std::fs::write(&trace, b"owned trace").unwrap();
    let sidecar = root.join("sample.tracy.meridian.json");
    let metadata = metadata("experiment", "executable", "workload", "steady");
    std::fs::write(
        &sidecar,
        serde_json::to_vec(&json!({
            "schema": 2,
            "trace_sha256": format!("{:x}", Sha256::digest(b"owned trace")),
            "meridian_mcp_build": {"build_id": "owned-test-build"},
            "experiment_identity": metadata.experiment_identity,
            "phase": "steady", "phase_iteration": 1,
            "capture": {"validation": {
                "raw_begin": 1, "raw_end": 10,
                "trace_begin_ns": 1, "trace_end_ns": 10,
                "complete_frames": 3, "partial_frames": 0, "zones": 1,
                "valid": true, "queue": {"saturation_count": 0, "dropped_events": 0}
            }},
            "memory_series": [
                {"identity": {"role": "dream_daemon"}},
                {"identity": {"role": "collector"}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    (root, trace, sidecar)
}

#[test]
fn trace_metadata_rejects_an_oversized_sidecar() {
    let (root, trace, sidecar) = metadata_fixture();
    assert!(meridian_mcp::tracy_artifact::read_trace_metadata(
        &PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        &trace
    )
    .unwrap()
    .is_some());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&sidecar)
        .unwrap();
    for _ in 0..1024 {
        file.write_all(&[b' '; 16 * 1024]).unwrap();
    }
    drop(file);
    let result = meridian_mcp::tracy_artifact::read_trace_metadata(
        &PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        &trace,
    );
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        result.is_err(),
        "oversized valid JSON must be rejected before loading it"
    );
}

#[test]
fn a_sidecar_directory_is_not_treated_as_missing_metadata() {
    let (root, trace, sidecar) = metadata_fixture();
    std::fs::remove_file(&sidecar).unwrap();
    assert!(meridian_mcp::tracy_artifact::read_trace_metadata(
        &PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        &trace
    )
    .unwrap()
    .is_none());
    std::fs::create_dir(&sidecar).unwrap();
    let result = meridian_mcp::tracy_artifact::read_trace_metadata(
        &PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        &trace,
    );
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn trace_metadata_rejects_a_sidecar_symlink() {
    let (root, trace, sidecar) = metadata_fixture();
    let outside = fixture();
    let external = outside.join("external.json");
    std::fs::rename(&sidecar, &external).unwrap();
    std::os::unix::fs::symlink(&external, &sidecar).unwrap();
    let result = meridian_mcp::tracy_artifact::read_trace_metadata(
        &PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        &trace,
    );
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
    assert!(result.is_err());
}

#[test]
fn promotes_exact_trace_and_schema_sidecar_pair() {
    let root = fixture();
    let trace = root.join("sample.tracy");
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    let reserved = reserve_trace_set(&policy, &trace, false).unwrap();
    std::fs::write(reserved.temporary_trace_path(), b"standard-tracy-bytes").unwrap();
    let promoted = reserved.promote(&json!({"schema":2})).unwrap();
    assert_eq!(promoted.trace.path, trace.canonicalize().unwrap());
    let sidecar = root.join("sample.tracy.meridian.json");
    assert_eq!(promoted.sidecar.path, sidecar.canonicalize().unwrap());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(sidecar).unwrap()).unwrap()
            ["schema"],
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_capture_is_retained_only_as_a_non_authoritative_diagnostic() {
    let root = fixture();
    let trace = root.join("authoritative.tracy");
    let diagnostics = root.join("diagnostics");
    std::fs::create_dir_all(&diagnostics).unwrap();
    let diagnostic_trace = diagnostics.join("steady-state-1.invalid.tracy");
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    let reserved = reserve_trace_set(&policy, &trace, false).unwrap();
    std::fs::write(reserved.temporary_trace_path(), b"invalid-tracy-bytes").unwrap();

    let retained = reserved
        .promote_diagnostic(
            &policy,
            &diagnostic_trace,
            &json!({
                "schema": 2,
                "authoritative": false,
                "validation": {"valid": false, "error_codes": ["zero_zones"]},
            }),
        )
        .unwrap();

    assert!(!trace.exists());
    assert!(!root.join("authoritative.tracy.meridian.json").exists());
    assert!(!retained.authoritative);
    assert_eq!(
        retained.trace.path,
        diagnostic_trace.canonicalize().unwrap()
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(&retained.sidecar.path).unwrap()
        )
        .unwrap()["validation"]["error_codes"],
        json!(["zero_zones"])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_publication_rejects_invalid_or_malformed_helper_results() {
    let invalid = json!({
        "validation": {
            "valid": false,
            "raw_begin": 10,
            "raw_end": 20,
            "trace_begin_ns": 100,
            "trace_end_ns": 200,
            "complete_frames": 3,
            "zones": 0,
            "error_codes": ["zero_zones"],
        }
    });
    assert_eq!(
        validate_capture_result(&invalid).unwrap_err(),
        vec!["zero_zones"]
    );

    let malformed = json!({"validation": {"valid": true}});
    assert_eq!(
        validate_capture_result(&malformed).unwrap_err(),
        vec![
            "missing_raw_range",
            "missing_trace_range",
            "no_complete_frames",
            "zero_zones"
        ]
    );

    let valid = json!({
        "validation": {
            "valid": true,
            "raw_begin": 10,
            "raw_end": 20,
            "trace_begin_ns": 100,
            "trace_end_ns": 200,
            "complete_frames": 3,
            "zones": 4,
            "error_codes": [],
        }
    });
    assert_eq!(validate_capture_result(&valid), Ok(()));
}

#[test]
fn refuses_collisions_and_overwrite_before_capture() {
    let root = fixture();
    let trace = root.join("sample.tracy");
    std::fs::write(&trace, b"human-owned").unwrap();
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    assert!(reserve_trace_set(&policy, &trace, false).is_err());
    assert!(matches!(
        reserve_trace_set(&policy, &root.join("new.tracy"), true),
        Err(TraceSetError::OverwriteUnsupported)
    ));
    assert_eq!(std::fs::read(&trace).unwrap(), b"human-owned");
    std::fs::remove_dir_all(root).unwrap();
}

fn metadata(
    experiment_id: &str,
    executable_id: &str,
    workload_id: &str,
    phase: &str,
) -> TraceMetadata {
    TraceMetadata {
        trace_sha256: "00".repeat(32),
        meridian_mcp_build_id: "mcp-build-a".into(),
        experiment_identity: ExperimentIdentity {
            experiment_id: experiment_id.into(),
            executable: ExecutableIdentity {
                schema: 1,
                executable_id: executable_id.into(),
                repository_revision: None,
                repository_dirty_digest: String::new(),
                dmb_sha256: String::new(),
                rsc_sha256: None,
                byond_version: "516.1687".into(),
                byond_executable_sha256: String::new(),
                native_modules: Vec::new(),
                helper_identity: HelperIdentity {
                    source_revision: String::new(),
                    sha256: String::new(),
                    patch_sha256: None,
                },
                hook_identity: HelperIdentity {
                    source_revision: String::new(),
                    sha256: String::new(),
                    patch_sha256: None,
                },
                startup_mode: "tracy".into(),
                launch_parameters_sha256: String::new(),
                build_record_id: None,
            },
            workload: WorkloadIdentity {
                workload_id: workload_id.into(),
                map: None,
                seed: None,
                configuration_profile: None,
                feature_set: Vec::new(),
                scenario: None,
                external_run_id: None,
                annotations: Default::default(),
            },
        },
        phase: phase.into(),
        phase_iteration: 1,
        range: RawRange {
            raw_begin: 1,
            raw_end: 2,
        },
        trace_range_ns: RawRange {
            raw_begin: 1,
            raw_end: 2,
        },
        complete_frames: 10,
        partial_frames: 2,
        zones: 4,
        capture_valid: true,
        queue_saturated: false,
        memory_roles: vec![
            meridian_mcp::process_metrics::ProcessRole::DreamDaemon,
            meridian_mcp::process_metrics::ProcessRole::Collector,
        ],
    }
}

#[test]
fn comparison_requires_identity_compatibility_before_native_analysis() {
    let baseline = metadata("experiment-a", "executable", "workload", "steady_state");
    let mut current = baseline.clone();
    current.phase_iteration = 2;
    assert!(
        compare_metadata(&baseline, &current, ComparisonMode::SameExperimentSamePhase).compatible
    );
    current.phase = "boot".into();
    assert!(
        !compare_metadata(&baseline, &current, ComparisonMode::SameExperimentSamePhase).compatible
    );
    let cross = metadata("experiment-b", "executable", "workload", "steady_state");
    assert!(compare_metadata(&baseline, &cross, ComparisonMode::CrossExperiment).compatible);
    assert!(
        !compare_metadata(&baseline, &cross, ComparisonMode::SameExperimentSamePhase).compatible
    );
    let mut different_mcp = baseline.clone();
    different_mcp.meridian_mcp_build_id = "mcp-build-b".into();
    assert!(
        !compare_metadata(&baseline, &different_mcp, ComparisonMode::CrossExperiment).compatible
    );
}

#[test]
fn saturation_without_data_loss_does_not_disqualify_a_control_capture() {
    let mut saturated = metadata("experiment-a", "executable", "workload", "steady_state");
    saturated.queue_saturated = true;
    assert!(meridian_mcp::tracy_artifact::is_complete_control_capture(
        &saturated
    ));

    saturated.capture_valid = false;
    assert!(!meridian_mcp::tracy_artifact::is_complete_control_capture(
        &saturated
    ));
}
