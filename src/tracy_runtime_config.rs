use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub fn sustained_scheduler_progress(before: u64, wake_observed: u64, sustained: u64) -> bool {
    wake_observed > before && sustained > wake_observed
}

pub fn wake_client_url(port: u16) -> String {
    format!("byond://127.0.0.1:{port}##guest")
}

const MAX_CONFIGURATION_ENTRIES: u64 = 4_096;
const MAX_CONFIGURATION_BYTES: u64 = 64 * 1024 * 1024;
const RESUME_AFTER_INITIALIZATIONS: &str = "RESUME_AFTER_INITIALIZATIONS";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RuntimeConfigurationIdentity {
    pub schema: u32,
    pub directory_sha256: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub resume_after_initializations: bool,
}

#[derive(Clone, Debug)]
pub struct RuntimeConfiguration {
    pub directory: PathBuf,
    pub identity: RuntimeConfigurationIdentity,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeConfigurationError {
    #[error("Tracy config_directory must identify a directory")]
    NotDirectory,
    #[error("Tracy config_directory must contain config.txt")]
    MissingConfig,
    #[error("Tracy config_directory must explicitly enable RESUME_AFTER_INITIALIZATIONS in config.txt or dev_overrides.txt so a post-initialization wake can disable offline sleep")]
    ResumeAfterInitializationsNotEnabled,
    #[error("Tracy config_directory exceeds the fixed 4096-file or 64-MiB limit")]
    ScopeTooLarge,
    #[error("Tracy config_directory cannot contain symbolic links or reparse points")]
    LinkNotAllowed,
    #[error("Tracy config_directory cannot be represented safely as a BYOND world parameter")]
    UnsafeParameter,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl RuntimeConfiguration {
    pub fn world_parameter(&self) -> String {
        format!(
            "config-directory={}",
            encode_form_component(&self.directory.to_string_lossy())
        )
    }
}

pub fn inspect_runtime_configuration(
    directory: &Path,
) -> Result<RuntimeConfiguration, RuntimeConfigurationError> {
    if !directory.is_dir() {
        return Err(RuntimeConfigurationError::NotDirectory);
    }
    let directory = directory.canonicalize()?;
    let config_path = directory.join("config.txt");
    if !config_path.is_file() {
        return Err(RuntimeConfigurationError::MissingConfig);
    }
    let keep_awake = [config_path, directory.join("dev_overrides.txt")]
        .into_iter()
        .filter(|path| path.is_file())
        .try_fold(false, |enabled, path| {
            Ok::<_, std::io::Error>(enabled || file_enables_keep_awake(&path)?)
        })?;
    if !keep_awake {
        return Err(RuntimeConfigurationError::ResumeAfterInitializationsNotEnabled);
    }

    let mut files = Vec::new();
    collect_files(&directory, &directory, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    let mut total_bytes = 0_u64;
    for (relative, path) in &files {
        let metadata = std::fs::metadata(path)?;
        total_bytes = total_bytes.saturating_add(metadata.len());
        if files.len() as u64 > MAX_CONFIGURATION_ENTRIES || total_bytes > MAX_CONFIGURATION_BYTES {
            return Err(RuntimeConfigurationError::ScopeTooLarge);
        }
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(metadata.len().to_le_bytes());
        let mut input = std::fs::File::open(path)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        hasher.update([0]);
    }

    Ok(RuntimeConfiguration {
        directory,
        identity: RuntimeConfigurationIdentity {
            schema: 1,
            directory_sha256: format!("{:x}", hasher.finalize()),
            file_count: files.len() as u64,
            total_bytes,
            resume_after_initializations: true,
        },
    })
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), RuntimeConfigurationError> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(RuntimeConfigurationError::LinkNotAllowed);
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| RuntimeConfigurationError::UnsafeParameter)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
            if files.len() as u64 > MAX_CONFIGURATION_ENTRIES {
                return Err(RuntimeConfigurationError::ScopeTooLarge);
            }
        }
    }
    Ok(())
}

fn file_enables_keep_awake(path: &Path) -> Result<bool, std::io::Error> {
    let contents = std::fs::read_to_string(path)?;
    Ok(contents.lines().any(|line| {
        let line = line.trim();
        !line.starts_with('#')
            && line
                .split_whitespace()
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(RESUME_AFTER_INITIALIZATIONS))
    }))
}

fn encode_form_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            b' ' => encoded.push('+'),
            value => {
                encoded.push('%');
                encoded.push_str(&format!("{value:02X}"));
            }
        }
    }
    encoded
}
