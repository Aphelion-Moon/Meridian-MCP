use meridian_mcp::{
    BuildAttempt, BuildAttemptOutcome, BuildInputIdentity, BuildProvenanceStore, BuildRecord,
    EffectiveRoot, FileIdentity, PathPolicy, PrivateStateStore, ProvenanceStatus,
    RepositoryIdentity, RootSource,
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
    store.record_success(&success).unwrap();
    store
        .record_attempt(&BuildAttempt {
            schema: 1,
            attempt_id: "attempt-failed".to_owned(),
            artifact_key: success.artifact_key.clone(),
            outcome: BuildAttemptOutcome::Failed {
                code: "compiler_failed".to_owned(),
            },
            observed_inputs: success.inputs.clone(),
            retained_dmb_sha256: Some(success.dmb.sha256.clone()),
            created_at_unix_ms: 2,
        })
        .unwrap();
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
        store.record_success(&fixture.success(&store)).unwrap();
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
    store.record_success(&fixture.success(&store)).unwrap();
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
    store.record_success(&first).unwrap();
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
    store.record_success(&second).unwrap();
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
    store.record_success(&fixture.success(&store)).unwrap();
    let other = fixture.workspace.join("Fixture.dmb");
    std::fs::write(&other, b"uppercase artifact").unwrap();
    let mut second = fixture.success(&store);
    second.artifact_key = store.artifact_key(&other).unwrap();
    second.dmb = FileIdentity::capture(&other).unwrap();
    store.record_success(&second).unwrap();
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
    store.record_success(&fixture.success(&store)).unwrap();
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
    store.record_success(&record).unwrap();
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
    store.record_success(&record).unwrap();
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
