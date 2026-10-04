use anyhow::{anyhow, Result};

use std::path::PathBuf;

use super::DEFAULT_OUTPUT_WAIT_TIMEOUT_MS;

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
    pub fn new(args: crate::parameters::RunParams) -> Result<Self> {
        let daemon_args = args.daemon_args.unwrap_or_default();
        validate_daemon_args(&daemon_args)?;
        let readiness = args
            .wait_for
            .map(|pattern| {
                OutputPattern::new(pattern, args.wait_regex.unwrap_or(false), "wait_for")
            })
            .transpose()?;
        Ok(Self {
            dmb_path: args.dmb_path,
            port: args.port.unwrap_or(1337) as u16,
            working_directory: args.working_directory.map(PathBuf::from),
            daemon_args,
            readiness,
            startup_timeout_ms: args
                .startup_timeout_ms
                .unwrap_or(DEFAULT_OUTPUT_WAIT_TIMEOUT_MS),
            require_verified: args.require_verified_provenance.unwrap_or(false),
        })
    }
}

pub(crate) fn validate_daemon_args(arguments: &[String]) -> Result<()> {
    let mut values = arguments.iter();
    while let Some(argument) = values.next() {
        anyhow::ensure!(
            !argument.contains('\0'),
            "daemon_args must not contain NUL bytes"
        );
        let text = argument.trim();
        let flag_end =
            text.find(|character: char| character.is_ascii_whitespace() || character == '=');
        let flag = text[..flag_end.unwrap_or(text.len())].to_ascii_lowercase();
        let option = flag.trim_start_matches('-');
        anyhow::ensure!(
            !matches!(option, "ip" | "port" | "ports" | "cd"),
            "daemon_args cannot override {flag}; use port or working_directory, with the fixed loopback listener"
        );
        // BYOND consumes the next token as data for these options. A parameter
        // value such as "-ip" or "9999" is not another launch option.
        if matches!(flag.as_str(), "-params" | "-log" | "-home" | "-suid") {
            if flag_end.is_none() {
                let value = values
                    .next()
                    .ok_or_else(|| anyhow!("daemon_args {flag} requires a value"))?;
                anyhow::ensure!(
                    !value.contains('\0'),
                    "daemon_args must not contain NUL bytes"
                );
            }
            continue;
        }
        anyhow::ensure!(
            !text.chars().all(|character| character.is_ascii_digit())
                && text.parse::<i64>().is_err()
                && !matches!(flag.as_str(), "any" | "none")
                && !text.to_ascii_lowercase().ends_with(".dmb"),
            "daemon_args cannot supply a positional DMB or port; use dmb_path and port"
        );
    }
    Ok(())
}

pub(super) struct WaitOptions {
    pub pattern: OutputPattern,
    pub timeout_ms: u64,
}

impl WaitOptions {
    pub fn new(args: crate::parameters::WaitForOutputParams) -> Result<Self> {
        Ok(Self {
            pattern: OutputPattern::new(args.pattern, args.regex.unwrap_or(false), "pattern")?,
            timeout_ms: args.timeout_ms.unwrap_or(DEFAULT_OUTPUT_WAIT_TIMEOUT_MS),
        })
    }
}
