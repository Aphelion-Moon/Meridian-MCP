use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::OnceLock;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct BuildIdentityInput {
    pub version: String,
    pub source_revision: Option<String>,
    pub source_dirty: Option<bool>,
    pub target: String,
    pub profile: String,
    pub executable_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct BuildIdentity {
    pub schema: u32,
    pub build_id: String,
    pub complete: bool,
    pub version: String,
    pub source_revision: Option<String>,
    pub source_dirty: Option<bool>,
    pub target: String,
    pub profile: String,
    pub executable_sha256: Option<String>,
}

impl BuildIdentity {
    pub fn from_input(input: BuildIdentityInput) -> Self {
        let complete = input.source_revision.is_some()
            && input.source_dirty.is_some()
            && input.executable_sha256.is_some()
            && input.target != "unknown"
            && input.profile != "unknown";
        let bytes = serde_json::to_vec(&input).expect("build identity serialization cannot fail");
        Self {
            schema: 1,
            build_id: format!("{:x}", Sha256::digest(bytes)),
            complete,
            version: input.version,
            source_revision: input.source_revision,
            source_dirty: input.source_dirty,
            target: input.target,
            profile: input.profile,
            executable_sha256: input.executable_sha256,
        }
    }

    pub fn as_input(&self) -> BuildIdentityInput {
        BuildIdentityInput {
            version: self.version.clone(),
            source_revision: self.source_revision.clone(),
            source_dirty: self.source_dirty,
            target: self.target.clone(),
            profile: self.profile.clone(),
            executable_sha256: self.executable_sha256.clone(),
        }
    }
}

pub fn current() -> &'static BuildIdentity {
    static IDENTITY: OnceLock<BuildIdentity> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        BuildIdentity::from_input(BuildIdentityInput {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            source_revision: known_string(env!("MERIDIAN_BUILD_REVISION")),
            source_dirty: match env!("MERIDIAN_BUILD_DIRTY") {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            target: env!("MERIDIAN_BUILD_TARGET").to_owned(),
            profile: env!("MERIDIAN_BUILD_PROFILE").to_owned(),
            executable_sha256: std::env::current_exe()
                .ok()
                .and_then(|path| hash_file(&path).ok()),
        })
    })
}

fn known_string(value: &str) -> Option<String> {
    (value != "unknown" && !value.is_empty()).then(|| value.to_owned())
}

fn hash_file(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
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
