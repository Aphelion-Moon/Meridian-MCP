//! Canonical location identity shared by provenance and execution coordination.
use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) fn canonical_location(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("artifact path has no parent"))?
        .canonicalize()?;
    Ok(parent.join(
        path.file_name()
            .ok_or_else(|| anyhow!("artifact path has no file name"))?,
    ))
}

pub(crate) fn location_key(path: &Path) -> Result<String> {
    let path = canonical_location(path)?;
    check_supported_location(&path)?;
    #[cfg(windows)]
    {
        // Canonicalization resolves existing ancestor spelling, including aliases.
        // Preserve it: Windows may distinguish case in an ancestor. Only the
        // leaf lives in the case-insensitive directory qualified below.
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("managed artifact name must be Unicode"))?;
        if !name.is_ascii() {
            bail!("non-ASCII managed artifact names require a qualified Windows case policy");
        }
        Ok(hash_location(
            &path.with_file_name(name.to_ascii_lowercase()),
            false,
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(hash_location(&path, false))
    }
}

// Used only to find records written by the old, case-folding Unix layout.
pub(crate) fn legacy_folded_location_key(path: &Path) -> Result<String> {
    Ok(hash_location(&canonical_location(path)?, true))
}

fn hash_location(path: &Path, fold_case: bool) -> String {
    let text = path.to_string_lossy();
    let identity = if fold_case {
        text.to_ascii_lowercase()
    } else {
        text.into_owned()
    };
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

#[cfg(unix)]
fn check_supported_location(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if path.exists() && path.metadata()?.is_file() && path.metadata()?.nlink() != 1 {
        bail!("managed artifacts cannot have multiple hard links");
    }
    Ok(())
}

#[cfg(windows)]
fn check_supported_location(path: &Path) -> Result<()> {
    use anyhow::Context;
    use std::fs::OpenOptions;
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_READ_ATTRIBUTES,
    };
    if path.is_file() {
        let file = std::fs::File::open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if info.nNumberOfLinks != 1 {
            bail!("managed artifacts cannot have multiple hard links");
        }
    }
    // The canonical ancestor spelling is preserved by the key. Only this
    // directory needs case-insensitive leaf semantics.
    if let Some(directory) = path.parent() {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory)
            .with_context(|| format!("cannot inspect artifact ancestor {}", directory.display()))?;
        let mut info: FILE_CASE_SENSITIVE_INFO = unsafe { std::mem::zeroed() };
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileCaseSensitiveInfo,
                (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
                std::mem::size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            // Filesystems without per-directory case sensitivity reject this query.
            if !matches!(error.raw_os_error(), Some(1 | 50 | 87)) {
                return Err(error).with_context(|| {
                    format!("cannot query case sensitivity of {}", directory.display())
                });
            }
        } else if info.Flags != 0 {
            bail!("managed artifacts in case-sensitive Windows directories are unsupported");
        }
    }
    Ok(())
}
