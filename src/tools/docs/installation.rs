use anyhow::{anyhow, Result};
use serde_json::{json, Value};
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

    pub fn install(&mut self, output: &Path, overwrite: bool) -> Result<Value> {
        self.install_with(output, overwrite, rename_without_replace)
    }

    fn install_with(
        &mut self,
        output: &Path,
        overwrite: bool,
        mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<Value> {
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
            let mut report = json!({"installed":false,"success":false,"code":"documentation_install_failed","message":install.to_string()});
            if let Some(backup) = backup {
                match rename(&backup.join("previous"), output) {
                    Ok(()) => {
                        report["previous_restored"] = json!(true);
                        if let Err(error) = std::fs::remove_dir(&backup) {
                            recovery(&mut report, &backup, &error.to_string());
                        }
                    }
                    Err(error) => {
                        report["previous_restored"] = json!(false);
                        recovery(&mut report, &backup, &error.to_string());
                    }
                }
            }
            return Ok(report);
        }
        self.armed = false;
        let mut report = json!({"installed":true,"success":true,"cleanup_complete":true});
        if let Some(backup) = backup {
            if let Err(error) = std::fs::remove_dir_all(&backup) {
                report["success"] = json!(false);
                report["code"] = json!("documentation_cleanup_incomplete");
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

fn recovery(report: &mut Value, backup: &Path, message: &str) {
    report["cleanup_complete"] = json!(false);
    report["backup_directory"] = json!(backup);
    report["backup_name"] = json!(backup.file_name());
    report["cleanup_error"] = json!(message);
    report["recovery"] = json!("Inspect the installed output and retained backup before retrying. Restore needed previous files from the backup's previous directory.");
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

// Never overwrite a destination created after preflight, including an empty
// directory. The same rule protects restoration from replacing a late writer.
fn rename_without_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
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
        // std::fs::rename can replace an existing empty directory on Windows.
        // Omitting MOVEFILE_REPLACE_EXISTING preserves a late destination.
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
            "atomic directory installation is unavailable on this platform",
        ))
    }
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
