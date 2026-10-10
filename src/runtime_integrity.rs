use crate::private_state::PrivateStateLivenessLock;
use crate::state::{nearest_output_before, OutputLog, RuntimeOutputEntry};
use crate::workspace_integrity::{
    compare_snapshots, FileIdentity, MutationKind, WorkspaceSnapshot,
};
use crate::{LaunchProvenance, PrivateStateStore};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

const MAX_RUNTIME_EVENTS: usize = 10_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIntegrityStatus {
    Active,
    FinalizedClean,
    FinalizedWithChanges,
    FinalizedWithViolation,
    ObservedDuringRecovery,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RuntimeIntegrityEvent {
    #[serde(default = "source_integrity_warning_code")]
    pub code: String,
    pub relative_path: String,
    pub change_kind: MutationKind,
    pub before: Option<FileIdentity>,
    pub after: Option<FileIdentity>,
    pub first_observed_offset_ms: u64,
    pub nearest_output: Option<RuntimeOutputEntry>,
    pub session_id: String,
    pub process_id: Option<u32>,
    pub build_record_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RuntimeIntegrityJournal {
    pub schema: u32,
    pub session_id: String,
    pub status: RuntimeIntegrityStatus,
    pub protected_root: PathBuf,
    pub baseline: WorkspaceSnapshot,
    pub events: Vec<RuntimeIntegrityEvent>,
    pub last_action: String,
    pub process_id: Option<u32>,
    pub launch_provenance: LaunchProvenance,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RuntimeIntegritySummary {
    pub session_id: String,
    pub status: RuntimeIntegrityStatus,
    pub last_action: String,
    pub event_count: usize,
    pub warnings: Vec<RuntimeIntegrityEvent>,
    pub violations: Vec<RuntimeIntegrityEvent>,
}

pub struct RuntimeIntegritySession {
    store: Arc<PrivateStateStore>,
    record_name: String,
    _liveness: PrivateStateLivenessLock,
    output_log: OutputLog,
    owned_paths: Vec<PathBuf>,
    started_at: Instant,
    document: RuntimeIntegrityJournal,
    observed: BTreeSet<(String, MutationKind)>,
}

impl RuntimeIntegritySession {
    pub fn create(
        store: Arc<PrivateStateStore>,
        protected_root: &Path,
        launch_provenance: LaunchProvenance,
        output_log: OutputLog,
        owned_paths: Vec<PathBuf>,
    ) -> Result<Self> {
        let baseline = WorkspaceSnapshot::capture(protected_root)?;
        let session_id = random_id()?;
        let record_name = format!("runtime-integrity/{session_id}.json");
        let liveness = store.acquire_liveness_lock(&liveness_record_name(&session_id)?)?;
        let document = RuntimeIntegrityJournal {
            schema: 1,
            session_id,
            status: RuntimeIntegrityStatus::Active,
            protected_root: baseline.root.clone(),
            baseline,
            events: Vec::new(),
            last_action: "baseline_captured".to_owned(),
            process_id: None,
            launch_provenance,
        };
        store.write_json_atomic(&record_name, &document)?;
        Ok(Self {
            store,
            record_name,
            _liveness: liveness,
            output_log,
            owned_paths,
            started_at: Instant::now(),
            document,
            observed: BTreeSet::new(),
        })
    }

    pub fn set_process_id(&mut self, process_id: Option<u32>) -> Result<()> {
        self.document.process_id = process_id;
        self.persist()
    }

    pub async fn observe_now(&mut self, action: &'static str) -> Result<RuntimeIntegritySummary> {
        let root = self.document.protected_root.clone();
        let current =
            tokio::task::spawn_blocking(move || WorkspaceSnapshot::capture(&root)).await??;
        self.observe_snapshot(current, action)
    }

    fn observe_blocking(&mut self, action: &'static str) -> Result<RuntimeIntegritySummary> {
        let current = WorkspaceSnapshot::capture(&self.document.protected_root)?;
        self.observe_snapshot(current, action)
    }

    fn observe_snapshot(
        &mut self,
        current: WorkspaceSnapshot,
        action: &'static str,
    ) -> Result<RuntimeIntegritySummary> {
        let delta = compare_snapshots(&self.document.baseline, &current, &self.owned_paths)?;
        let offset_ms = self.started_at.elapsed().as_millis() as u64;
        let nearest_output = nearest_output_before(&self.output_log, offset_ms);
        for mutation in delta
            .added
            .into_iter()
            .chain(delta.modified)
            .chain(delta.deleted)
        {
            let key = (mutation.relative_path.clone(), mutation.change_kind);
            if self.observed.insert(key) {
                if self.document.events.len() >= MAX_RUNTIME_EVENTS {
                    return Err(anyhow!("runtime integrity event limit exceeded"));
                }
                self.document.events.push(RuntimeIntegrityEvent {
                    code: source_integrity_warning_code(),
                    relative_path: mutation.relative_path,
                    change_kind: mutation.change_kind,
                    before: mutation.before,
                    after: mutation.after,
                    first_observed_offset_ms: offset_ms,
                    nearest_output: nearest_output.clone(),
                    session_id: self.document.session_id.clone(),
                    process_id: self.document.process_id,
                    build_record_id: self.document.launch_provenance.build_record_id.clone(),
                });
            }
        }
        self.document.last_action = action.to_owned();
        self.persist()?;
        Ok(self.summary())
    }

    pub async fn finalize(&mut self, action: &'static str) -> Result<RuntimeIntegritySummary> {
        self.observe_now(action).await?;
        self.finish_observation()
    }

    fn finish_observation(&mut self) -> Result<RuntimeIntegritySummary> {
        self.document.status = if self
            .document
            .events
            .iter()
            .any(|event| event.change_kind == MutationKind::Deleted)
        {
            RuntimeIntegrityStatus::FinalizedWithViolation
        } else if self.document.events.is_empty() {
            RuntimeIntegrityStatus::FinalizedClean
        } else {
            RuntimeIntegrityStatus::FinalizedWithChanges
        };
        self.persist()?;
        Ok(self.summary())
    }

    pub fn summary(&self) -> RuntimeIntegritySummary {
        let (violations, warnings): (Vec<_>, Vec<_>) = self
            .document
            .events
            .iter()
            .cloned()
            .partition(|event| event.change_kind == MutationKind::Deleted);
        RuntimeIntegritySummary {
            session_id: self.document.session_id.clone(),
            status: self.document.status,
            last_action: self.document.last_action.clone(),
            event_count: self.document.events.len(),
            warnings,
            violations,
        }
    }

    fn persist(&self) -> Result<()> {
        self.store
            .write_json_atomic(&self.record_name, &self.document)?;
        Ok(())
    }
}

fn source_integrity_warning_code() -> String {
    "source_integrity_warning".to_owned()
}

pub(crate) async fn observe_owned(
    session: Arc<Mutex<RuntimeIntegritySession>>,
    pool: crate::request::BlockingPool,
    action: &'static str,
    finalize: bool,
) -> Result<RuntimeIntegritySummary> {
    pool.run(move || {
        let mut session = session.blocking_lock();
        let summary = session.observe_blocking(action)?;
        if finalize {
            session.finish_observation()
        } else {
            Ok(summary)
        }
    })
    .await
}

pub(crate) fn spawn_monitor(
    session: Arc<Mutex<RuntimeIntegritySession>>,
    mut stop: watch::Receiver<bool>,
    pool: crate::request::BlockingPool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => { let _ = observe_owned(session.clone(), pool.clone(), "monitor", false).await; }
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() { break; }
                }
            }
        }
    })
}

pub fn recover_unfinished(
    store: &PrivateStateStore,
    effective_roots: &[crate::EffectiveRoot],
) -> Result<Vec<RuntimeIntegritySummary>> {
    let mut summaries = Vec::new();
    for path in store.list_records("runtime-integrity", 10_000)? {
        let relative = path
            .strip_prefix(store.root())?
            .to_string_lossy()
            .replace('\\', "/");
        let mut document: RuntimeIntegrityJournal = store.read_json(&relative)?;
        if document.status != RuntimeIntegrityStatus::Active {
            continue;
        }
        let Some(_liveness) =
            store.try_acquire_liveness_lock(&liveness_record_name(&document.session_id)?)?
        else {
            continue;
        };
        document = store.read_json(&relative)?;
        if document.status != RuntimeIntegrityStatus::Active {
            continue;
        }
        if !effective_roots
            .iter()
            .any(|root| document.protected_root.starts_with(&root.path))
        {
            summaries.push(summary_from_document(&document));
            continue;
        }
        document.status = RuntimeIntegrityStatus::ObservedDuringRecovery;
        document.last_action = "observed_during_recovery".to_owned();
        store.write_json_atomic(&relative, &document)?;
        summaries.push(summary_from_document(&document));
    }
    Ok(summaries)
}

fn liveness_record_name(session_id: &str) -> Result<String> {
    if session_id.len() != 32
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("runtime integrity session ID is invalid");
    }
    Ok(format!("runtime-integrity-locks/{session_id}.lock"))
}

fn summary_from_document(document: &RuntimeIntegrityJournal) -> RuntimeIntegritySummary {
    let (violations, warnings): (Vec<_>, Vec<_>) = document
        .events
        .iter()
        .cloned()
        .partition(|event| event.change_kind == MutationKind::Deleted);
    RuntimeIntegritySummary {
        session_id: document.session_id.clone(),
        status: document.status,
        last_action: document.last_action.clone(),
        event_count: document.events.len(),
        warnings,
        violations,
    }
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
