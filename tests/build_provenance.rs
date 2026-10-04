use meridian_mcp::{
    BuildAttemptOutcome, BuildInputIdentity, BuildProvenanceStore, BuildRecord, EffectiveRoot,
    FileIdentity, PathPolicy, PrivateStateStore, ProvenanceStatus, RepositoryIdentity, RootSource,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ProvenanceFixture {
    base: PathBuf,
    workspace: PathBuf,
    state: PathBuf,
    input: PathBuf,
    dmb: PathBuf,
    rsc: PathBuf,
}

impl ProvenanceFixture {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "meridian-mcp-provenance-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let workspace = base.join("workspace");
        let state = base.join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let input = workspace.join("fixture.dm");
        let dmb = workspace.join("fixture.dmb");
        let rsc = workspace.join("fixture.rsc");
        std::fs::write(&input, "source-v1").unwrap();
        std::fs::write(&dmb, "dmb-v1").unwrap();
        std::fs::write(&rsc, "rsc-v1").unwrap();
        Self {
            base,
            workspace,
            state,
            input,
            dmb,
            rsc,
        }
    }

    fn root(&self, digest: &str) -> EffectiveRoot {
        EffectiveRoot {
            path: self.workspace.canonicalize().unwrap(),
            source: RootSource::ExplicitRoot,
            repository_identity: Some(RepositoryIdentity {
                kind: "git_common_directory_sha256",
                digest: digest.to_owned(),
            }),
            head_revision: Some("revision-a".to_owned()),
            dirty: Some(false),
        }
    }

    fn store(&self, digest: &str) -> BuildProvenanceStore {
        let roots = vec![self.root(digest)];
        let private = Arc::new(PrivateStateStore::open(&self.state, &roots).unwrap());
        let policy = PathPolicy::from_effective_roots(roots, Vec::new()).unwrap();
        BuildProvenanceStore::new(private, policy)
    }

    fn success(&self, store: &BuildProvenanceStore) -> BuildRecord {
        BuildRecord {
            schema: 2,
            record_id: "record-success".to_owned(),
            artifact_key: store.artifact_key(&self.dmb).unwrap(),
            mcp_build: meridian_mcp::build_identity::current().clone(),
            compiler: FileIdentity::capture(&self.input).unwrap(),
            project: store.project_identity(&self.dmb).unwrap(),
            inputs: vec![
                BuildInputIdentity::capture(&self.workspace, &self.input, "source").unwrap(),
            ],
            verification: Some(meridian_mcp::build_provenance::BuildVerification {
                method: "literal_dm_closure_v1".to_owned(),
                arguments: vec!["fixture.dme".to_owned()],
                working_directory: self.workspace.clone(),
                absent_inputs: Vec::new(),
            }),
            dmb: FileIdentity::capture(&self.dmb).unwrap(),
            rsc: Some(FileIdentity::capture(&self.rsc).unwrap()),
            fixture_manifest_sha256: None,
            created_at_unix_ms: 1,
        }
    }

    fn write_legacy(&self, mut record: BuildRecord) -> BuildRecord {
        use sha2::{Digest, Sha256};
        let store = PrivateStateStore::open(&self.state, &[self.root("repo-a")]).unwrap();
        let relative = record
            .dmb
            .path
            .strip_prefix(&record.project.root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        record.artifact_key = format!(
            "{:x}",
            Sha256::digest(
                format!("{}\n{relative}", record.project.repository_identity).as_bytes()
            )
        );
        let path = record
            .dmb
            .path
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let path = if cfg!(windows) {
            path.to_ascii_lowercase()
        } else {
            path
        };
        let location = format!("{:x}", Sha256::digest(path.as_bytes()));
        store
            .write_json_atomic(&format!("builds/{}.json", record.artifact_key), &record)
            .unwrap();
        store
            .write_json_atomic(
                &format!("locations/{location}.json"),
                &serde_json::json!({"schema": 1, "artifact_key": record.artifact_key}),
            )
            .unwrap();
        record
    }
}

fn record_success(store: &BuildProvenanceStore, record: &BuildRecord) -> anyhow::Result<()> {
    let mut attempt = store.begin_attempt(&record.dmb.path, record.inputs.clone())?;
    attempt.outcome = BuildAttemptOutcome::Succeeded;
    store.finish_attempt(&attempt, Some(record))
}

impl Drop for ProvenanceFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn failed_attempt_makes_last_success_stale_across_reopen() {
    let fixture = ProvenanceFixture::new("failed");
    let store = fixture.store("repo-a");
    let success = fixture.success(&store);
    record_success(&store, &success).unwrap();
    let mut attempt = store.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    attempt.outcome = BuildAttemptOutcome::Failed {
        code: "compiler_failed".to_owned(),
    };
    // Wall-clock rollback cannot make a newer explicit attempt disappear.
    attempt.created_at_unix_ms = 0;
    store.finish_attempt(&attempt, None).unwrap();
    drop(store);

    let reopened = fixture.store("repo-a");
    let decision = reopened.evaluate_launch(&fixture.dmb, false).unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Stale);
    assert!(!decision.allowed);
    assert!(decision
        .reasons
        .iter()
        .any(|reason| reason.code == "later_compile_failed"));
}

#[test]
fn changed_inputs_outputs_and_repository_identity_are_stale() {
    for (name, change, reason) in [
        ("input", "input", "input_changed"),
        ("dmb", "dmb", "dmb_changed"),
        ("rsc", "rsc", "rsc_changed"),
    ] {
        let fixture = ProvenanceFixture::new(name);
        let store = fixture.store("repo-a");
        record_success(&store, &fixture.success(&store)).unwrap();
        match change {
            "input" => std::fs::write(&fixture.input, "source-v2").unwrap(),
            "dmb" => std::fs::write(&fixture.dmb, "dmb-v2").unwrap(),
            "rsc" => std::fs::write(&fixture.rsc, "rsc-v2").unwrap(),
            _ => unreachable!(),
        }
        let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
        assert_eq!(decision.status, ProvenanceStatus::Stale, "{name}");
        assert!(
            decision.reasons.iter().any(|item| item.code == reason),
            "{name}: {decision:#?}"
        );
    }

    let fixture = ProvenanceFixture::new("repository");
    let store = fixture.store("repo-a");
    record_success(&store, &fixture.success(&store)).unwrap();
    drop(store);
    let changed_repository = fixture.store("repo-b");
    let decision = changed_repository
        .evaluate_launch(&fixture.dmb, false)
        .unwrap();
    assert!(decision
        .reasons
        .iter()
        .any(|reason| reason.code == "repository_identity_changed"));
}

#[test]
fn unmanaged_artifacts_are_unverified_and_can_require_verification() {
    let fixture = ProvenanceFixture::new("unmanaged");
    let store = fixture.store("repo-a");
    let permissive = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert_eq!(permissive.status, ProvenanceStatus::Unverified);
    assert!(permissive.allowed);
    let strict = store.evaluate_launch(&fixture.dmb, true).unwrap();
    assert_eq!(strict.status, ProvenanceStatus::Unverified);
    assert!(!strict.allowed);
}

#[test]
fn a_location_record_cannot_verify_another_artifact() {
    let fixture = ProvenanceFixture::new("misdirected-location");
    let store = fixture.store("repo-a");
    let first = fixture.success(&store);
    fixture.write_legacy(first);
    let original_location = std::fs::read_dir(fixture.state.join("locations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let other = fixture.workspace.join("other.dmb");
    std::fs::write(&other, b"other compiled bytes").unwrap();
    let mut second = fixture.success(&store);
    second.artifact_key = store.artifact_key(&other).unwrap();
    second.dmb = FileIdentity::capture(&other).unwrap();
    let second = fixture.write_legacy(second);
    std::fs::write(
        &original_location,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "artifact_key": second.artifact_key,
        }))
        .unwrap(),
    )
    .unwrap();
    let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Stale);
    assert!(!decision.allowed);
    assert!(decision
        .reasons
        .iter()
        .any(|reason| reason.code == "artifact_location_changed"));
}

#[cfg(unix)]
#[test]
fn case_distinct_artifacts_keep_separate_provenance() {
    let fixture = ProvenanceFixture::new("case-distinct");
    let store = fixture.store("repo-a");
    record_success(&store, &fixture.success(&store)).unwrap();
    let other = fixture.workspace.join("Fixture.dmb");
    std::fs::write(&other, b"uppercase artifact").unwrap();
    let mut second = fixture.success(&store);
    second.artifact_key = store.artifact_key(&other).unwrap();
    second.dmb = FileIdentity::capture(&other).unwrap();
    record_success(&store, &second).unwrap();
    std::fs::write(&fixture.dmb, b"changed lowercase artifact").unwrap();
    let changed = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert_eq!(changed.status, ProvenanceStatus::Stale);
    assert!(!changed.allowed);
    assert_eq!(
        store.evaluate_launch(&other, false).unwrap().status,
        ProvenanceStatus::Verified
    );
}

#[cfg(unix)]
#[test]
fn legacy_location_keys_retain_stale_checks() {
    use sha2::{Digest, Sha256};
    let fixture = ProvenanceFixture::new("Legacy-Location");
    let store = fixture.store("repo-a");
    fixture.write_legacy(fixture.success(&store));
    let current_location = std::fs::read_dir(fixture.state.join("locations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let legacy_name = format!(
        "{:x}.json",
        Sha256::digest(
            fixture
                .dmb
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase()
                .as_bytes()
        )
    );
    let legacy_location = fixture.state.join("locations").join(legacy_name);
    if current_location != legacy_location {
        std::fs::rename(current_location, &legacy_location).unwrap();
    }
    assert_eq!(
        store.evaluate_launch(&fixture.dmb, false).unwrap().status,
        ProvenanceStatus::Verified
    );
    std::fs::write(&fixture.dmb, b"changed legacy artifact").unwrap();
    let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Stale);
    assert!(!decision.allowed);
}

#[test]
fn not_yet_compiled_artifact_is_unverified_instead_of_an_io_error() {
    let fixture = ProvenanceFixture::new("missing-artifact");
    std::fs::remove_file(&fixture.dmb).unwrap();
    let decision = fixture
        .store("repo-a")
        .evaluate_launch(&fixture.dmb, false)
        .unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Unverified);
    assert!(decision.allowed);
}

#[test]
fn legacy_records_remain_readable_unverified_and_stale_checks_still_apply() {
    let fixture = ProvenanceFixture::new("legacy");
    let store = fixture.store("repo-a");
    let mut record = fixture.success(&store);
    record.schema = 1;
    record.verification = None;
    fixture.write_legacy(record);
    let decision = store.evaluate_launch(&fixture.dmb, true).unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Unverified);
    assert!(!decision.allowed);
    std::fs::write(&fixture.input, "source-v2").unwrap();
    assert_eq!(
        store.evaluate_launch(&fixture.dmb, false).unwrap().status,
        ProvenanceStatus::Stale
    );
}

#[test]
fn appearing_configuration_input_invalidates_a_verified_record() {
    let fixture = ProvenanceFixture::new("config-appeared");
    let store = fixture.store("repo-a");
    let mut record = fixture.success(&store);
    let optional = fixture.workspace.join("SpacemanDMM.toml");
    record
        .verification
        .as_mut()
        .unwrap()
        .absent_inputs
        .push(optional.clone());
    record_success(&store, &record).unwrap();
    assert_eq!(
        store.evaluate_launch(&fixture.dmb, true).unwrap().status,
        ProvenanceStatus::Verified
    );
    std::fs::write(optional, "[environment]\n").unwrap();
    let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Stale);
    assert!(decision
        .reasons
        .iter()
        .any(|reason| reason.code == "input_appeared"));
}

#[test]
fn linked_worktrees_with_a_shared_store_keep_independent_builds_and_attempts() {
    let fixture = ProvenanceFixture::new("linked-worktrees");
    let git = |directory: &std::path::Path, args: &[&std::ffi::OsStr]| {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&fixture.workspace, &["init".as_ref()]);
    git(&fixture.workspace, &["add".as_ref(), ".".as_ref()]);
    git(
        &fixture.workspace,
        &["commit".as_ref(), "-m".as_ref(), "fixture".as_ref()],
    );
    let linked = fixture.base.join("linked");
    git(
        &fixture.workspace,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            "--detach".as_ref(),
            linked.as_os_str(),
        ],
    );
    let roots = meridian_mcp::repository_roots::expand_effective_roots(
        &[],
        std::slice::from_ref(&fixture.workspace),
    )
    .unwrap();
    let private = Arc::new(PrivateStateStore::open(&fixture.state, &roots).unwrap());
    let policy = PathPolicy::from_effective_roots(roots, Vec::new()).unwrap();
    let store = BuildProvenanceStore::new(private, policy);
    let first = fixture.success(&store);
    record_success(&store, &first).unwrap();
    let mut second = first.clone();
    second.record_id = "linked-success".to_owned();
    second.dmb = FileIdentity::capture(&linked.join("fixture.dmb")).unwrap();
    second.rsc = Some(FileIdentity::capture(&linked.join("fixture.rsc")).unwrap());
    second.artifact_key = store.artifact_key(&second.dmb.path).unwrap();
    second.project = store.project_identity(&second.dmb.path).unwrap();
    second.inputs =
        vec![BuildInputIdentity::capture(&linked, &linked.join("fixture.dm"), "source").unwrap()];
    record_success(&store, &second).unwrap();
    assert_eq!(
        store.evaluate_launch(&fixture.dmb, true).unwrap().status,
        ProvenanceStatus::Verified
    );
    assert_eq!(
        store
            .evaluate_launch(&second.dmb.path, true)
            .unwrap()
            .status,
        ProvenanceStatus::Verified
    );
    assert_ne!(first.artifact_key, second.artifact_key);
    let mut attempt = store.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    attempt.outcome = BuildAttemptOutcome::Failed {
        code: "compiler_failed".to_owned(),
    };
    // Wall-clock rollback cannot make a newer explicit attempt disappear.
    attempt.created_at_unix_ms = 0;
    store.finish_attempt(&attempt, None).unwrap();
    assert!(!store.evaluate_launch(&fixture.dmb, false).unwrap().allowed);
    assert_eq!(
        store
            .evaluate_launch(&second.dmb.path, true)
            .unwrap()
            .status,
        ProvenanceStatus::Verified
    );
}

#[test]
fn a_failed_first_managed_attempt_cannot_fall_back_to_unmanaged_launch() {
    let fixture = ProvenanceFixture::new("first-failed-attempt");
    let store = fixture.store("repo-a");
    let mut attempt = store.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    attempt.outcome = BuildAttemptOutcome::Failed {
        code: "compiler_failed".to_owned(),
    };
    // Wall-clock rollback cannot make a newer explicit attempt disappear.
    attempt.created_at_unix_ms = 0;
    store.finish_attempt(&attempt, None).unwrap();
    drop(store);
    let decision = fixture
        .store("repo-a")
        .evaluate_launch(&fixture.dmb, false)
        .unwrap();
    assert_eq!(decision.status, ProvenanceStatus::Stale);
    assert!(!decision.allowed);
}

#[test]
fn interrupted_attempt_survives_reopen_and_only_current_completion_can_publish() {
    let fixture = ProvenanceFixture::new("interrupted-attempt");
    let store = fixture.store("repo-a");
    record_success(&store, &fixture.success(&store)).unwrap();
    let mut abandoned = store.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    drop(store);
    let reopened = fixture.store("repo-a");
    let decision = reopened.evaluate_launch(&fixture.dmb, false).unwrap();
    assert!(!decision.allowed);
    assert!(decision
        .reasons
        .iter()
        .any(|reason| reason.code == "build_in_progress_or_interrupted"));
    let mut current = reopened.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    abandoned.outcome = BuildAttemptOutcome::Succeeded;
    assert!(reopened
        .finish_attempt(&abandoned, Some(&fixture.success(&reopened)))
        .is_err());
    assert!(
        !reopened
            .evaluate_launch(&fixture.dmb, false)
            .unwrap()
            .allowed
    );
    current.outcome = BuildAttemptOutcome::Succeeded;
    reopened
        .finish_attempt(&current, Some(&fixture.success(&reopened)))
        .unwrap();
    assert_eq!(
        reopened.evaluate_launch(&fixture.dmb, true).unwrap().status,
        ProvenanceStatus::Verified
    );
    assert!(reopened
        .finish_attempt(&current, Some(&fixture.success(&reopened)))
        .is_err());
}

#[test]
fn incomplete_legacy_state_requires_recovery_even_without_verified_requirement() {
    for missing in ["builds", "locations"] {
        let fixture = ProvenanceFixture::new(missing);
        let store = fixture.store("repo-a");
        fixture.write_legacy(fixture.success(&store));
        std::fs::remove_dir_all(fixture.state.join(missing)).unwrap();
        let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
        assert!(!decision.allowed, "missing {missing}");
        assert_eq!(decision.status, ProvenanceStatus::Stale);
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.code == "build_recovery_required"));
    }
}

#[test]
fn failed_completion_does_not_erase_legacy_evidence_or_earlier_success() {
    let fixture = ProvenanceFixture::new("legacy-preserved");
    let store = fixture.store("repo-a");
    let legacy = fixture.write_legacy(fixture.success(&store));
    let path = fixture
        .state
        .join(format!("builds/{}.json", legacy.artifact_key));
    let bytes = std::fs::read(&path).unwrap();
    let mut attempt = store.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    attempt.outcome = BuildAttemptOutcome::Failed {
        code: "compiler_failed".to_owned(),
    };
    store.finish_attempt(&attempt, None).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert!(!decision.allowed);
    assert_eq!(decision.record_id.as_deref(), Some("record-success"));
}

#[test]
fn supported_aliases_share_location_but_hard_linked_outputs_are_rejected() {
    let fixture = ProvenanceFixture::new("aliases");
    let store = fixture.store("repo-a");
    let alias = fixture.workspace.join(".").join("fixture.dmb");
    assert_eq!(
        store.artifact_key(&fixture.dmb).unwrap(),
        store.artifact_key(&alias).unwrap()
    );
    let hardlink = fixture.workspace.join("linked.dmb");
    std::fs::hard_link(&fixture.dmb, hardlink).unwrap();
    assert!(store.artifact_key(&fixture.dmb).is_err());
}

#[test]
fn missing_v2_publication_never_resurrects_a_legacy_success() {
    let fixture = ProvenanceFixture::new("missing-v2-publication");
    let store = fixture.store("repo-a");
    fixture.write_legacy(fixture.success(&store));
    let attempt = store.begin_attempt(&fixture.dmb, Vec::new()).unwrap();
    let record = fixture
        .state
        .join(format!("artifacts-v2/{}/state.json", attempt.artifact_key));
    std::fs::remove_file(record).unwrap();
    let decision = store.evaluate_launch(&fixture.dmb, false).unwrap();
    assert!(!decision.allowed);
    assert_eq!(decision.status, ProvenanceStatus::Stale);
}
