use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ArtifactSnapshot {
    pub path: PathBuf,
    pub exists: bool,
    pub size: Option<u64>,
    pub modified_unix_ms: Option<u128>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct FileIdentity {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

impl FileIdentity {
    pub fn capture(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("cannot inspect file identity: {}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(anyhow!(
                "file identity requires a regular non-symlink file: {}",
                path.display()
            ));
        }
        let path = path.canonicalize()?;
        Ok(Self {
            path: path.clone(),
            size: metadata.len(),
            sha256: hash_file(&path)?,
        })
    }
}

impl ArtifactSnapshot {
    pub fn capture(project_root: &Path, artifact_path: &Path) -> Result<Self> {
        let project_root = project_root.canonicalize().with_context(|| {
            format!(
                "cannot canonicalize project root: {}",
                project_root.display()
            )
        })?;
        let requested = if artifact_path.is_absolute() {
            artifact_path.to_owned()
        } else {
            project_root.join(artifact_path)
        };

        if requested.exists() {
            let path = requested.canonicalize().with_context(|| {
                format!("cannot canonicalize artifact: {}", requested.display())
            })?;
            ensure_contained(&project_root, &path)?;
            let metadata = std::fs::metadata(&path)
                .with_context(|| format!("cannot inspect artifact: {}", path.display()))?;
            if !metadata.is_file() {
                return Err(anyhow!("artifact is not a file: {}", path.display()));
            }
            let modified_unix_ms = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis());
            return Ok(Self {
                path: path.clone(),
                exists: true,
                size: Some(metadata.len()),
                modified_unix_ms,
                sha256: Some(hash_file(&path)?),
            });
        }

        let path = normalize_missing_path(&requested)?;
        ensure_contained(&project_root, &path)?;
        Ok(Self {
            path,
            exists: false,
            size: None,
            modified_unix_ms: None,
            sha256: None,
        })
    }
}

fn ensure_contained(project_root: &Path, path: &Path) -> Result<()> {
    if path == project_root || path.starts_with(project_root) {
        Ok(())
    } else {
        Err(anyhow!(
            "artifact path escapes the qualified project root: {}",
            path.display()
        ))
    }
}

fn normalize_missing_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(anyhow!("artifact path contains invalid parent traversal"));
                }
            }
        }
    }
    Ok(normalized)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("cannot open artifact: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("cannot read artifact: {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
