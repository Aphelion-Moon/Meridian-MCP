use crate::atomic_output::rename_without_replace;
use crate::outputs::DocsInstallation;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) struct StagingDirectory {
    path: PathBuf,
    armed: bool,
}

impl StagingDirectory {
    pub fn create(parent: &Path) -> Result<Self> {
        for _ in 0..32 {
            let path = private_path(parent, "tmp")?;
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, armed: true }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(anyhow!(
            "could not allocate a private documentation directory"
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn check_installation_support(&self) -> Result<()> {
        // Probe only owned, empty children on the destination filesystem. An
        // unsupported rename must fail before running a potentially costly helper
        // or moving any existing documentation into a backup.
        let source = self.path.join("rename-probe");
        let destination = self.path.join("rename-probe-complete");
        std::fs::create_dir(&source)?;
        if let Err(error) = rename_without_replace(&source, &destination) {
            #[cfg(target_os = "linux")]
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
            ) {
                return Err(anyhow!(
                    "documentation output filesystem does not support RENAME_NOREPLACE; \
                     choose an output on a supported Linux filesystem (for example, \
                     native WSL storage instead of a mounted Windows drive): {error}"
                ));
            }
            return Err(error.into());
        }
        std::fs::remove_dir(&destination)?;
        Ok(())
    }

    // Cancellation drops the contained process before this guard. Windows may
    // briefly retain its file handles after termination; bound that cleanup wait.
    pub fn cleanup(&mut self) -> Option<String> {
        if !self.armed {
            return None;
        }
        self.armed = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir_all(&self.path) {
                Ok(()) => return None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::DirectoryNotEmpty
                    ) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Some(error.to_string()),
            }
        }
    }

    pub fn install(&mut self, output: &Path, overwrite: bool) -> Result<DocsInstallation> {
        self.install_with(output, overwrite, rename_without_replace)
    }

    fn install_with(
        &mut self,
        output: &Path,
        overwrite: bool,
        mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<DocsInstallation> {
        let exists = validate_directory_target(output)?;
        if exists && !overwrite {
            return Err(anyhow!("output exists; set overwrite=true"));
        }
        let backup = if exists {
            // Reserve the backup container, then move into its absent child. Its
            // guard must never delete old documentation after a failed restore.
            let mut reservation = Self::create(
                output
                    .parent()
                    .ok_or_else(|| anyhow!("output has no parent"))?,
            )?;
            let backup = reservation.path.join("previous");
            rename(output, &backup)?;
            reservation.armed = false;
            Some(std::mem::take(&mut reservation.path))
        } else {
            None
        };

        if let Err(install) = rename(&self.path, output) {
            let mut report = DocsInstallation {
                cleanup_complete: true,
                code: Some("documentation_install_failed".into()),
                message: Some(install.to_string()),
                ..Default::default()
            };
            if let Some(backup) = backup {
                match rename(&backup.join("previous"), output) {
                    Ok(()) => {
                        report.previous_restored = Some(true);
                        if let Err(error) = std::fs::remove_dir(&backup) {
                            recovery(&mut report, &backup, &error.to_string());
                        }
                    }
                    Err(error) => {
                        report.previous_restored = Some(false);
                        recovery(&mut report, &backup, &error.to_string());
                    }
                }
            }
            return Ok(report);
        }
        self.armed = false;
        let mut report = DocsInstallation {
            installed: true,
            success: true,
            cleanup_complete: true,
            ..Default::default()
        };
        if let Some(backup) = backup {
            if let Err(error) = std::fs::remove_dir_all(&backup) {
                report.success = false;
                report.code = Some("documentation_cleanup_incomplete".into());
                recovery(&mut report, &backup, &error.to_string());
            }
        }
        Ok(report)
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if let Some(error) = self.cleanup() {
            tracing::warn!(path = %self.path.display(), %error, "documentation staging cleanup failed");
        }
    }
}

fn recovery(report: &mut DocsInstallation, backup: &Path, message: &str) {
    report.cleanup_complete = false;
    report.backup_directory = Some(backup.into());
    report.backup_name = backup.file_name().map(|v| v.to_string_lossy().into_owned());
    report.cleanup_error = Some(message.into());
    report.recovery=Some("Inspect the installed output and retained backup before retrying. Restore needed previous files from the backup's previous directory.".into());
}

pub(super) fn validate_directory_target(output: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(output) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(anyhow!(
            "documentation output must be a directory, not a file or symbolic link"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn private_path(parent: &Path, purpose: &str) -> Result<PathBuf> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| anyhow!(error.to_string()))?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(parent.join(format!(".meridian-mcp-dmdoc-{suffix}.{purpose}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement_fixture() -> (StagingDirectory, StagingDirectory, PathBuf) {
        let root = StagingDirectory::create(&std::env::temp_dir()).unwrap();
        let stage = StagingDirectory::create(root.path()).unwrap();
        std::fs::write(stage.path().join("index.html"), "new docs").unwrap();
        let output = root.path.join("html");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("old.html"), "old docs").unwrap();
        (root, stage, output)
    }

    #[test]
    fn restoration_collision_preserves_the_late_writer_and_reports_old_docs() {
        let (mut root, mut stage, output) = replacement_fixture();
        let mut calls = 0;
        let report = stage
            .install_with(&output, true, |source, destination| {
                calls += 1;
                if calls == 2 {
                    // A different writer creates even an empty directory after the
                    // previous output was moved. Restoration must not overwrite it.
                    std::fs::create_dir(destination)?;
                    return Err(std::io::Error::other("injected install failure"));
                }
                rename_without_replace(source, destination)
            })
            .unwrap();
        let report = serde_json::to_value(report).unwrap();
        assert_eq!(report["installed"], false);
        assert_eq!(report["previous_restored"], false);
        assert_eq!(report["cleanup_complete"], false);
        assert!(output.is_dir());
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 0);
        let backup = PathBuf::from(report["backup_directory"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(backup.join("previous/old.html")).unwrap(),
            "old docs"
        );
        assert!(stage.cleanup().is_none());
        assert!(backup.join("previous/old.html").is_file());
        assert!(root.cleanup().is_none());
    }

    #[cfg(windows)]
    fn lock_file(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn locked_staging_restores_the_previous_directory_after_install_failure() {
        let (mut root, mut stage, output) = replacement_fixture();
        let held = lock_file(&stage.path.join("index.html"));
        let report = stage.install(&output, true).unwrap();
        let report = serde_json::to_value(report).unwrap();
        assert_eq!(report["installed"], false);
        assert_eq!(report["previous_restored"], true);
        assert_eq!(
            std::fs::read_to_string(output.join("old.html")).unwrap(),
            "old docs"
        );
        drop(held);
        assert!(stage.cleanup().is_none());
        assert!(root.cleanup().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn backup_cleanup_failure_reports_that_new_docs_are_already_installed() {
        let (mut root, mut stage, output) = replacement_fixture();
        let mut held = None;
        let mut calls = 0;
        let report = stage
            .install_with(&output, true, |source, destination| {
                calls += 1;
                rename_without_replace(source, destination)?;
                if calls == 1 {
                    held = Some(lock_file(&destination.join("old.html")));
                }
                Ok(())
            })
            .unwrap();
        let report = serde_json::to_value(report).unwrap();
        assert_eq!(report["installed"], true);
        assert_eq!(report["success"], false);
        assert_eq!(report["code"], "documentation_cleanup_incomplete");
        assert_eq!(report["cleanup_complete"], false);
        assert_eq!(
            std::fs::read_to_string(output.join("index.html")).unwrap(),
            "new docs"
        );
        let backup = PathBuf::from(report["backup_directory"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(backup.join("previous/old.html")).unwrap(),
            "old docs"
        );
        drop(held);
        assert!(stage.cleanup().is_none());
        assert!(root.cleanup().is_none());
    }

    #[test]
    fn installation_never_replaces_an_existing_empty_directory() {
        let mut root = StagingDirectory::create(&std::env::temp_dir()).unwrap();
        let source = root.path.join("source");
        let destination = root.path.join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(source.join("index.html"), "new").unwrap();
        assert!(rename_without_replace(&source, &destination).is_err());
        assert!(source.join("index.html").is_file());
        assert!(!destination.join("index.html").exists());
        assert!(root.cleanup().is_none());
    }
}
