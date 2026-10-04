//! Explicit controls and source identity for the optional native memory helper.
use anyhow::{anyhow, bail, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SOURCE_REVISION: &str = "889006e334570a426f35c0a2f579c08d3d7b2186";
pub(crate) const MAX_DURATION_MS: u64 = 60_000;
pub(crate) const MAX_RECORDS: usize = 100_000;
pub(crate) const MAX_ROW_LIMIT: usize = 1000;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAction {
    Status,
    Start,
    Stop,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryControl {
    pub action: MemoryAction,
    #[serde(default = "default_duration")]
    #[schemars(range(min = 1, max = MAX_DURATION_MS))]
    pub duration_ms: u64,
    #[serde(default = "default_records")]
    #[schemars(range(min = 1, max = MAX_RECORDS))]
    pub max_records: usize,
    #[serde(default = "default_rows")]
    #[schemars(range(min = 1, max = MAX_ROW_LIMIT))]
    pub row_limit: usize,
}
fn default_duration() -> u64 {
    10_000
}
fn default_records() -> usize {
    20_000
}
fn default_rows() -> usize {
    100
}
impl MemoryControl {
    pub fn parse(value: Value) -> Result<Self> {
        crate::parameters::decode(value).map_err(Into::into)
    }
    pub fn command(&self) -> Result<String> {
        Ok(format!(
            "#meridian_memory_v1 {}",
            serde_json::to_string(self)?
        ))
    }
}

pub fn parse_response(text: &str) -> Result<Value> {
    if text.len() > 1_000_000 {
        bail!("Native memory response exceeds its byte limit");
    }
    let value: Value = serde_json::from_str(text)?;
    if value["protocol_version"] != 1 {
        bail!("Native memory helper protocol mismatch");
    }
    if value["evidence"]["ok"] != true {
        bail!(
            "{}",
            value["evidence"]["error"]
                .as_str()
                .unwrap_or("Native memory helper failed")
                .chars()
                .take(512)
                .collect::<String>()
        );
    }
    if !value["evidence"]["result"].is_object() {
        bail!("Native memory helper omitted its result");
    }
    Ok(value)
}

pub fn verified_memory_helper(manifest: &Path) -> Result<crate::helper_manifest::VerifiedHelper> {
    if !cfg!(windows) {
        bail!("Native memory capture is Windows-only");
    }
    let helper = crate::helper_manifest::verified_helper(
        manifest,
        crate::helper_manifest::HelperRequest {
            id: "auxtools-memory",
            platform: "windows",
            target_arch: "x86",
            source_revision: SOURCE_REVISION,
            protocol_version: Some(1),
            byond_version: Some("516.1687"),
        },
    )?;
    let expected = [
        (
            "accounting.rs",
            include_bytes!("../helpers/auxtools-memory/accounting.rs").as_slice(),
        ),
        (
            "mem_profiler.rs",
            include_bytes!("../helpers/auxtools-memory/mem_profiler.rs").as_slice(),
        ),
        (
            "protocol.patch",
            include_bytes!("../helpers/auxtools-memory/protocol.patch").as_slice(),
        ),
    ];
    if helper.patches.len() != expected.len()
        || expected.iter().any(|(name, bytes)| {
            !helper.patches.iter().any(|patch| {
                patch.name == *name && patch.patch_sha256 == format!("{:x}", Sha256::digest(bytes))
            })
        })
    {
        bail!("Native memory helper source overlay identity mismatch");
    }
    Ok(helper)
}

pub fn installed_memory_helper() -> Result<crate::helper_manifest::VerifiedHelper> {
    let executable = std::env::current_exe()?;
    let root = executable
        .parent()
        .ok_or_else(|| anyhow!("MCP executable has no parent directory"))?;
    verified_memory_helper(&root.join("helpers/auxtools-memory/manifest.json"))
}
