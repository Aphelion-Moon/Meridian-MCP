use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::PathBuf;

use super::{DEFAULT_OUTPUT_WAIT_TIMEOUT_MS, MAX_OUTPUT_WAIT_TIMEOUT_MS};

pub(super) struct OutputPattern {
    pub text: String,
    regex: Option<regex::Regex>,
}

impl OutputPattern {
    pub fn new(text: String, use_regex: bool, field: &str) -> Result<Self> {
        let regex = use_regex
            .then(|| regex::Regex::new(&text))
            .transpose()
            .map_err(|error| anyhow!("{field} is not a valid output regex: {error}"))?;
        Ok(Self { text, regex })
    }

    pub fn matches(&self, output: &str) -> bool {
        self.regex.as_ref().map_or_else(
            || output.contains(&self.text),
            |regex| regex.is_match(output),
        )
    }

    pub fn is_regex(&self) -> bool {
        self.regex.is_some()
    }
}

pub(super) struct RunOptions {
    pub dmb_path: String,
    pub port: u16,
    pub working_directory: Option<PathBuf>,
    pub daemon_args: Vec<String>,
    pub readiness: Option<OutputPattern>,
    pub startup_timeout_ms: u64,
    pub require_verified: bool,
}

impl RunOptions {
    pub fn parse(args: &Value) -> Result<Self> {
        let object = args
            .as_object()
            .ok_or_else(|| anyhow!("dm_run arguments must be an object"))?;
        for field in object.keys() {
            if ![
                "dmb_path",
                "port",
                "working_directory",
                "daemon_args",
                "wait_for",
                "wait_regex",
                "startup_timeout_ms",
                "require_verified_provenance",
            ]
            .contains(&field.as_str())
            {
                return Err(anyhow!("unknown dm_run argument: {field}"));
            }
        }
        let dmb_path = required_string(args, "dmb_path")?;
        let port = u16::try_from(crate::tools::bounded_u64(args, "port", 1337, 1, 65_535)?)?;
        let working_directory = optional_string(args, "working_directory")?.map(PathBuf::from);
        let daemon_args = match args.get("daemon_args") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| anyhow!("daemon_args must contain only strings"))
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(anyhow!("daemon_args must be an array of strings")),
        };
        let use_regex = optional_bool(args, "wait_regex")?;
        let readiness = optional_string(args, "wait_for")?
            .map(|pattern| OutputPattern::new(pattern, use_regex, "wait_for"))
            .transpose()?;
        let startup_timeout_ms = crate::tools::bounded_u64(
            args,
            "startup_timeout_ms",
            DEFAULT_OUTPUT_WAIT_TIMEOUT_MS,
            1,
            MAX_OUTPUT_WAIT_TIMEOUT_MS,
        )?;
        let require_verified = optional_bool(args, "require_verified_provenance")?;
        Ok(Self {
            dmb_path,
            port,
            working_directory,
            daemon_args,
            readiness,
            startup_timeout_ms,
            require_verified,
        })
    }
}

pub(super) struct WaitOptions {
    pub pattern: OutputPattern,
    pub timeout_ms: u64,
}

impl WaitOptions {
    pub fn parse(args: &Value) -> Result<Self> {
        if !args.is_object() {
            return Err(anyhow!("dm_wait_for_output arguments must be an object"));
        }
        let pattern = OutputPattern::new(
            required_string(args, "pattern")?,
            optional_bool(args, "regex")?,
            "pattern",
        )?;
        let timeout_ms = match args.get("timeout_ms") {
            None => DEFAULT_OUTPUT_WAIT_TIMEOUT_MS,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| anyhow!("timeout_ms must be a nonnegative integer"))?,
        }
        .min(MAX_OUTPUT_WAIT_TIMEOUT_MS);
        Ok(Self {
            pattern,
            timeout_ms,
        })
    }
}

fn optional_string(args: &Value, name: &str) -> Result<Option<String>> {
    args.get(name)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("{name} must be a string"))
        })
        .transpose()
}

fn required_string(args: &Value, name: &str) -> Result<String> {
    optional_string(args, name)?.ok_or_else(|| anyhow!("missing {name} argument"))
}

fn optional_bool(args: &Value, name: &str) -> Result<bool> {
    match args.get(name) {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| anyhow!("{name} must be a boolean")),
    }
}
