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
        .ok_or_else(|| anyhow!("dmdoc helper unavailable"))?
        .to_owned();
    let snapshot = state.snapshot().await?;
    let output = options.output;
    let initial_context = context.clone();
    let initial_snapshot = snapshot.clone();
    let initial_output = output.clone();
    let initial_helper = helper.clone();
    let (initial_inputs, mut staging) = state
        .run_mutation_job(context, move || {
            let context = &initial_context;
            let snapshot = &initial_snapshot;
            let output = &initial_output;
            let helper = &initial_helper;
            context.progress(crate::request::Stage::Capture);
            let parent = context.policy().read_directory(
                output
                    .parent()
                    .ok_or_else(|| anyhow!("output has no parent"))?,
            )?;
            anyhow::ensure!(
                !helper.starts_with(output)
                    && !snapshot
                        .source_inputs()
                        .iter()
                        .any(|input| input.starts_with(output)),
                "documentation output must not contain parsed project inputs or the helper"
            );
            let exists = validate_directory_target(output)?;
            anyhow::ensure!(
                !exists || options.overwrite,
                "output exists; set overwrite=true"
            );
            let initial_inputs = inputs::capture_checked(
                context.policy(),
                &snapshot.environment_path,
                output,
                || context.checkpoint(),
            )?;
            context.checkpoint()?;
            let staging = StagingDirectory::create(&parent)?;
            staging.check_installation_support()?;
            Ok((initial_inputs, staging))
        })
        .await?;
    let working_directory = snapshot
        .environment_path
        .parent()
        .ok_or_else(|| anyhow!("parsed environment has no parent directory"))?
        .to_owned();
    let limits = ServerLimits::default();
    let mut process_attempted = false;
    let outcome = async {
        context.checkpoint()?;
        context.progress(crate::request::Stage::Execution);
        process_attempted = true;
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
            timeout: context
                .deadline(limits.max_docs_duration_ms)
                .saturating_duration_since(tokio::time::Instant::now()),
            idle_timeout: Duration::from_millis(limits.max_docs_duration_ms),
            capture_network: false,
            cancellation: context.cancellation.clone(),
        })
        .await
    }
    .await;
    let known_process = crate::result::MutationOutcome {
        operation_ran: process_attempted,
        process_started: outcome
            .as_ref()
            .is_ok_and(|outcome| outcome.process_started),
        process_stopped: outcome
            .as_ref()
            .is_ok_and(|outcome| outcome.process_started),
        process_start_uncertain: process_attempted
            && outcome.as_ref().map_or(true, |outcome| {
                outcome.termination == TerminationReason::SpawnFailed
            }),
        cleanup_complete: false,
        recovery_required: true,
        action: Some("generate_docs".into()),
        ..Default::default()
    };
    let control = context.clone();
    state
        .run_blocking_job(move || {
            let context = &control;
            context.progress(crate::request::Stage::Evidence);
            let mut result = crate::outputs::DocsData {
                output_directory: Some(output.clone()),
                helper: Some(helper),
                source_revision: snapshot.spacemandmm_revision.into(),
                cleanup_complete: true,
                stdout: Some(String::new()),
                stderr: Some(String::new()),
                ..Default::default()
            };
            let process_started = outcome
                .as_ref()
                .is_ok_and(|outcome| outcome.process_started);
            let process_start_uncertain = process_attempted
                && outcome.as_ref().map_or(true, |outcome| {
                    outcome.termination == TerminationReason::SpawnFailed
                });
            let installation = match outcome {
                Err(error) => Err(anyhow!("dmdoc process setup failed: {error}")),
                Ok(outcome) => {
                    result.duration_ms = Some(outcome.duration_ms);
                    result.termination = Some(outcome.termination);
                    result.exit_code = Some(outcome.exit_code);
                    result.stdout = Some(outcome.stdout.text);
                    result.stderr = Some(outcome.stderr.text);
                    result.stdout_truncated_bytes = outcome.stdout.truncated_bytes;
                    result.stderr_truncated_bytes = outcome.stderr.truncated_bytes;
                    result.output_complete = Some(outcome.output_complete);
                    if outcome.termination != TerminationReason::Exited
                        || outcome.exit_code != Some(0)
                    {
                        Err(anyhow!(
                            "dmdoc failed ({:?}, exit {:?})",
                            outcome.termination,
                            outcome.exit_code
                        ))
                    } else {
                        match inputs::capture_checked(
                            context.policy(),
                            &snapshot.environment_path,
                            &output,
                            || context.checkpoint(),
                        ) {
                            Ok(current) if current == initial_inputs => install_generated(
                                &mut staging,
                                &output,
                                options.overwrite,
                                &limits,
                                &mut result,
                                || context.checkpoint(),
                            ),
                            Ok(_) => Err(anyhow!("documentation inputs changed during generation")),
                            Err(error) => Err(error),
                        }
                    }
                }
            };
            if let Err(error) = installation {
                result.code = Some("documentation_generation_failed".into());
                result.message = Some(error.to_string());
            }
            if let Some(error) = staging.cleanup() {
                result.success = false;
                result.cleanup_complete = false;
                result.code = Some("documentation_cleanup_incomplete".into());
                result.staging_directory = Some(staging.path().into());
                result.staging_name = staging
                    .path()
                    .file_name()
                    .map(|v| v.to_string_lossy().into_owned());
                result.staging_cleanup_error = Some(error);
            }
            let data = result;
            let essential = crate::result::MutationOutcome {
                operation_ran: true,
                operation_succeeded: Some(data.success),
                failure_code: data.code.clone(),
                process_started,
                process_stopped: process_started,
                process_start_uncertain,
                cleanup_complete: data.cleanup_complete && !process_start_uncertain,
                recovery_required: !data.cleanup_complete || process_start_uncertain,
                outputs: vec![crate::result::OutputMutation {
                    request_index: 0,
                    installed: data.installed,
                    path_preview: Some(crate::result::path_preview(&output)),
                    sha256: None,
                    cleanup_complete: data.cleanup_complete,
                    backup_preview: data
                        .backup_directory
                        .as_deref()
                        .map(crate::result::path_preview),
                }],
                ..Default::default()
            };
            let failed = !data.success;
            let projected =
                build_response::project_helper(data, options.response).map_err(|error| {
                    crate::result::OutcomeError {
                        error,
                        outcome: essential.clone(),
                    }
                })?;
            let mut metadata = ToolMetadata::for_snapshot(&snapshot);
            metadata.truncated = projected.truncated;
            metadata.truncation_reasons = projected.truncation_reasons;
            let mut result = crate::result::json_success_compact(metadata, projected.data)
                .with_outcome(essential);
            if failed {
                result.is_error = Some(true);
            }
            Ok(result)
        })
        .await
        .map_err(|error| {
            if error.is::<crate::result::OutcomeError>() {
                error
            } else {
                crate::result::OutcomeError {
                    error,
                    outcome: known_process,
                }
                .into()
            }
        })
}

fn install_generated(
    staging: &mut StagingDirectory,
    output: &Path,
    overwrite: bool,
    limits: &ServerLimits,
    result: &mut crate::outputs::DocsData,
    checkpoint: impl Fn() -> Result<()>,
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
    result.files = Some(files);
    result.bytes = Some(bytes);
    checkpoint()?;
    let installation = staging.install(output, overwrite)?;
    // Capture installed state before any later reply serialization can fail.
    result.installation(installation);
    if result.installed {
        result.index = Some(output.join("index.html"));
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
