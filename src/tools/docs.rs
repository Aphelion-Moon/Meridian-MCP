mod inputs;
mod installation;

use super::build_response::{self, ResponseOptions};
use crate::limits::ServerLimits;
use crate::mcp::ToolResult;
use crate::process::{run_contained_process, ProcessSpec, TerminationReason};
use crate::result::ToolMetadata;
use crate::state::ServerState;
use crate::tools::ToolExecutionContext;
use anyhow::{anyhow, Result};
use installation::{validate_directory_target, StagingDirectory};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Options {
    output: PathBuf,
    overwrite: bool,
    response: ResponseOptions,
}
impl From<crate::parameters::GenerateDocsParams> for Options {
    fn from(args: crate::parameters::GenerateDocsParams) -> Self {
        Self {
            output: args.output_directory.into(),
            overwrite: args.overwrite.unwrap_or(false),
            response: ResponseOptions::new(args.include_output, args.output_max_bytes, None),
        }
    }
}

pub async fn generate(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::GenerateDocsParams,
) -> Result<ToolResult> {
    let options = Options::from(args);
    let helper = context
        .dmdoc_helper()
        .ok_or_else(|| anyhow!("dmdoc helper unavailable"))?;
    let snapshot = state.snapshot().await?;
    let output = options.output;
    let parent = context.policy().read_directory(
        output
            .parent()
            .ok_or_else(|| anyhow!("output has no parent"))?,
    )?;
    anyhow::ensure!(
        !helper.starts_with(&output)
            && !snapshot
                .source_inputs()
                .iter()
                .any(|input| input.starts_with(&output)),
        "documentation output must not contain parsed project inputs or the helper"
    );
    let exists = validate_directory_target(&output)?;
    anyhow::ensure!(
        !exists || options.overwrite,
        "output exists; set overwrite=true"
    );
    let working_directory = snapshot
        .environment_path
        .parent()
        .ok_or_else(|| anyhow!("parsed environment has no parent directory"))?
        .to_owned();
    let policy = context.policy().clone();
    let environment = snapshot.environment_path.clone();
    let target = output.clone();
    let initial_inputs = state
        .run_asset_job(move || inputs::capture(&policy, &environment, &target))
        .await?;
    let mut staging = StagingDirectory::create(&parent)?;
    let limits = ServerLimits::default();
    let outcome = async {
        staging.check_installation_support()?;
        run_contained_process(ProcessSpec {
            program: helper.to_owned(),
            arguments: vec![
                "-e".into(),
                snapshot.environment_path.as_os_str().to_owned(),
                "--output".into(),
                staging.path().as_os_str().to_owned(),
            ],
            working_directory,
            environment: Vec::new(),
            stdin: None,
            timeout: Duration::from_millis(limits.max_docs_duration_ms),
            idle_timeout: Duration::from_millis(limits.max_docs_duration_ms),
            capture_network: false,
            cancellation: None,
        })
        .await
    }
    .await;
    let mut result = json!({"output_directory":output,"helper":helper,"source_revision":snapshot.spacemandmm_revision,
        "installed":false,"success":false,"cleanup_complete":true,"stdout":"","stderr":"",
        "stdout_truncated_bytes":0,"stderr_truncated_bytes":0});
    let installation = match outcome {
        Err(error) => Err(anyhow!("dmdoc process setup failed: {error}")),
        Ok(outcome) => {
            result["duration_ms"] = json!(outcome.duration_ms);
            result["termination"] = json!(outcome.termination);
            result["exit_code"] = json!(outcome.exit_code);
            result["stdout"] = json!(outcome.stdout.text);
            result["stderr"] = json!(outcome.stderr.text);
            result["stdout_truncated_bytes"] = json!(outcome.stdout.truncated_bytes);
            result["stderr_truncated_bytes"] = json!(outcome.stderr.truncated_bytes);
            result["output_complete"] = json!(outcome.output_complete);
            if outcome.termination != TerminationReason::Exited || outcome.exit_code != Some(0) {
                Err(anyhow!(
                    "dmdoc failed ({:?}, exit {:?})",
                    outcome.termination,
                    outcome.exit_code
                ))
            } else {
                let policy = context.policy().clone();
                let environment = snapshot.environment_path.clone();
                let target = output.clone();
                match state
                    .run_asset_job(move || inputs::capture(&policy, &environment, &target))
                    .await
                {
                    Ok(current) if current == initial_inputs => install_generated(
                        &mut staging,
                        &output,
                        options.overwrite,
                        &limits,
                        &mut result,
                    ),
                    Ok(_) => Err(anyhow!("documentation inputs changed during generation")),
                    Err(error) => Err(error),
                }
            }
        }
    };
    if let Err(error) = installation {
        result["code"] = json!("documentation_generation_failed");
        result["message"] = json!(error.to_string());
    }
    if let Some(error) = staging.cleanup() {
        result["success"] = json!(false);
        result["cleanup_complete"] = json!(false);
        result["code"] = json!("documentation_cleanup_incomplete");
        result["staging_directory"] = json!(staging.path());
        result["staging_name"] = json!(staging.path().file_name());
        result["staging_cleanup_error"] = json!(error);
    }
    result.as_object_mut().unwrap().extend(
        serde_json::to_value(ToolMetadata::for_snapshot(&snapshot))?
            .as_object()
            .unwrap()
            .clone(),
    );
    let failed = result["success"] != true;
    let text = build_response::format_helper(result, options.response)?;
    Ok(if failed {
        ToolResult::error(text)
    } else {
        ToolResult::text(text)
    })
}

fn install_generated(
    staging: &mut StagingDirectory,
    output: &Path,
    overwrite: bool,
    limits: &ServerLimits,
    result: &mut Value,
) -> Result<()> {
    anyhow::ensure!(
        validate_directory_target(staging.path())?,
        "dmdoc staging directory is missing"
    );
    anyhow::ensure!(
        staging.path().join("index.html").is_file(),
        "dmdoc did not produce index.html"
    );
    let (files, bytes) = directory_stats(staging.path(), limits)?;
    result["files"] = json!(files);
    result["bytes"] = json!(bytes);
    let installation = staging.install(output, overwrite)?;
    result
        .as_object_mut()
        .unwrap()
        .extend(installation.as_object().unwrap().clone());
    if result["installed"] == true {
        result["index"] = json!(output.join("index.html"));
    }
    Ok(())
}

fn directory_stats(root: &Path, limits: &ServerLimits) -> Result<(usize, u64)> {
    let mut stack = vec![root.to_owned()];
    let mut files = 0;
    let mut bytes = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(anyhow!("dmdoc output contains a symbolic link"));
            }
            if file_type.is_dir() {
                stack.push(entry.path())
            } else if file_type.is_file() {
                files += 1;
                bytes += entry.metadata()?.len();
                if files > limits.max_docs_files {
                    return Err(anyhow!("dmdoc output exceeds max_docs_files"));
                }
                if bytes > limits.max_docs_output_bytes {
                    return Err(anyhow!("dmdoc output exceeds max_docs_output_bytes"));
                }
            } else {
                return Err(anyhow!("dmdoc output contains an unsupported file type"));
            }
        }
    }
    Ok((files, bytes))
}
