use meridian_mcp::native_evidence::model::*;
use meridian_mcp::native_evidence::{parse_artifact, validate_request, NativeEvidenceContext};
use meridian_mcp::PathPolicy;
use serde_json::json;
use std::fs;

fn fixture() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evidence")
}
fn context() -> NativeEvidenceContext {
    NativeEvidenceContext {
        policy: PathPolicy::new(vec![fixture()], Vec::new()).unwrap(),
        provenance: None,
    }
}

#[cfg(target_os = "linux")]
#[test]
fn evidence_byte_budget_counts_actual_reads_when_metadata_reports_zero() {
    use meridian_mcp::limits::MAX_EVIDENCE_TOTAL_BYTES;
    use meridian_mcp::native_evidence::reader::read_artifact;
    use std::path::PathBuf;

    // procfs supplies regular files with zero metadata length and readable
    // contents, exercising unreliable size metadata without a timing race.
    let root = PathBuf::from("/proc/self").canonicalize().unwrap();
    let path = root.join("stat");
    let metadata = fs::metadata(&path).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 0);
    let policy = PathPolicy::new(vec![root], Vec::new()).unwrap();
    let descriptor = ArtifactDescriptor {
        kind: ArtifactKind::RuntimeJsonl,
        path,
        options: None,
    };
    let mut total = 0;
    let read = read_artifact(&policy, &descriptor, &mut total).unwrap();
    assert!(!read.bytes.is_empty());
    assert_eq!(total, read.identity.bytes);

    let mut total = MAX_EVIDENCE_TOTAL_BYTES - 1;
    assert!(read_artifact(&policy, &descriptor, &mut total).is_err());
    assert_eq!(total, MAX_EVIDENCE_TOTAL_BYTES - 1);
}

#[test]
fn descriptors_are_strict_and_limits_are_validated() {
    assert!(serde_json::from_value::<NativeEvidenceRequest>(
        json!({"artifacts":[{"kind":"auto","path":"evidence.json"}],"surprise":true})
    )
    .is_err());
    let request = NativeEvidenceRequest {
        artifacts: (0..33)
            .map(|_| ArtifactDescriptor {
                kind: ArtifactKind::PerformanceCsv,
                path: fixture().join("performance-lf.csv"),
                options: None,
            })
            .collect(),
        dmb_path: None,
        workload: None,
        phases: Vec::new(),
    };
    assert!(validate_request(&request).is_err());
}

#[test]
fn all_five_explicit_formats_are_bounded_and_hashed() {
    let cases = [
        (
            ArtifactKind::ByondProcProfileJson,
            "proc-profile.json",
            EvidenceSemantics::CumulativeSnapshot,
            1,
        ),
        (
            ArtifactKind::ByondSendmapsJson,
            "sendmaps.json",
            EvidenceSemantics::CumulativeSnapshot,
            1,
        ),
        (
            ArtifactKind::PerformanceCsv,
            "performance-lf.csv",
            EvidenceSemantics::IntervalSeries,
            3,
        ),
        (
            ArtifactKind::RuntimeJsonl,
            "runtime-lf.jsonl",
            EvidenceSemantics::EventStream,
            2,
        ),
        (
            ArtifactKind::EventJsonl,
            "events.jsonl",
            EvidenceSemantics::EventStream,
            2,
        ),
    ];
    let context = context();
    let mut total = 0;
    let mut redacted = 0;
    for (kind, name, semantics, count) in cases {
        let parsed = parse_artifact(
            &context,
            &ArtifactDescriptor {
                kind,
                path: fixture().join(name),
                options: Some(ArtifactOptions {
                    selected_metrics: vec![
                        "tick_usage".into(),
                        "duration_ms".into(),
                        "value".into(),
                        "calls".into(),
                        "send_count".into(),
                    ],
                    wall_time_field: Some("timestamp".into()),
                    ..Default::default()
                }),
            },
            &mut total,
            &mut redacted,
        )
        .unwrap();
        assert_eq!(parsed.semantics, semantics);
        assert_eq!(parsed.accepted_records, count);
        assert_eq!(parsed.identity.sha256.len(), 64);
        assert!(!parsed.identity.relative_path.contains(":"));
    }
    assert!(redacted > 0);
}

#[test]
fn csv_and_jsonl_have_lf_crlf_parity() {
    let temporary = std::env::temp_dir().join(format!(
        "meridian-native-evidence-crlf-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temporary).unwrap();
    for name in ["performance-lf.csv", "runtime-lf.jsonl"] {
        let contents = fs::read_to_string(fixture().join(name)).unwrap();
        fs::write(temporary.join(name), contents.replace('\n', "\r\n")).unwrap();
    }
    let context = NativeEvidenceContext {
        policy: PathPolicy::new(vec![temporary.clone()], Vec::new()).unwrap(),
        provenance: None,
    };
    for (kind, name, expected) in [
        (ArtifactKind::PerformanceCsv, "performance-lf.csv", 3),
        (ArtifactKind::RuntimeJsonl, "runtime-lf.jsonl", 2),
    ] {
        let mut total = 0;
        let mut redacted = 0;
        let parsed = parse_artifact(
            &context,
            &ArtifactDescriptor {
                kind,
                path: temporary.join(name),
                options: None,
            },
            &mut total,
            &mut redacted,
        )
        .unwrap();
        assert_eq!(parsed.accepted_records, expected);
    }
    fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn protected_identifiers_cannot_be_numeric_metrics_or_clocks() {
    let temporary =
        std::env::temp_dir().join(format!("meridian-native-redaction-{}", std::process::id()));
    fs::create_dir_all(&temporary).unwrap();
    let path = temporary.join("identifiers.json");
    fs::write(&path, br#"{"discord_id":1234567,"PLAYER-ID":7654321,"account_id":"private-account","tick_usage":20,"message":"Discord-ID=private-discord player-id=private-player"}"#).unwrap();
    let context = NativeEvidenceContext {
        policy: PathPolicy::new(vec![temporary.clone()], Vec::new()).unwrap(),
        provenance: None,
    };
    for kind in [
        ArtifactKind::ByondProcProfileJson,
        ArtifactKind::ByondSendmapsJson,
        ArtifactKind::RuntimeJsonl,
        ArtifactKind::EventJsonl,
    ] {
        for protected_clocks in [false, true] {
            let mut redacted = 0;
            let parsed = parse_artifact(
                &context,
                &ArtifactDescriptor {
                    kind,
                    path: path.clone(),
                    options: Some(ArtifactOptions {
                        selected_metrics: vec![
                            "tick_usage".into(),
                            "discord_id".into(),
                            "PLAYER-ID".into(),
                        ],
                        group_fields: vec!["message".into(), "account_id".into()],
                        wall_time_field: protected_clocks.then(|| "discord_id".into()),
                        world_time_field: protected_clocks.then(|| "PLAYER-ID".into()),
                    }),
                },
                &mut 0,
                &mut redacted,
            )
            .unwrap();
            let record = &parsed.records[0];
            assert_eq!(
                record
                    .metrics
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["tick_usage"]
            );
            assert_eq!(record.wall_unix_ms, None);
            assert_eq!(record.world_deciseconds, None);
            assert_eq!(record.groups.len(), 1);
            assert_eq!(
                record.groups["message"],
                "Discord-ID=<redacted> player-id=<redacted>"
            );
            assert_eq!(redacted, 5);
        }
    }
    fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn integer_csv_clocks_preserve_values_and_half_open_phase_boundaries() {
    use meridian_mcp::native_evidence::timeline;
    let temporary =
        std::env::temp_dir().join(format!("meridian-native-csv-clocks-{}", std::process::id()));
    fs::create_dir_all(&temporary).unwrap();
    let path = temporary.join("clocks.csv");
    fs::write(&path, "timestamp,world_time,tick_usage\n1767225600000,10,1\n1767225601000,20,2\n1767225602000,30,3\n").unwrap();
    let context = NativeEvidenceContext {
        policy: PathPolicy::new(vec![temporary.clone()], Vec::new()).unwrap(),
        provenance: None,
    };
    let parsed = parse_artifact(
        &context,
        &ArtifactDescriptor {
            kind: ArtifactKind::PerformanceCsv,
            path,
            options: Some(ArtifactOptions {
                wall_time_field: Some("timestamp".into()),
                world_time_field: Some("world_time".into()),
                ..Default::default()
            }),
        },
        &mut 0,
        &mut 0,
    )
    .unwrap();
    let phases = vec![
        PhaseInput {
            id: "startup".into(),
            wall_start: Some("2026-01-01T00:00:00Z".into()),
            wall_end: Some("2026-01-01T00:00:01Z".into()),
            world_start_ds: Some(10),
            world_end_ds: Some(20),
        },
        PhaseInput {
            id: "steady".into(),
            wall_start: Some("2026-01-01T00:00:01Z".into()),
            wall_end: Some("2026-01-01T00:00:02Z".into()),
            world_start_ds: Some(20),
            world_end_ds: Some(30),
        },
    ];
    for (index, record) in parsed.records.iter().enumerate() {
        assert_eq!(
            record.wall_unix_ms,
            Some(1_767_225_600_000 + index as i128 * 1000)
        );
        assert_eq!(record.world_deciseconds, Some(10 + index as i64 * 10));
        assert_eq!(
            timeline::assign(record, &phases).unwrap().as_deref(),
            [Some("startup"), Some("steady"), None][index]
        );
    }
    fs::remove_dir_all(temporary).unwrap();
}
