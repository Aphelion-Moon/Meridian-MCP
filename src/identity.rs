//! Volatile handles identify objects within one server lifetime, not disk content.
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

macro_rules! handle {
    ($name:ident, $prefix:literal, $pattern:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
        #[serde(transparent)]
        pub struct $name(#[schemars(regex(pattern = $pattern))] String);
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let value = String::deserialize(d)?;
                if value.len() != 67
                    || !value.starts_with($prefix)
                    || !value.as_bytes()[3..]
                        .iter()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
                {
                    return Err(serde::de::Error::custom("invalid volatile identity handle"));
                }
                Ok(Self(value))
            }
        }
        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}
handle!(SnapshotId, "s1:", "^s1:[0-9a-f]{64}$");
handle!(RuntimeId, "r1:", "^r1:[0-9a-f]{64}$");

pub(crate) fn snapshot(
    epoch: &[u8; 16],
    environment: &std::path::Path,
    generation: u64,
    instance: u64,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"meridian snapshot v1\0");
    hash.update(epoch);
    hash.update(environment.as_os_str().as_encoded_bytes());
    hash.update(generation.to_le_bytes());
    hash.update(instance.to_le_bytes());
    format!("s1:{:x}", hash.finalize())
}

pub(crate) fn runtime(epoch: &[u8; 16], sequence: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(b"meridian runtime v1\0");
    hash.update(epoch);
    hash.update(sequence.to_le_bytes());
    format!("r1:{:x}", hash.finalize())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnalysisIdentity {
    pub snapshot_id: String,
    pub generation: u64,
    pub state_generation: u64,
    pub environment: std::path::PathBuf,
    pub source: &'static str,
    pub completeness: &'static str,
    pub disk_state: &'static str,
    pub recomputed: bool,
    pub refresh_with: &'static str,
}

#[derive(Debug, thiserror::Error)]
#[error("{field} does not identify the captured session")]
pub(crate) struct StaleIdentity {
    pub field: &'static str,
    pub expected: String,
    pub current: Option<String>,
}
impl StaleIdentity {
    pub(crate) fn result(&self) -> crate::result::ToolResult {
        crate::result::structured_error(
            crate::result::ToolErrorCode::StaleGeneration,
            self.to_string(),
            Some("Read the current identity and retry as a new operation.".into()),
            serde_json::json!({"field":self.field,"expected":self.expected,"current":self.current}),
        )
    }
}
