use super::response::ResponseOptions;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::PathBuf;

pub(super) struct CompileOptions {
    pub dme_path: String,
    pub compiler_path: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub fixture_manifest_path: Option<PathBuf>,
    pub defines: Vec<String>,
    pub timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub capture_network: bool,
    pub response: ResponseOptions,
}

impl CompileOptions {
    pub fn parse(args: &Value) -> Result<Self> {
        let object = args
            .as_object()
            .ok_or_else(|| anyhow!("dm_compile arguments must be an object"))?;
        for name in object.keys() {
            anyhow::ensure!(
                [
                    "dme_path",
                    "compiler_path",
                    "working_directory",
                    "fixture_manifest_path",
                    "defines",
                    "timeout_ms",
                    "idle_timeout_ms",
                    "capture_network",
                    "include_output",
                    "output_max_bytes",
                    "diagnostic_limit",
                ]
                .contains(&name.as_str()),
                "unknown dm_compile argument: {name}"
            );
        }
        let dme_path = optional_string(args, "dme_path")?
            .ok_or_else(|| anyhow!("missing dme_path argument"))?;
        let compiler_path = optional_string(args, "compiler_path")?;
        let working_directory = optional_string(args, "working_directory")?.map(PathBuf::from);
        if working_directory
            .as_ref()
            .is_some_and(|path| !path.is_dir())
        {
            return Err(anyhow!("working_directory must be an existing directory"));
        }
        let fixture_manifest_path =
            optional_string(args, "fixture_manifest_path")?.map(PathBuf::from);
        let defines = match args.get("defines") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    let value = value
                        .as_str()
                        .ok_or_else(|| anyhow!("defines must contain only strings"))?;
                    anyhow::ensure!(!value.contains('\0'), "defines must not contain NUL bytes");
                    Ok(value.to_owned())
                })
                .collect::<Result<Vec<_>>>()?,
            Some(_) => return Err(anyhow!("defines must be an array of strings")),
        };
        let capture_network = match args.get("capture_network") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| anyhow!("capture_network must be a boolean"))?,
        };
        Ok(Self {
            dme_path,
            compiler_path,
            working_directory,
            fixture_manifest_path,
            defines,
            capture_network,
            response: ResponseOptions::parse(args)?,
            timeout_ms: capped_timeout(args, "timeout_ms", 600_000, 1, 1_800_000)?,
            idle_timeout_ms: capped_timeout(
                args,
                "idle_timeout_ms",
                super::DEFAULT_IDLE_TIMEOUT_MS,
                1_000,
                super::MAX_IDLE_TIMEOUT_MS,
            )?,
        })
    }
}

fn optional_string(args: &Value, name: &str) -> Result<Option<String>> {
    args.get(name)
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| anyhow!("{name} must be a string"))?;
            anyhow::ensure!(!value.contains('\0'), "{name} must not contain NUL bytes");
            Ok(value.to_owned())
        })
        .transpose()
}

fn capped_timeout(
    args: &Value,
    name: &str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64> {
    let value = match args.get(name) {
        None => default,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow!("{name} must be an integer"))?,
    };
    anyhow::ensure!(value >= minimum, "{name} must be at least {minimum}");
    Ok(value.min(maximum))
}
