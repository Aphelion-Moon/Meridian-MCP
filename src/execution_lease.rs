//! Cooperative execution ownership. An abandoned active record is quarantined:
//! acquiring its file lock never proves that the previous writer has exited.
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::private_state::PrivateStateLivenessLock;
use crate::{PathPolicy, PrivateStateStore};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AdmissionError {
    #[error("the project execution scope is busy")]
    Busy,
    #[error("the project has unfinished execution state; writer cleanup must be established before recovery")]
    RecoveryRequired,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AdmissionError {
    pub(crate) fn result(&self) -> crate::mcp::ToolResult {
        crate::mcp::ToolResult::structured_error(
            match self {
                Self::Busy => "execution_busy",
                Self::RecoveryRequired => "recovery_required",
                Self::Other(_) => "execution_scope_invalid",
            },
            self.to_string(),
            "Stop the owned session or establish cleanup of the interrupted execution; do not erase state to bypass recovery.",
        )
    }
}

#[derive(Deserialize, Serialize)]
struct ExecutionRecord {
    schema: u32,
    scope: PathBuf,
    operation_id: String,
    kind: String,
    active: bool,
}

pub(crate) struct ExecutionLease {
    store: Arc<PrivateStateStore>,
    record_path: String,
    record: ExecutionRecord,
    _lock: PrivateStateLivenessLock,
    writer_started: bool,
    cleanup_uncertain: bool,
    pub(crate) request_owner: Option<Arc<()>>,
}

impl ExecutionLease {
    pub(crate) fn acquire(
        store: Arc<PrivateStateStore>,
        policy: &PathPolicy,
        artifact: &Path,
        working_directory: &Path,
        kind: &str,
    ) -> Result<Self, AdmissionError> {
        let artifact = crate::artifact_location::canonical_location(artifact)?;
        policy
            .output_path(&artifact, true)
            .map_err(anyhow::Error::from)?;
        let parent = artifact
            .parent()
            .ok_or_else(|| anyhow!("artifact has no project parent"))?;
        let scope = worktree_root(parent)?.unwrap_or_else(|| parent.to_owned());
        let working = policy
            .read_directory(working_directory)
            .map_err(anyhow::Error::from)?;
        if let Some(working_scope) = worktree_root(&working)? {
            if working_scope != scope {
                return Err(anyhow!("working directory and artifact cross Git worktrees").into());
            }
        } else if worktree_root(parent)?.is_some() || !working.starts_with(&scope) {
            return Err(anyhow!("working directory crosses the project execution scope").into());
        }
        // The marker leaf also uses the qualified Windows location case policy.
        let key = crate::artifact_location::location_key(&scope.join(".meridian-execution"))?;
        let lock = store
            .try_acquire_liveness_lock(&format!("execution-locks-v1/{key}.lock"))?
            .ok_or(AdmissionError::Busy)?;
        let record_path = format!("execution-v1/{key}/state.json");
        // Lock order: exact scope, then this brief state transaction. Overlapping
        // scopes can only try each other's locks; waiting would deadlock.
        let transaction = store.transaction()?;
        let records = transaction
            .list_entries_bounded("execution-v1", 4096)
            .map_err(|_| AdmissionError::RecoveryRequired)?;
        // Lock files have a separate namespace: a rejected first admission must
        // not leave a record directory that looks like damaged durable state.
        let namespace = store.root().join("execution-v1");
        for directory in records {
            if directory.parent() != Some(namespace.as_path()) {
                continue;
            }
            let path = directory.join("state.json");
            let relative = path
                .strip_prefix(store.root())
                .ok()
                .and_then(Path::to_str)
                .ok_or(AdmissionError::RecoveryRequired)?;
            let previous: ExecutionRecord = transaction
                .read_json(relative)
                .map_err(|_| AdmissionError::RecoveryRequired)?;
            if previous.schema != 1
                || !previous.scope.is_absolute()
                || previous.operation_id.len() != 32
                || !previous
                    .operation_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(AdmissionError::RecoveryRequired);
            }
            // Retired worktrees may have been removed. Active state requires
            // a valid canonical identity; a missing scope never proves cleanup.
            if !previous.active && !previous.scope.exists() {
                continue;
            }
            let previous_key =
                crate::artifact_location::location_key(&previous.scope.join(".meridian-execution"))
                    .map_err(|_| AdmissionError::RecoveryRequired)?;
            if path
                != store
                    .root()
                    .join(format!("execution-v1/{previous_key}/state.json"))
            {
                return Err(AdmissionError::RecoveryRequired);
            }
            if previous_key == key && previous.scope != scope {
                return Err(AdmissionError::RecoveryRequired);
            }
            if previous.active
                && (scope.starts_with(&previous.scope) || previous.scope.starts_with(&scope))
            {
                if previous_key == key {
                    return Err(AdmissionError::RecoveryRequired);
                }
                return Err(
                    if transaction
                        .try_acquire_liveness_lock(&format!(
                            "execution-locks-v1/{previous_key}.lock"
                        ))?
                        .is_none()
                    {
                        AdmissionError::Busy
                    } else {
                        AdmissionError::RecoveryRequired
                    },
                );
            }
        }
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|error| anyhow!("execution identity failed: {error}"))?;
        let record = ExecutionRecord {
            schema: 1,
            scope,
            operation_id: random.iter().map(|byte| format!("{byte:02x}")).collect(),
            kind: kind.to_owned(),
            active: true,
        };
        transaction.write_json_atomic(&record_path, &record)?;
        drop(transaction);
        Ok(Self {
            store,
            record_path,
            record,
            _lock: lock,
            writer_started: false,
            cleanup_uncertain: false,
            request_owner: None,
        })
    }

    pub(crate) fn mark_writer_started(&mut self) {
        self.writer_started = true;
    }

    pub(crate) fn require_recovery(&mut self) {
        self.cleanup_uncertain = true;
    }

    pub(crate) fn kind(&self) -> &str {
        &self.record.kind
    }

    pub(crate) fn belongs_to(&self, owner: &Arc<()>) -> bool {
        self.request_owner
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, owner))
    }

    /// Call only after supported writers have stopped and final publication ended.
    pub(crate) fn finish(&mut self) -> Result<()> {
        anyhow::ensure!(
            !self.cleanup_uncertain,
            "execution cleanup was not established; recovery is required"
        );
        let transaction = self.store.transaction()?;
        let current: ExecutionRecord = transaction.read_json(&self.record_path)?;
        anyhow::ensure!(
            current.schema == 1
                && current.operation_id == self.record.operation_id
                && current.scope == self.record.scope,
            "execution record changed while owned"
        );
        self.record.active = false;
        transaction.write_json_atomic(&self.record_path, &self.record)?;
        Ok(())
    }
}

impl Drop for ExecutionLease {
    fn drop(&mut self) {
        // Before spawn there is a local proof that this operation created no
        // writer. After spawn, failure/drop must retain the durable quarantine.
        if !self.writer_started && self.record.active {
            let _ = self.finish();
        }
    }
}

fn worktree_root(directory: &Path) -> Result<Option<PathBuf>> {
    for ancestor in directory.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(metadata) if metadata.is_file() || metadata.is_dir() => {
                return Ok(Some(ancestor.to_owned()))
            }
            Ok(_) => return Err(anyhow!("unsupported Git worktree metadata")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

pub(crate) fn cleanup_dropped_writer(
    containment: Arc<crate::process::ProcessContainment>,
    mut lease: ExecutionLease,
) {
    let _ = containment.request_termination();
    if let Ok(executor) = tokio::runtime::Handle::try_current() {
        executor.spawn(async move {
            if crate::process::wait_for_cleanup(&containment).await.is_ok() {
                let _ = lease.finish();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "meridian-execution-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join("state")).unwrap();
            std::fs::create_dir_all(root.join("first/.git")).unwrap();
            std::fs::create_dir_all(root.join("first/nested")).unwrap();
            std::fs::create_dir_all(root.join("second")).unwrap();
            std::fs::create_dir_all(root.join("plain/artifacts")).unwrap();
            std::fs::create_dir_all(root.join("plain/working")).unwrap();
            std::fs::write(
                root.join("second/.git"),
                "gitdir: ../first/.git/worktrees/second\n",
            )
            .unwrap();
            Self(root)
        }
        fn policy(&self, relative: &str) -> PathPolicy {
            PathPolicy::new(vec![self.0.join(relative)], Vec::new()).unwrap()
        }
        fn store(&self, policy: &PathPolicy) -> Arc<PrivateStateStore> {
            Arc::new(
                PrivateStateStore::open(&self.0.join("state"), policy.effective_roots()).unwrap(),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn nested_roots_share_a_worktree_scope_and_linked_worktrees_remain_independent() {
        let fixture = Fixture::new();
        let first_policy = fixture.policy("first");
        let nested_policy = fixture.policy("first/nested");
        let second_policy = fixture.policy("second");
        let first = ExecutionLease::acquire(
            fixture.store(&first_policy),
            &first_policy,
            &fixture.0.join("first/world.dmb"),
            &fixture.0.join("first"),
            "compile",
        )
        .unwrap();
        assert!(matches!(
            ExecutionLease::acquire(
                fixture.store(&nested_policy),
                &nested_policy,
                &fixture.0.join("first/nested/other.dmb"),
                &fixture.0.join("first/nested"),
                "standard"
            ),
            Err(AdmissionError::Busy)
        ));
        let second = ExecutionLease::acquire(
            fixture.store(&second_policy),
            &second_policy,
            &fixture.0.join("second/world.dmb"),
            &fixture.0.join("second"),
            "compile",
        )
        .unwrap();
        drop(second);
        drop(first);
        assert!(ExecutionLease::acquire(
            fixture.store(&nested_policy),
            &nested_policy,
            &fixture.0.join("first/nested/other.dmb"),
            &fixture.0.join("first/nested"),
            "standard"
        )
        .is_ok());
    }

    #[test]
    fn an_abandoned_writer_requires_recovery_even_after_its_lock_becomes_available() {
        let fixture = Fixture::new();
        let policy = fixture.policy("first");
        let store = fixture.store(&policy);
        let acquire = || {
            ExecutionLease::acquire(
                store.clone(),
                &policy,
                &fixture.0.join("first/world.dmb"),
                &fixture.0.join("first"),
                "compile",
            )
        };
        let mut writer = acquire().unwrap();
        writer.mark_writer_started();
        drop(writer);
        assert!(matches!(acquire(), Err(AdmissionError::RecoveryRequired)));
    }

    #[test]
    fn ancestor_and_descendant_scopes_exclude_each_other_and_keep_crash_quarantine() {
        let fixture = Fixture::new();
        let policy = fixture.policy("plain");
        let store = fixture.store(&policy);
        let acquire = |directory: &str| {
            let directory = fixture.0.join(directory);
            ExecutionLease::acquire(
                store.clone(),
                &policy,
                &directory.join("world.dmb"),
                &directory,
                "compile",
            )
        };
        let parent = acquire("plain").unwrap();
        assert!(matches!(
            acquire("plain/artifacts"),
            Err(AdmissionError::Busy)
        ));
        drop(parent);
        let child = acquire("plain/artifacts").unwrap();
        assert!(matches!(acquire("plain"), Err(AdmissionError::Busy)));
        // Sibling non-Git projects do not share mutable directories.
        drop(acquire("plain/working").unwrap());
        drop(child);
        let mut parent = acquire("plain").unwrap();
        parent.mark_writer_started();
        drop(parent);
        assert!(matches!(
            acquire("plain/artifacts"),
            Err(AdmissionError::RecoveryRequired)
        ));
    }

    #[test]
    fn missing_scope_state_fails_closed_for_overlapping_admission() {
        let fixture = Fixture::new();
        let policy = fixture.policy("plain");
        let store = fixture.store(&policy);
        let parent = ExecutionLease::acquire(
            store.clone(),
            &policy,
            &fixture.0.join("plain/world.dmb"),
            &fixture.0.join("plain"),
            "compile",
        )
        .unwrap();
        std::fs::remove_file(store.root().join(&parent.record_path)).unwrap();
        assert!(matches!(
            ExecutionLease::acquire(
                store.clone(),
                &policy,
                &fixture.0.join("plain/artifacts/world.dmb"),
                &fixture.0.join("plain/artifacts"),
                "compile",
            ),
            Err(AdmissionError::RecoveryRequired)
        ));
        drop(parent);
        assert!(matches!(
            ExecutionLease::acquire(
                store,
                &policy,
                &fixture.0.join("plain/world.dmb"),
                &fixture.0.join("plain"),
                "compile",
            ),
            Err(AdmissionError::RecoveryRequired)
        ));
    }

    #[test]
    fn malformed_scope_state_fails_closed() {
        let fixture = Fixture::new();
        let policy = fixture.policy("first");
        let store = fixture.store(&policy);
        store
            .write_json_atomic("execution-v1/broken/state.json", &serde_json::json!({}))
            .unwrap();
        assert!(matches!(
            ExecutionLease::acquire(
                store,
                &policy,
                &fixture.0.join("first/world.dmb"),
                &fixture.0.join("first"),
                "compile",
            ),
            Err(AdmissionError::RecoveryRequired)
        ));
    }

    #[test]
    fn unmarked_projects_reject_sibling_and_ancestor_working_directories() {
        let fixture = Fixture::new();
        let policy = fixture.policy("plain");
        let store = fixture.store(&policy);
        for working in ["plain/working", "plain"] {
            assert!(matches!(
                ExecutionLease::acquire(
                    store.clone(),
                    &policy,
                    &fixture.0.join("plain/artifacts/world.dmb"),
                    &fixture.0.join(working),
                    "standard"
                ),
                Err(AdmissionError::Other(_))
            ));
        }
        assert!(ExecutionLease::acquire(
            store,
            &policy,
            &fixture.0.join("plain/artifacts/world.dmb"),
            &fixture.0.join("plain/artifacts"),
            "standard"
        )
        .is_ok());
    }
}
