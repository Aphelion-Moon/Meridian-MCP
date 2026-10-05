use crate::atomic_output::{write_atomic, AtomicOutputError};
use crate::PathPolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_INTEGRITY_ENTRIES: usize = 250_000;
const MAX_INTEGRITY_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct FileIdentity {
    pub tracked: bool,
    pub git_object_kind: Option<String>,
    pub git_object_id: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct WorkspaceSnapshot {
    pub root: PathBuf,
    pub records: BTreeMap<String, FileIdentity>,
    pub digest: String,
    pub preexisting_changes: Vec<String>,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PathMutation {
    pub relative_path: String,
    pub change_kind: MutationKind,
    pub before: Option<FileIdentity>,
    pub after: Option<FileIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct IntegrityDelta {
    pub added: Vec<PathMutation>,
    pub modified: Vec<PathMutation>,
    pub deleted: Vec<PathMutation>,
    pub owned_paths: Vec<String>,
    pub current_digest: String,
}

#[derive(Clone, Debug)]
pub struct IntegrityBaseline {
    snapshot: WorkspaceSnapshot,
    pub digest: String,
    pub preexisting_changes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrityCheckpoint {
    pub action: String,
    pub baseline_digest: String,
    pub current_digest: String,
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub owned_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityJournalStatus {
    Active,
    Finalized,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct IntegrityJournalDocument {
    schema: u32,
    journal_id: String,
    status: IntegrityJournalStatus,
    baseline_digest: String,
    preexisting_change_count: usize,
    last_action: String,
    checkpoints: Vec<IntegrityCheckpoint>,
}

#[derive(Clone, Debug)]
pub struct IntegrityJournal {
    path: PathBuf,
    document: IntegrityJournalDocument,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct IntegrityJournalSummary {
    pub journal_id: String,
    pub status: IntegrityJournalStatus,
    pub last_action: String,
    pub checkpoint_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum IntegrityError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("integrity scope exceeds the fixed entry or byte limit")]
    ScopeTooLarge,
    #[error("workspace integrity violation: {0:?}")]
    Violation(Vec<String>),
    #[error("an unfinished Tracy integrity journal requires recovery after {last_action}")]
    RecoveryRequired { last_action: String },
    #[error("owned integrity exemptions must name exact files: {0}")]
    InvalidOwnedPath(String),
    #[error(transparent)]
    Atomic(#[from] AtomicOutputError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

impl WorkspaceSnapshot {
    pub fn capture(root: &Path) -> Result<Self, IntegrityError> {
        let root = root.canonicalize()?;
        let (records, preexisting_changes) = capture_records(&root)?;
        let digest = digest_records(&records);
        Ok(Self {
            root,
            records,
            digest,
            preexisting_changes,
        })
    }
}

pub fn compare_snapshots(
    baseline: &WorkspaceSnapshot,
    current: &WorkspaceSnapshot,
    owned_paths: &[PathBuf],
) -> Result<IntegrityDelta, IntegrityError> {
    if baseline.root != current.root {
        return Err(IntegrityError::InvalidOwnedPath(
            "snapshot roots differ".to_owned(),
        ));
    }
    let mut owned = BTreeSet::new();
    for path in owned_paths {
        if path.is_dir() {
            return Err(IntegrityError::InvalidOwnedPath(path.display().to_string()));
        }
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            baseline.root.join(path)
        };
        let absolute = absolute.canonicalize().unwrap_or(absolute);
        let relative = match absolute.strip_prefix(&baseline.root) {
            Ok(relative) => relative,
            Err(_) if path.is_absolute() => continue,
            Err(_) => {
                return Err(IntegrityError::InvalidOwnedPath(path.display().to_string()));
            }
        };
        owned.insert(normalize_relative(relative));
    }
    let mut delta = IntegrityDelta {
        added: Vec::new(),
        modified: Vec::new(),
        deleted: Vec::new(),
        owned_paths: owned.iter().cloned().collect(),
        current_digest: current.digest.clone(),
    };
    for (path, after) in &current.records {
        if owned.contains(path) {
            continue;
        }
        match baseline.records.get(path) {
            None => delta.added.push(PathMutation {
                relative_path: path.clone(),
                change_kind: MutationKind::Added,
                before: None,
                after: Some(after.clone()),
            }),
            Some(before) if before != after => delta.modified.push(PathMutation {
                relative_path: path.clone(),
                change_kind: MutationKind::Modified,
                before: Some(before.clone()),
                after: Some(after.clone()),
            }),
            _ => {}
        }
    }
    for (path, before) in &baseline.records {
        if !owned.contains(path) && !current.records.contains_key(path) {
            delta.deleted.push(PathMutation {
                relative_path: path.clone(),
                change_kind: MutationKind::Deleted,
                before: Some(before.clone()),
                after: None,
            });
        }
    }
    Ok(delta)
}

impl IntegrityBaseline {
    pub fn capture(root: &Path) -> Result<Self, IntegrityError> {
        let snapshot = WorkspaceSnapshot::capture(root)?;
        Ok(Self {
            digest: snapshot.digest.clone(),
            preexisting_changes: snapshot.preexisting_changes.clone(),
            snapshot,
        })
    }
    pub fn checkpoint(
        &self,
        action: impl Into<String>,
        owned_paths: &[PathBuf],
    ) -> Result<IntegrityCheckpoint, IntegrityError> {
        let current = WorkspaceSnapshot::capture(&self.snapshot.root)?;
        let delta = compare_snapshots(&self.snapshot, &current, owned_paths)?;
        let checkpoint = IntegrityCheckpoint {
            action: action.into(),
            baseline_digest: self.digest.clone(),
            current_digest: delta.current_digest,
            added: delta
                .added
                .iter()
                .map(|item| item.relative_path.clone())
                .collect(),
            modified: delta
                .modified
                .iter()
                .map(|item| item.relative_path.clone())
                .collect(),
            deleted: delta
                .deleted
                .iter()
                .map(|item| item.relative_path.clone())
                .collect(),
            owned_paths: delta.owned_paths,
        };
        let violations = checkpoint
            .added
            .iter()
            .chain(&checkpoint.modified)
            .chain(&checkpoint.deleted)
            .cloned()
            .collect::<Vec<_>>();
        if violations.is_empty() {
            Ok(checkpoint)
        } else {
            Err(IntegrityError::Violation(violations))
        }
    }
}

impl IntegrityJournal {
    pub fn create(
        policy: &PathPolicy,
        evidence_directory: &Path,
        baseline: &IntegrityBaseline,
    ) -> Result<Self, IntegrityError> {
        let path = evidence_directory.join(".meridian-tracy-session.json");
        let overwrite = if path.is_file() {
            let existing: IntegrityJournalDocument =
                serde_json::from_slice(&std::fs::read(&path)?)?;
            if existing.status != IntegrityJournalStatus::Finalized {
                return Err(IntegrityError::RecoveryRequired {
                    last_action: existing.last_action,
                });
            }
            true
        } else {
            false
        };
        let document = IntegrityJournalDocument {
            schema: 1,
            journal_id: random_id()?,
            status: IntegrityJournalStatus::Active,
            baseline_digest: baseline.digest.clone(),
            preexisting_change_count: baseline.preexisting_changes.len(),
            last_action: "baseline_captured".to_owned(),
            checkpoints: Vec::new(),
        };
        let path = persist_journal(policy, &path, overwrite, &document)?;
        Ok(Self { path, document })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn status(&self) -> IntegrityJournalStatus {
        self.document.status
    }
    pub fn summary(&self) -> IntegrityJournalSummary {
        IntegrityJournalSummary {
            journal_id: self.document.journal_id.clone(),
            status: self.document.status,
            last_action: self.document.last_action.clone(),
            checkpoint_count: self.document.checkpoints.len(),
        }
    }
    pub fn record(
        &mut self,
        policy: &PathPolicy,
        checkpoint: IntegrityCheckpoint,
    ) -> Result<(), IntegrityError> {
        self.document.last_action = checkpoint.action.clone();
        self.document.checkpoints.push(checkpoint);
        self.path = persist_journal(policy, &self.path, true, &self.document)?;
        Ok(())
    }
    pub fn finalize(&mut self, policy: &PathPolicy) -> Result<(), IntegrityError> {
        self.document.status = IntegrityJournalStatus::Finalized;
        self.document.last_action = "finalized".to_owned();
        self.path = persist_journal(policy, &self.path, true, &self.document)?;
        Ok(())
    }
}

fn persist_journal(
    policy: &PathPolicy,
    path: &Path,
    overwrite: bool,
    document: &IntegrityJournalDocument,
) -> Result<PathBuf, IntegrityError> {
    let bytes = serde_json::to_vec_pretty(document)?;
    let artifact = write_atomic(policy, path, overwrite, |output| {
        std::io::Write::write_all(output, &bytes)?;
        std::io::Write::write_all(output, b"\n")?;
        Ok(())
    })?;
    Ok(artifact.path)
}

fn capture_records(
    root: &Path,
) -> Result<(BTreeMap<String, FileIdentity>, Vec<String>), IntegrityError> {
    let status = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
        ])
        .output();
    if let Ok(status) = status {
        if status.status.success() {
            return capture_git_records(root, &status.stdout);
        }
    }
    let mut records = BTreeMap::new();
    let mut total_bytes = 0;
    visit_files(root, root, &mut records, &mut total_bytes)?;
    Ok((records, Vec::new()))
}

fn capture_git_records(
    root: &Path,
    status_bytes: &[u8],
) -> Result<(BTreeMap<String, FileIdentity>, Vec<String>), IntegrityError> {
    let changes = status_bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .collect::<Vec<_>>();
    let mut statuses = BTreeMap::new();
    for change in &changes {
        if let Some(path) = status_path(change) {
            statuses.insert(path.replace('\\', "/"), change.clone());
        }
    }
    let tracked = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "ls-files", "-s", "-z"])
        .output()?;
    let mut records = BTreeMap::new();
    for entry in tracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let entry = String::from_utf8_lossy(entry);
        let Some((metadata, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut fields = metadata.split_whitespace();
        let mode = fields.next().map(str::to_owned);
        let object = fields.next().map(str::to_owned);
        let relative = path.replace('\\', "/");
        records.insert(
            relative.clone(),
            capture_identity(
                &root.join(path),
                true,
                mode,
                object,
                statuses.get(&relative).cloned(),
            )?,
        );
        if records.len() > MAX_INTEGRITY_ENTRIES {
            return Err(IntegrityError::ScopeTooLarge);
        }
    }
    for change in &changes {
        if let Some(relative) = change.strip_prefix("? ") {
            let relative = relative.replace('\\', "/");
            records.insert(
                relative.clone(),
                capture_identity(
                    &root.join(&relative),
                    false,
                    None,
                    None,
                    Some(change.clone()),
                )?,
            );
        }
    }
    Ok((records, changes))
}

fn status_path(change: &str) -> Option<&str> {
    if let Some(path) = change.strip_prefix("? ") {
        return Some(path);
    }
    if change.starts_with("1 ") {
        return change.splitn(9, ' ').nth(8);
    }
    if change.starts_with("2 ") {
        return change.splitn(10, ' ').nth(9);
    }
    None
}

fn capture_identity(
    path: &Path,
    tracked: bool,
    kind: Option<String>,
    object: Option<String>,
    status: Option<String>,
) -> Result<FileIdentity, IntegrityError> {
    let (sha256, size) = if path.is_file() {
        let metadata = std::fs::metadata(path)?;
        (Some(hash_file(path)?), Some(metadata.len()))
    } else {
        (None, None)
    };
    Ok(FileIdentity {
        tracked,
        git_object_kind: kind,
        git_object_id: object,
        sha256,
        size,
        status: status.unwrap_or_else(|| "clean".to_owned()),
    })
}

fn visit_files(
    root: &Path,
    directory: &Path,
    records: &mut BTreeMap<String, FileIdentity>,
    total_bytes: &mut u64,
) -> Result<(), IntegrityError> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if entry.file_type()?.is_dir() {
            if matches!(
                name.to_str(),
                Some(".git" | "target" | ".meridian-tracy-diagnostics")
            ) {
                continue;
            }
            visit_files(root, &path, records, total_bytes)?;
        } else if entry.file_type()?.is_file() {
            let metadata = entry.metadata()?;
            *total_bytes = total_bytes.saturating_add(metadata.len());
            if records.len() >= MAX_INTEGRITY_ENTRIES || *total_bytes > MAX_INTEGRITY_BYTES {
                return Err(IntegrityError::ScopeTooLarge);
            }
            records.insert(
                normalize_relative(path.strip_prefix(root).unwrap_or(&path)),
                capture_identity(&path, false, None, None, None)?,
            );
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, IntegrityError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn digest_records(records: &BTreeMap<String, FileIdentity>) -> String {
    let mut hasher = Sha256::new();
    for (path, identity) in records {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher
            .update(serde_json::to_vec(identity).expect("file identity serialization cannot fail"));
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn random_id() -> Result<String, IntegrityError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn normalize_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
