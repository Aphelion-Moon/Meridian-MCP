use crate::{limits::ServerLimits, PathPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FIXTURE_ID_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_INPUTS: usize = 10_000;
const MAX_REQUIRED_PROCS: usize = 1_000;
const MAX_REQUIRED_TOKENS: usize = 1_000;
const MAX_TOKEN_BYTES: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureManifestDocument {
    pub schema: u32,
    pub fixture_id: String,
    pub dme_path: String,
    pub dmb_path: String,
    #[serde(default)]
    pub rsc_path: Option<String>,
    pub inputs: Vec<FixtureInputDocument>,
    #[serde(default)]
    pub required_procs: Vec<RequiredProcDocument>,
    #[serde(default)]
    pub required_tokens: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureInputDocument {
    pub path: String,
    pub role: FixtureInputRole,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureInputRole {
    Source,
    GeneratedBinding,
    NativeModule,
    ServiceExecutable,
    Configuration,
}

impl FixtureInputRole {
    pub(crate) fn is_text(self) -> bool {
        matches!(
            self,
            Self::Source | Self::GeneratedBinding | Self::Configuration
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::GeneratedBinding => "generated_binding",
            Self::NativeModule => "native_module",
            Self::ServiceExecutable => "service_executable",
            Self::Configuration => "configuration",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredProcDocument {
    pub path: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifiedFixtureInput {
    pub relative_path: String,
    pub canonical_path: PathBuf,
    pub role: FixtureInputRole,
    pub size: u64,
    pub sha256: String,
}

impl VerifiedFixtureInput {
    pub(crate) fn read_verified_bytes(
        &self,
        policy: &PathPolicy,
        limits: &ServerLimits,
    ) -> Result<Vec<u8>, FixtureManifestError> {
        check_input_size(self.role, self.size, limits)?;
        let (path, mut file) = open_regular_file(policy, &self.canonical_path, "input")?;
        let metadata = file.metadata()?;
        if path != self.canonical_path || metadata.len() != self.size {
            return Err(invalid("input changed since fixture hashing"));
        }
        let bytes = read_bounded_bytes(&mut file, self.size)?;
        if bytes.len() as u64 != self.size
            || metadata_changed(&metadata, &file.metadata()?)
            || format!("{:x}", Sha256::digest(&bytes)) != self.sha256
        {
            return Err(invalid("input changed since fixture hashing"));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifiedFixtureManifest {
    pub manifest_path: PathBuf,
    pub fixture_root: PathBuf,
    pub fixture_id: String,
    pub dme_path: PathBuf,
    pub dmb_path: PathBuf,
    pub rsc_path: Option<PathBuf>,
    pub inputs: Vec<VerifiedFixtureInput>,
    pub required_procs: Vec<RequiredProcDocument>,
    pub required_tokens: Vec<String>,
    pub identity_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum FixtureManifestError {
    #[error("fixture manifest policy rejected the path: {0}")]
    Policy(#[from] crate::PolicyError),
    #[error("fixture manifest I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("fixture manifest JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("fixture manifest is invalid: {0}")]
    Invalid(String),
}

pub struct FixtureManifest;

impl FixtureManifest {
    pub fn load(
        policy: &PathPolicy,
        path: &Path,
    ) -> Result<VerifiedFixtureManifest, FixtureManifestError> {
        Self::load_with_limits(policy, path, &ServerLimits::default())
    }

    pub fn load_with_limits(
        policy: &PathPolicy,
        path: &Path,
        limits: &ServerLimits,
    ) -> Result<VerifiedFixtureManifest, FixtureManifestError> {
        let (manifest_path, mut file) = open_regular_file(policy, path, "manifest")?;
        let metadata = file.metadata()?;
        if metadata.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("manifest exceeds the 4 MiB limit"));
        }
        // A metadata check alone cannot bound a file that grows after opening.
        let bytes = read_bounded_bytes(&mut file, MAX_MANIFEST_BYTES)?;
        if bytes.len() as u64 != metadata.len() || metadata_changed(&metadata, &file.metadata()?) {
            return Err(invalid("manifest changed while reading"));
        }
        let document: FixtureManifestDocument = serde_json::from_slice(&bytes)?;
        validate_document(&document)?;

        let fixture_root = manifest_path
            .parent()
            .expect("a canonical manifest file has a parent")
            .to_owned();
        let dme_path = resolve_existing(policy, &fixture_root, &document.dme_path)?;
        let dmb_path = resolve_output(&fixture_root, &document.dmb_path)?;
        let rsc_path = document
            .rsc_path
            .as_deref()
            .map(|path| resolve_output(&fixture_root, path))
            .transpose()?;

        let mut normalized_paths = BTreeSet::new();
        let mut canonical_paths = BTreeSet::new();
        let mut inputs = Vec::with_capacity(document.inputs.len());
        let mut total_bytes = 0_u64;
        for input in &document.inputs {
            let normalized = validate_relative_path(&input.path)?;
            if !normalized_paths.insert(path_identity(&normalized)) {
                return Err(invalid("input paths are duplicated after normalization"));
            }
            let requested = fixture_root.join(&normalized);
            let (canonical_path, file) = open_regular_file(policy, &requested, "input")?;
            let metadata = file.metadata()?;
            let canonical_key = path_identity(&canonical_path.to_string_lossy());
            if !canonical_paths.insert(canonical_key) {
                return Err(invalid(
                    "multiple inputs resolve to the same canonical file",
                ));
            }
            check_input_size(input.role, metadata.len(), limits)?;
            total_bytes = total_bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= limits.max_fixture_input_bytes)
                .ok_or_else(|| invalid("fixture inputs exceed the total byte limit"))?;
            inputs.push(VerifiedFixtureInput {
                relative_path: normalized,
                canonical_path,
                role: input.role,
                size: metadata.len(),
                sha256: hash_file(file, &metadata)?,
            });
        }
        inputs.sort_by(|left, right| {
            (left.role.as_str(), &left.relative_path)
                .cmp(&(right.role.as_str(), &right.relative_path))
        });

        let identity_sha256 = manifest_identity(&document, &inputs)?;
        Ok(VerifiedFixtureManifest {
            manifest_path,
            fixture_root,
            fixture_id: document.fixture_id,
            dme_path,
            dmb_path,
            rsc_path,
            inputs,
            required_procs: document.required_procs,
            required_tokens: document.required_tokens,
            identity_sha256,
        })
    }
}

fn validate_document(document: &FixtureManifestDocument) -> Result<(), FixtureManifestError> {
    if document.schema != 1 {
        return Err(invalid("schema must be 1"));
    }
    if document.fixture_id.is_empty() || document.fixture_id.len() > MAX_FIXTURE_ID_BYTES {
        return Err(invalid("fixture_id must contain 1-128 bytes"));
    }
    validate_relative_path(&document.dme_path)?;
    validate_relative_path(&document.dmb_path)?;
    if let Some(path) = &document.rsc_path {
        validate_relative_path(path)?;
    }
    if document.inputs.is_empty() || document.inputs.len() > MAX_INPUTS {
        return Err(invalid("inputs must contain 1-10000 entries"));
    }
    if document.required_procs.len() > MAX_REQUIRED_PROCS {
        return Err(invalid("required_procs exceeds 1000 entries"));
    }
    if document.required_tokens.len() > MAX_REQUIRED_TOKENS {
        return Err(invalid("required_tokens exceeds 1000 entries"));
    }
    for required in &document.required_procs {
        if !required.path.starts_with('/')
            || required.path.len() > MAX_PATH_BYTES
            || required.arguments.len() > 1_000
            || required.arguments.iter().any(|argument| {
                argument.is_empty()
                    || argument.len() > MAX_PATH_BYTES
                    || !argument
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
        {
            return Err(invalid("required proc contract is invalid"));
        }
    }
    if document
        .required_tokens
        .iter()
        .any(|token| token.is_empty() || token.len() > MAX_TOKEN_BYTES)
    {
        return Err(invalid("required token must contain 1-4096 bytes"));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<String, FixtureManifestError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.contains('\\')
        || path.contains("://")
        || path.contains(['*', '?', '[', ']'])
        || path.starts_with('/')
        || path.contains(':')
    {
        return Err(invalid(
            "paths must be bounded forward-slash relative paths",
        ));
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(invalid(
            "paths cannot contain empty, dot, or parent components",
        ));
    }
    Ok(components.join("/"))
}

fn resolve_existing(
    policy: &PathPolicy,
    root: &Path,
    relative: &str,
) -> Result<PathBuf, FixtureManifestError> {
    let normalized = validate_relative_path(relative)?;
    let requested = root.join(normalized);
    let metadata = std::fs::symlink_metadata(&requested)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("fixture DME must be a regular non-symlink file"));
    }
    Ok(policy.read_path(requested)?)
}

fn resolve_output(root: &Path, relative: &str) -> Result<PathBuf, FixtureManifestError> {
    let normalized = validate_relative_path(relative)?;
    Ok(root.join(normalized))
}

fn open_regular_file(
    policy: &PathPolicy,
    path: &Path,
    kind: &str,
) -> Result<(PathBuf, File), FixtureManifestError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid(format!(
            "{kind} must be a regular non-symlink file"
        )));
    }
    let canonical = policy.read_path(path)?;
    let file = File::open(&canonical)?;
    if !file.metadata()?.is_file() {
        return Err(invalid(format!("{kind} must be a regular file")));
    }
    Ok((canonical, file))
}

fn check_input_size(
    role: FixtureInputRole,
    size: u64,
    limits: &ServerLimits,
) -> Result<(), FixtureManifestError> {
    let limit = if role.is_text() {
        limits.max_fixture_text_file_bytes
    } else {
        limits.max_fixture_binary_file_bytes
    };
    if size > limit {
        return Err(invalid(format!(
            "{} input exceeds the {limit}-byte limit",
            role.as_str()
        )));
    }
    Ok(())
}

fn read_bounded_bytes(reader: impl Read, limit: u64) -> Result<Vec<u8>, FixtureManifestError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid(format!("file exceeds its {limit}-byte read limit")));
    }
    Ok(bytes)
}

fn metadata_changed(before: &Metadata, after: &Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

fn hash_file(mut file: File, metadata: &Metadata) -> Result<String, FixtureManifestError> {
    let hash = hash_reader(&mut file, metadata.len())?;
    if metadata_changed(metadata, &file.metadata()?) {
        return Err(invalid("input changed while hashing"));
    }
    Ok(hash)
}

fn hash_reader(reader: impl Read, expected_size: u64) -> Result<String, FixtureManifestError> {
    let mut reader = reader.take(expected_size.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&buffer[..count]);
    }
    if size != expected_size {
        return Err(invalid("input size changed while hashing"));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn manifest_identity(
    document: &FixtureManifestDocument,
    inputs: &[VerifiedFixtureInput],
) -> Result<String, FixtureManifestError> {
    let portable_inputs = inputs
        .iter()
        .map(|input| {
            serde_json::json!({
                "path": input.relative_path,
                "role": input.role,
                "size": input.size,
                "sha256": input.sha256,
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema": document.schema,
        "fixture_id": document.fixture_id,
        "dme_path": document.dme_path,
        "dmb_path": document.dmb_path,
        "rsc_path": document.rsc_path,
        "inputs": portable_inputs,
        "required_procs": document.required_procs,
        "required_tokens": document.required_tokens,
    }))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn invalid(message: impl Into<String>) -> FixtureManifestError {
    FixtureManifestError::Invalid(message.into())
}

fn path_identity(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn manifest_reader_bounds_trailing_whitespace_growth() {
        let mut bytes = vec![b' '; MAX_MANIFEST_BYTES as usize + 512];
        bytes[..2].copy_from_slice(b"{}");
        let mut reader = Cursor::new(bytes);
        assert!(read_bounded_bytes(&mut reader, MAX_MANIFEST_BYTES).is_err());
        assert_eq!(reader.position(), MAX_MANIFEST_BYTES + 1);
        assert_eq!(read_bounded_bytes(&b"{}  "[..], 4).unwrap(), b"{}  ");
    }

    #[test]
    fn input_hash_rejects_size_changes_without_reading_unbounded_growth() {
        let mut reader = Cursor::new(b"same-and-more");
        assert!(hash_reader(&mut reader, 4).is_err());
        assert_eq!(reader.position(), 5);
        assert!(hash_reader(&b"sam"[..], 4).is_err());
        assert_eq!(
            hash_reader(&b"same"[..], 4).unwrap(),
            format!("{:x}", Sha256::digest(b"same"))
        );
    }
}
