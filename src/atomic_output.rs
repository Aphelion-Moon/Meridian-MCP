use crate::{PathPolicy, PolicyError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const RANDOM_NAME_ATTEMPTS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct OutputArtifact {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AtomicOutputError {
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("output path is not a file: {0}")]
    InvalidOutputType(PathBuf),
    #[error("could not allocate a private temporary output name")]
    TemporaryNameExhausted,
    #[error("could not acquire randomness for a temporary output name: {0}")]
    Entropy(String),
    #[error("{0}")]
    Writer(String),
    #[error("could not install replacement: {install}; restoration failure: {restore}; original retained at {backup}")]
    Replacement {
        install: String,
        restore: String,
        backup: PathBuf,
    },
    #[error("output installed at {path}; post-install finalization failed: {message}", path = .artifact.path.display())]
    Installed {
        artifact: Box<OutputArtifact>,
        cleanup_complete: bool,
        backup: Option<PathBuf>,
        message: String,
    },
}

impl AtomicOutputError {
    pub fn writer(message: impl Into<String>) -> Self {
        Self::Writer(message.into())
    }

    pub fn policy_code(&self) -> Option<&'static str> {
        match self {
            Self::Policy(error) => Some(error.code()),
            _ => None,
        }
    }
}

struct TemporaryOutput {
    path: PathBuf,
    armed: bool,
}

pub struct ReservedExternalOutput {
    output: PathBuf,
    temporary: TemporaryOutput,
    overwrite: bool,
}

impl ReservedExternalOutput {
    pub fn temporary_path(&self) -> &Path {
        &self.temporary.path
    }

    pub fn output_path(&self) -> &Path {
        &self.output
    }

    pub fn commit(self) -> Result<OutputArtifact, AtomicOutputError> {
        let metadata = std::fs::symlink_metadata(&self.temporary.path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(AtomicOutputError::InvalidOutputType(
                self.temporary.path.clone(),
            ));
        }
        OpenOptions::new()
            .write(true)
            .open(&self.temporary.path)?
            .sync_all()?;
        install_temporary(self.output, self.temporary, self.overwrite)
    }
}

impl TemporaryOutput {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn write_atomic<F>(
    policy: &PathPolicy,
    output: &Path,
    overwrite: bool,
    write: F,
) -> Result<OutputArtifact, AtomicOutputError>
where
    F: FnOnce(&mut File) -> Result<(), AtomicOutputError>,
{
    let output = policy.output_path(output, overwrite)?;
    if output.exists() && !output.is_file() {
        return Err(AtomicOutputError::InvalidOutputType(output));
    }
    let parent = output.parent().ok_or_else(|| {
        AtomicOutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path has no parent",
        ))
    })?;
    let (temporary_path, mut temporary_file) = create_private_file(parent, "tmp")?;
    let temporary = TemporaryOutput {
        path: temporary_path,
        armed: true,
    };

    write(&mut temporary_file)?;
    temporary_file.flush()?;
    temporary_file.sync_all()?;
    drop(temporary_file);

    install_temporary(output, temporary, overwrite)
}

pub fn promote_external_atomic<F>(
    policy: &PathPolicy,
    output: &Path,
    overwrite: bool,
    produce: F,
) -> Result<OutputArtifact, AtomicOutputError>
where
    F: FnOnce(&Path) -> Result<(), AtomicOutputError>,
{
    let output = policy.output_path(output, overwrite)?;
    if output.exists() && !output.is_file() {
        return Err(AtomicOutputError::InvalidOutputType(output));
    }
    let parent = output.parent().ok_or_else(|| {
        AtomicOutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path has no parent",
        ))
    })?;
    let (temporary_path, temporary_file) = create_private_file(parent, "external")?;
    drop(temporary_file);
    let temporary = TemporaryOutput {
        path: temporary_path,
        armed: true,
    };

    produce(&temporary.path)?;
    let metadata = std::fs::symlink_metadata(&temporary.path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(AtomicOutputError::InvalidOutputType(temporary.path.clone()));
    }
    OpenOptions::new()
        .write(true)
        .open(&temporary.path)?
        .sync_all()?;

    install_temporary(output, temporary, overwrite)
}

pub fn reserve_external_atomic(
    policy: &PathPolicy,
    output: &Path,
    overwrite: bool,
) -> Result<ReservedExternalOutput, AtomicOutputError> {
    let output = policy.output_path(output, overwrite)?;
    if output.exists() && !output.is_file() {
        return Err(AtomicOutputError::InvalidOutputType(output));
    }
    let parent = output.parent().ok_or_else(|| {
        AtomicOutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path has no parent",
        ))
    })?;
    let (temporary_path, temporary_file) = create_private_file(parent, "external")?;
    drop(temporary_file);
    Ok(ReservedExternalOutput {
        output,
        overwrite,
        temporary: TemporaryOutput {
            path: temporary_path,
            armed: true,
        },
    })
}

fn install_temporary(
    output: PathBuf,
    temporary: TemporaryOutput,
    overwrite: bool,
) -> Result<OutputArtifact, AtomicOutputError> {
    install_with(output, temporary, overwrite, rename_without_replace)
}

fn install_with(
    output: PathBuf,
    mut temporary: TemporaryOutput,
    overwrite: bool,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<OutputArtifact, AtomicOutputError> {
    let parent = output.parent().ok_or_else(|| {
        AtomicOutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path has no parent",
        ))
    })?;

    let bytes = std::fs::metadata(&temporary.path)?.len();
    let sha256 = hash_file(&temporary.path)?;
    let exists = match std::fs::symlink_metadata(&output) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => true,
        Ok(_) => return Err(AtomicOutputError::InvalidOutputType(output)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if exists && !overwrite {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "output appeared during generation; overwrite was not authorized",
        )
        .into());
    }
    let backup = if exists {
        let backup = private_available_path(parent, "backup")?;
        rename(&output, &backup)?;
        Some(backup)
    } else {
        None
    };

    if let Err(install_error) = rename(&temporary.path, &output) {
        let restore_error = backup
            .as_ref()
            .and_then(|backup| rename(backup, &output).err());
        return match restore_error {
            Some(restore_error) => Err(AtomicOutputError::Replacement {
                install: install_error.to_string(),
                restore: restore_error.to_string(),
                backup: backup.expect("restoration requires a backup"),
            }),
            None => Err(AtomicOutputError::Io(install_error)),
        };
    }
    temporary.disarm();

    // Promotion is the mutation boundary. Later cleanup/identity failures must
    // retain the installed artifact rather than imply that nothing was written.
    let mut artifact = OutputArtifact {
        path: output,
        bytes,
        sha256,
    };
    if let Some(backup) = &backup {
        if let Err(error) = std::fs::remove_file(backup) {
            return Err(AtomicOutputError::Installed {
                artifact: Box::new(artifact),
                cleanup_complete: false,
                backup: Some(backup.clone()),
                message: error.to_string(),
            });
        }
    }
    match artifact.path.canonicalize() {
        Ok(path) => {
            artifact.path = path;
            Ok(artifact)
        }
        Err(error) => Err(AtomicOutputError::Installed {
            artifact: Box::new(artifact),
            cleanup_complete: true,
            backup: None,
            message: error.to_string(),
        }),
    }
}

// Promotion and restoration must preserve a destination created after preflight.
// Plain rename may silently replace such a file (or an empty directory).
pub(crate) fn rename_without_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic output installation is unavailable on this platform",
        ))
    }
}

fn create_private_file(parent: &Path, purpose: &str) -> Result<(PathBuf, File), AtomicOutputError> {
    for _ in 0..RANDOM_NAME_ATTEMPTS {
        let path = private_path(parent, purpose)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AtomicOutputError::TemporaryNameExhausted)
}

fn private_available_path(parent: &Path, purpose: &str) -> Result<PathBuf, AtomicOutputError> {
    for _ in 0..RANDOM_NAME_ATTEMPTS {
        let path = private_path(parent, purpose)?;
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(AtomicOutputError::TemporaryNameExhausted)
}

fn private_path(parent: &Path, purpose: &str) -> Result<PathBuf, AtomicOutputError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|error| AtomicOutputError::Entropy(error.to_string()))?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(parent.join(format!(".meridian-mcp-{suffix}.{purpose}")))
}

fn hash_file(path: &Path) -> Result<String, AtomicOutputError> {
    let mut file = File::open(path)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_promotion_cleanup_failure_retains_installed_identity() {
        let root = private_path(&std::env::temp_dir(), "installed-test").unwrap();
        std::fs::create_dir(&root).unwrap();
        let output = root.join("result");
        std::fs::write(&output, "original").unwrap();
        let (path, mut file) = create_private_file(&root, "tmp").unwrap();
        file.write_all(b"replacement").unwrap();
        drop(file);
        let mut backup = None;
        let mut calls = 0;
        let result = install_with(
            output.clone(),
            TemporaryOutput { path, armed: true },
            true,
            |source, target| {
                calls += 1;
                rename_without_replace(source, target)?;
                if calls == 1 {
                    backup = Some(target.to_owned());
                }
                if calls == 2 {
                    let backup = backup.as_ref().unwrap();
                    std::fs::remove_file(backup)?;
                    std::fs::create_dir(backup)?;
                }
                Ok(())
            },
        );
        let AtomicOutputError::Installed {
            artifact,
            cleanup_complete,
            backup,
            ..
        } = result.unwrap_err()
        else {
            panic!("expected installed outcome")
        };
        assert_eq!(artifact.path, output);
        assert_eq!(artifact.bytes, 11);
        assert_eq!(artifact.sha256, hash_file(&output).unwrap());
        assert!(!cleanup_complete);
        assert!(backup.unwrap().is_dir());
        assert_eq!(std::fs::read_to_string(output).unwrap(), "replacement");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_collision_preserves_both_late_output_and_original_backup() {
        let root = private_path(&std::env::temp_dir(), "restore-test").unwrap();
        std::fs::create_dir(&root).unwrap();
        let output = root.join("result");
        std::fs::write(&output, "original").unwrap();
        let (path, mut file) = create_private_file(&root, "tmp").unwrap();
        file.write_all(b"replacement").unwrap();
        drop(file);
        let temporary = TemporaryOutput { path, armed: true };
        let mut calls = 0;
        let result = install_with(output.clone(), temporary, true, |source, target| {
            calls += 1;
            if calls == 2 {
                std::fs::write(target, "late output")?;
            }
            rename_without_replace(source, target)
        });
        let backup = match result.unwrap_err() {
            AtomicOutputError::Replacement { backup, .. } => backup,
            error => panic!("unexpected error: {error}"),
        };
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "late output");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
