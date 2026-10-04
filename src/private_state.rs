use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::EffectiveRoot;

const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const TEMPORARY_NAME_ATTEMPTS: usize = 32;

pub struct PrivateStateStore {
    root: PathBuf,
    operation_lock_path: PathBuf,
}

struct OperationLock {
    _file: File,
}

pub(crate) struct PrivateStateTransaction<'a> {
    store: &'a PrivateStateStore,
    _operation: OperationLock,
}

pub(crate) struct PrivateStateLivenessLock {
    _file: File,
}

impl PrivateStateStore {
    pub fn open(path: &Path, workspace_roots: &[EffectiveRoot]) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path).with_context(|| {
            format!("private state directory does not exist: {}", path.display())
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("private state path must be an existing non-symlink directory");
        }
        let root = path.canonicalize()?;
        if workspace_roots
            .iter()
            .any(|workspace| root.starts_with(&workspace.path) || workspace.path.starts_with(&root))
        {
            bail!("private state directory must be outside every workspace root");
        }

        let lock_path = root.join(".meridian-mcp.lock");
        if let Ok(metadata) = std::fs::symlink_metadata(&lock_path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                bail!("private state lock must be a regular file");
            }
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| {
                format!("could not open private state lock: {}", lock_path.display())
            })?;
        let lock_metadata = std::fs::symlink_metadata(&lock_path)?;
        if !lock_metadata.is_file() || lock_metadata.file_type().is_symlink() {
            bail!("private state lock must be a regular file");
        }
        drop(lock);
        Ok(Self {
            root,
            operation_lock_path: lock_path,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_json_atomic<T: Serialize>(&self, relative: &str, value: &T) -> Result<PathBuf> {
        let _operation = self.lock_operation()?;
        self.write_json_unlocked(relative, value)
    }

    fn write_json_unlocked<T: Serialize>(&self, relative: &str, value: &T) -> Result<PathBuf> {
        let output = self.resolve_record(relative, true)?;
        let bytes = serde_json::to_vec_pretty(value)?;
        if bytes.len() > MAX_RECORD_BYTES {
            bail!("private state record exceeds the 8 MiB limit");
        }
        let parent = output.parent().expect("validated record path has a parent");
        let (temporary, mut file) = create_private_file(parent)?;
        let result = (|| -> Result<()> {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            install_temporary(&temporary, &output)?;
            let installed: serde_json::Value = self.read_json_unlocked(relative)?;
            drop(installed);
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        Ok(output)
    }

    pub fn read_json<T: DeserializeOwned>(&self, relative: &str) -> Result<T> {
        self.transaction()?.read_json(relative)
    }

    fn read_json_unlocked<T: DeserializeOwned>(&self, relative: &str) -> Result<T> {
        let path = self.resolve_record(relative, false)?;
        self.read_json_path(&path)
    }

    fn read_json_optional_unlocked<T: DeserializeOwned>(
        &self,
        relative: &str,
    ) -> Result<Option<T>> {
        let path = self.resolve_record(relative, false)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => self.read_json_path(&path).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn read_json_path<T: DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("private state record is not a regular file");
        }
        if metadata.len() > MAX_RECORD_BYTES as u64 {
            bail!("private state record exceeds the 8 MiB limit");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(path)?
            .take((MAX_RECORD_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_RECORD_BYTES {
            bail!("private state record exceeds the 8 MiB limit");
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn list_records(&self, namespace: &str, max_entries: usize) -> Result<Vec<PathBuf>> {
        let _operation = self.lock_operation()?;
        let maximum = max_entries.min(MAX_RECORDS);
        let directory = self.resolve_record(namespace, false)?;
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let mut pending = vec![directory];
        let mut records = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let metadata = entry.file_type()?;
                if metadata.is_symlink() {
                    bail!("private state namespaces cannot contain symlinks");
                }
                if metadata.is_dir() {
                    pending.push(entry.path());
                } else if metadata.is_file() {
                    records.push(entry.path());
                    if records.len() > maximum {
                        bail!("private state record enumeration exceeds its limit");
                    }
                }
            }
        }
        records.sort();
        Ok(records)
    }

    pub(crate) fn transaction(&self) -> Result<PrivateStateTransaction<'_>> {
        Ok(PrivateStateTransaction {
            store: self,
            _operation: self.lock_operation()?,
        })
    }

    pub(crate) fn acquire_liveness_lock(&self, relative: &str) -> Result<PrivateStateLivenessLock> {
        let file = self.open_liveness_file(relative)?;
        file.lock().with_context(|| {
            format!("could not acquire private state liveness lock: {relative}")
        })?;
        Ok(PrivateStateLivenessLock { _file: file })
    }

    pub(crate) fn try_acquire_liveness_lock(
        &self,
        relative: &str,
    ) -> Result<Option<PrivateStateLivenessLock>> {
        let file = self.open_liveness_file(relative)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(PrivateStateLivenessLock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error).with_context(|| {
                format!("could not acquire private state liveness lock: {relative}")
            }),
        }
    }

    fn open_liveness_file(&self, relative: &str) -> Result<File> {
        let _operation = self.lock_operation()?;
        self.open_liveness_file_unlocked(relative)
    }

    fn open_liveness_file_unlocked(&self, relative: &str) -> Result<File> {
        let path = self.resolve_record(relative, true)?;
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                bail!("private state liveness lock must be a regular file");
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| {
                format!(
                    "could not open private state liveness lock: {}",
                    path.display()
                )
            })?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("private state liveness lock must be a regular file");
        }
        Ok(file)
    }

    fn lock_operation(&self) -> Result<OperationLock> {
        let metadata = std::fs::symlink_metadata(&self.operation_lock_path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("private state lock must be a regular file");
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.operation_lock_path)
            .with_context(|| {
                format!(
                    "could not open private state lock: {}",
                    self.operation_lock_path.display()
                )
            })?;
        let metadata = std::fs::symlink_metadata(&self.operation_lock_path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("private state lock must be a regular file");
        }
        file.lock()
            .with_context(|| format!("could not lock private state: {}", self.root.display()))?;
        Ok(OperationLock { _file: file })
    }

    fn resolve_record(&self, relative: &str, create_parent: bool) -> Result<PathBuf> {
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("private state record name must be a non-empty relative path");
        }
        let path = self.root.join(relative);
        let parent = path.parent().expect("validated record path has a parent");
        let mut checked_parent = self.root.clone();
        for component in relative
            .parent()
            .expect("relative record parent")
            .components()
        {
            checked_parent.push(component);
            match std::fs::symlink_metadata(&checked_parent) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_parent => {
                    // The preceding component was contained before this write.
                    // create_dir_all would follow an unchecked namespace link.
                    match std::fs::create_dir(&checked_parent) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error.into()),
            }
            let canonical = checked_parent.canonicalize()?;
            if !canonical.starts_with(&self.root) {
                bail!("private state record escapes through a symlink or reparse point");
            }
            if !canonical.is_dir() {
                bail!("private state record parent must be a directory");
            }
        }
        if parent.exists() {
            let canonical_parent = parent.canonicalize()?;
            if !canonical_parent.starts_with(&self.root) {
                bail!("private state record escapes through a symlink or reparse point");
            }
        }
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                bail!("private state records cannot be symlinks");
            }
        }
        Ok(path)
    }
}

impl PrivateStateTransaction<'_> {
    pub(crate) fn namespace_exists(&self, relative: &str) -> Result<bool> {
        let path = self.store.resolve_record(relative, false)?;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
            Ok(_) => bail!("private state namespace must be a regular directory"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Bound directory entries as well as records while retaining the operation
    /// lock. Execution admission cannot race another scope's active publication.
    pub(crate) fn list_entries_bounded(
        &self,
        namespace: &str,
        max_entries: usize,
    ) -> Result<Vec<PathBuf>> {
        if !self.namespace_exists(namespace)? {
            return Ok(Vec::new());
        }
        let maximum = max_entries.min(MAX_RECORDS);
        let mut pending = vec![self.store.resolve_record(namespace, false)?];
        let mut records = Vec::new();
        let mut entries = 0;
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                entries += 1;
                if entries > maximum {
                    bail!("private state entry enumeration exceeds its limit");
                }
                let metadata = entry.file_type()?;
                if metadata.is_symlink() {
                    bail!("private state namespaces cannot contain symlinks");
                }
                if metadata.is_dir() {
                    pending.push(entry.path());
                    records.push(entry.path());
                } else if metadata.is_file() {
                    records.push(entry.path());
                } else {
                    bail!("private state namespace entry must be a regular file or directory");
                }
            }
        }
        records.sort();
        Ok(records)
    }

    /// Only a nonblocking lock is permitted inside an operation transaction.
    pub(crate) fn try_acquire_liveness_lock(
        &self,
        relative: &str,
    ) -> Result<Option<PrivateStateLivenessLock>> {
        let file = self.store.open_liveness_file_unlocked(relative)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(PrivateStateLivenessLock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error).with_context(|| {
                format!("could not acquire private state liveness lock: {relative}")
            }),
        }
    }

    // Read/modify/publish one record while retaining the same operation lock.
    pub(crate) fn write_json_atomic<T: Serialize>(
        &self,
        relative: &str,
        value: &T,
    ) -> Result<PathBuf> {
        self.store.write_json_unlocked(relative, value)
    }

    pub(crate) fn read_json<T: DeserializeOwned>(&self, relative: &str) -> Result<T> {
        self.store.read_json_unlocked(relative)
    }

    pub(crate) fn read_json_optional<T: DeserializeOwned>(
        &self,
        relative: &str,
    ) -> Result<Option<T>> {
        self.store.read_json_optional_unlocked(relative)
    }
}

fn create_private_file(parent: &Path) -> Result<(PathBuf, File)> {
    for _ in 0..TEMPORARY_NAME_ATTEMPTS {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| anyhow!(error.to_string()))?;
        let path = parent.join(format!(".meridian-tmp-{}", hex(&random)));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not allocate a private state temporary file")
}

fn install_temporary(temporary: &Path, output: &Path) -> Result<()> {
    // Never remove the old name before the replacement: a crash must expose
    // either complete record, not a gap that looks like an unmanaged artifact.
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = output.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(temporary, output)?;
        File::open(output.parent().expect("validated record parent"))?.sync_all()?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
