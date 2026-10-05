use anyhow::{anyhow, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tracing::info;

mod arguments;
mod diagnostics;
use super::build_response as response;
use arguments::CompileOptions;

use super::ToolExecutionContext;
use crate::artifact::{ArtifactSnapshot, FileIdentity};
use crate::build_provenance::{
    BuildAttempt, BuildAttemptOutcome, BuildRecord, PreparedBuild, ProvenanceStatus,
};
use crate::fixture_manifest::VerifiedFixtureManifest;
use crate::mcp::ToolResult;
use crate::process::{run_owned_process_observed, ProcessSpec, TerminationReason};
use crate::state::ServerState;

const DEFAULT_IDLE_TIMEOUT_MS: u64 = 45_000;

#[derive(Debug, PartialEq, Eq)]
enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, PartialEq, Eq)]
struct CompilerDiagnostic<'a> {
    file: &'a str,
    line: u32,
    column: Option<u32>,
    severity: DiagnosticSeverity,
    message: &'a str,
}

fn diagnostic_regex() -> &'static Regex {
    static DIAGNOSTIC_REGEX: OnceLock<Regex> = OnceLock::new();
    DIAGNOSTIC_REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)^(?P<file>.*?):(?P<line>\d+)(?::(?P<column>\d+))?:\s*(?P<severity>error|warning)\b\s*:?[\s]*(?P<message>.*)$",
        ).expect("compiler diagnostic regex must be valid")
    })
}

fn parse_diagnostic_line(line: &str) -> Option<CompilerDiagnostic<'_>> {
    let captures = diagnostic_regex().captures(line.trim_end())?;
    let severity = match captures
        .name("severity")?
        .as_str()
        .to_ascii_lowercase()
        .as_str()
    {
        "error" => DiagnosticSeverity::Error,
        "warning" => DiagnosticSeverity::Warning,
        _ => return None,
    };

    Some(CompilerDiagnostic {
        file: captures.name("file")?.as_str(),
        line: captures.name("line")?.as_str().parse().ok()?,
        column: captures
            .name("column")
            .and_then(|value| value.as_str().parse().ok()),
        severity,
        message: captures.name("message")?.as_str().trim(),
    })
}

fn diagnostic_to_value(diagnostic: &CompilerDiagnostic) -> Value {
    json!({
        "file": diagnostic.file,
        "line": diagnostic.line,
        "column": diagnostic.column,
        "severity": match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
        },
        "message": diagnostic.message,
    })
}

fn compile_succeeded(process_succeeded: bool, error_count: u64) -> bool {
    process_succeeded && error_count == 0
}

fn normalize_spawn_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path_text = path.to_string_lossy();
        if let Some(unc_path) = path_text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc_path}"));
        }
        if let Some(dos_path) = path_text.strip_prefix(r"\\?\") {
            return PathBuf::from(dos_path);
        }
    }

    path.to_path_buf()
}

fn compiler_dme_argument(path: &Path, working_directory: &Path) -> String {
    path.strip_prefix(working_directory)
        .ok()
        .filter(|relative_path| !relative_path.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn relative_dme_paths_resolve_from_requested_working_directory() {
        let resolved = resolve_requested_path(
            Path::new("project/tgstation.dme"),
            Some(Path::new("workspace")),
        );

        assert_eq!(resolved, PathBuf::from("workspace/project/tgstation.dme"));
    }

    #[test]
    fn diagnostic_parser_handles_windows_paths_and_columns() {
        let diagnostic =
            parse_diagnostic_line(r"C:\workspace\code\example.dm:42:7: error: unexpected token")
                .expect("diagnostic should parse");

        assert_eq!(diagnostic.file, r"C:\workspace\code\example.dm");
        assert_eq!(diagnostic.line, 42);
        assert_eq!(diagnostic.column, Some(7));
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostic.message, "unexpected token");
    }

    #[test]
    fn diagnostic_parser_handles_line_only_warnings() {
        let diagnostic = parse_diagnostic_line("code/example.dm:9: warning: deprecated syntax")
            .expect("diagnostic should parse");

        assert_eq!(diagnostic.file, "code/example.dm");
        assert_eq!(diagnostic.line, 9);
        assert_eq!(diagnostic.column, None);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning);
        assert_eq!(diagnostic.message, "deprecated syntax");
    }

    #[test]
    fn diagnostic_parser_ignores_unstructured_output() {
        assert!(parse_diagnostic_line("DreamMaker finished with 0 errors").is_none());
    }

    #[test]
    fn compiler_diagnostics_override_a_zero_process_exit_code() {
        assert!(compile_succeeded(true, 0));
        assert!(!compile_succeeded(true, 1));
        assert!(!compile_succeeded(false, 0));
    }

    #[cfg(windows)]
    #[test]
    fn compiler_spawn_paths_drop_windows_verbatim_prefix() {
        let verbatim = Path::new(r"\\?\C:\workspace\tgstation.dme");
        assert_eq!(
            normalize_spawn_path(verbatim),
            PathBuf::from(r"C:\workspace\tgstation.dme")
        );

        let unc_verbatim = Path::new(r"\\?\UNC\server\share\tgstation.dme");
        assert_eq!(
            normalize_spawn_path(unc_verbatim),
            PathBuf::from(r"\\server\share\tgstation.dme")
        );
    }

    #[test]
    fn compiler_uses_a_dme_path_relative_to_the_spawn_directory() {
        let working_directory = Path::new(r"C:\workspace\project");
        let dme_path = working_directory.join("tgstation.dme");

        assert_eq!(
            compiler_dme_argument(&dme_path, working_directory),
            "tgstation.dme"
        );
    }
}

fn resolve_requested_path(requested_path: &Path, working_directory: Option<&Path>) -> PathBuf {
    if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else if let Some(working_directory) = working_directory {
        working_directory.join(requested_path)
    } else {
        requested_path.to_path_buf()
    }
}

fn compiler_environment() -> Vec<(OsString, OsString)> {
    #[cfg(windows)]
    let names = [
        "SystemRoot",
        "SystemDrive",
        "WINDIR",
        "ComSpec",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
    ];
    #[cfg(not(windows))]
    let names = ["PATH", "HOME", "TMPDIR", "LD_LIBRARY_PATH"];

    names
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.into(), value)))
        .collect()
}

/// Compile a DreamMaker environment
pub async fn compile(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::CompileParams,
) -> Result<ToolResult> {
    let CompileOptions {
        dme_path,
        compiler_path,
        working_directory: requested_working_directory,
        fixture_manifest_path,
        defines,
        timeout_ms,
        idle_timeout_ms,
        capture_network,
        response,
    } = CompileOptions::from(args);
    let requested_path = PathBuf::from(&dme_path);
    let path = resolve_requested_path(&requested_path, requested_working_directory.as_deref());
    if !path.is_file() {
        return Ok(ToolResult::structured_error(
            "invalid_input",
            "dme_path must be an existing regular file",
            "Select a contained DreamMaker environment file.",
        ));
    }
    let path = path.canonicalize()?;
    let snapshot = state.active_snapshot().await;
    let fixture = match fixture_manifest_path.as_deref() {
        Some(path) => Some(super::fixture::load_manifest(context, state, path).await?),
        None => None,
    };
    if let Some(fixture) = &fixture {
        if fixture.dme_path != path || fixture.dmb_path != path.with_extension("dmb") {
            return Ok(ToolResult::structured_error(
                "fixture_manifest_mismatch",
                "fixture manifest DME/DMB paths do not match the compile request",
                "Select the exact manifest for this contained DreamMaker environment.",
            ));
        }
    }

    let compiler = if let Some(compiler) = compiler_path {
        match context.policy().executable(compiler) {
            Ok(compiler) => compiler,
            Err(error) => {
                return Ok(ToolResult::structured_error(
                    error.code(),
                    error.to_string(),
                    "Configure an existing DreamMaker executable in MERIDIAN_MCP_COMPILERS.",
                ));
            }
        }
    } else {
        let configured = match context.policy().compiler_allowlist() {
            [] => {
                return Ok(ToolResult::structured_error(
                    "compiler_not_configured",
                    "dm_compile requires a startup-allowlisted DreamMaker compiler when compiler_path is omitted.",
                    "Restart Meridian-MCP with exactly one intended compiler in MERIDIAN_MCP_COMPILERS, or supply an explicitly allowlisted compiler_path.",
                ));
            }
            [compiler] => compiler,
            _ => {
                return Ok(ToolResult::structured_error(
                    "compiler_ambiguous",
                    "dm_compile cannot select among multiple startup-allowlisted compilers when compiler_path is omitted.",
                    "Supply compiler_path naming one allowlisted compiler, or restart Meridian-MCP with exactly one intended compiler in MERIDIAN_MCP_COMPILERS.",
                ));
            }
        };
        match context.policy().executable(configured) {
            Ok(compiler) => compiler,
            Err(error) => {
                return Ok(ToolResult::structured_error(
                    error.code(),
                    error.to_string(),
                    "Configure an existing DreamMaker executable in MERIDIAN_MCP_COMPILERS.",
                ));
            }
        }
    };

    let working_directory = requested_working_directory
        .map(|directory| resolve_requested_path(&directory, None))
        .map(|directory| directory.canonicalize())
        .transpose()?
        .or_else(|| path.parent().map(PathBuf::from));

    info!(
        "Compiling {} with {:?} (timeout {} ms)",
        dme_path, compiler, timeout_ms
    );

    let spawn_path = normalize_spawn_path(&path);
    let spawn_working_directory = working_directory.as_deref().map(normalize_spawn_path);
    let compiler_working_directory = spawn_working_directory
        .as_deref()
        .unwrap_or_else(|| spawn_path.parent().unwrap_or(Path::new(".")));
    let dme_argument = compiler_dme_argument(&spawn_path, compiler_working_directory);
    let arguments: Vec<OsString> = defines
        .iter()
        .map(|define| {
            OsString::from(if define.starts_with("-D") {
                define.clone()
            } else {
                format!("-D{define}")
            })
        })
        .chain(std::iter::once(OsString::from(&dme_argument)))
        .collect();
    let dmb_path = path.with_extension("dmb");
    anyhow::ensure!(
        path.parent().is_some(),
        "DreamMaker environment has no project root"
    );
    let deadline = context.deadline(timeout_ms);
    let lease = match context
        .execution_lease(
            state,
            &dmb_path,
            compiler_working_directory,
            "compile",
            deadline,
        )
        .await?
    {
        Ok(lease) => lease,
        Err(result) => return Ok(result),
    };
    let compiler_working_directory = compiler_working_directory.to_owned();
    let initial_context = context.clone();
    let initial_path = path.clone();
    let initial_compiler = compiler.clone();
    let initial_fixture = fixture.clone();
    let initial_working = compiler_working_directory.clone();
    let initial_dmb = dmb_path.clone();
    let initial_arguments = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let (mut lease, artifact_before, prepared, attempt) = state
        .run_mutation_job(context, move || {
            let checkpoint = || initial_context.checkpoint();
            initial_context.progress(crate::request::Stage::Capture);
            let root = initial_path.parent().expect("environment parent");
            let artifact_before =
                ArtifactSnapshot::capture_checked(root, &initial_dmb, checkpoint)?;
            let prepared = PreparedBuild::capture_checked(
                initial_context.policy(),
                snapshot.as_deref(),
                initial_fixture.as_ref(),
                &initial_path,
                &initial_compiler,
                initial_arguments,
                initial_working,
                checkpoint,
            )?;
            let attempt = initial_context
                .build_provenance()
                .map(|store| {
                    store.begin_attempt_checked(&initial_dmb, prepared.inputs.clone(), checkpoint)
                })
                .transpose()?;
            Ok((lease, artifact_before, prepared, attempt))
        })
        .await?;
    let mut diagnostics = diagnostics::CompilerDiagnostics::new(response.diagnostic_limit());
    if let Some(code) = context.finalization_reason(deadline) {
        let control = context.clone();
        return state
            .run_blocking_job(move || {
                let mut essential = crate::result::MutationOutcome {
                    operation_ran: attempt.is_some(),
                    operation_succeeded: Some(false),
                    failure_code: Some(code.to_owned()),
                    attempt_id: attempt.as_ref().map(|attempt| attempt.attempt_id.clone()),
                    action: Some("compile_pre_spawn".into()),
                    cleanup_complete: true,
                    ..Default::default()
                };
                let cleanup = (|| -> Result<()> {
                    if let (Some(store), Some(mut attempt)) = (control.build_provenance(), attempt)
                    {
                        attempt.outcome = BuildAttemptOutcome::Interrupted {
                            code: code.to_owned(),
                        };
                        store.finish_attempt(&attempt, None)?;
                    }
                    lease.finish()?;
                    Ok(())
                })();
                if let Err(error) = cleanup {
                    essential.cleanup_complete = false;
                    essential.recovery_required = true;
                    return Err(crate::result::OutcomeError {
                        error,
                        outcome: essential,
                    }
                    .into());
                }
                Ok(ToolResult::structured_error(
                    code,
                    "compilation ended before process spawn",
                    "Retry after cleanup.",
                )
                .with_outcome(essential))
            })
            .await;
    }
    context.progress(crate::request::Stage::Execution);
    lease.mark_writer_started();
    let execution = run_owned_process_observed(
        ProcessSpec {
            program: compiler.clone(),
            arguments,
            working_directory: compiler_working_directory.to_owned(),
            environment: compiler_environment(),
            stdin: None,
            timeout: deadline.saturating_duration_since(tokio::time::Instant::now()),
            idle_timeout: Duration::from_millis(idle_timeout_ms),
            capture_network,
            cancellation: context.cancellation(),
        },
        |stream, bytes| diagnostics.observe(stream, bytes),
    )
    .await
    .map_err(|error| crate::result::OutcomeError {
        error,
        outcome: crate::result::MutationOutcome {
            operation_ran: attempt.is_some(),
            process_start_uncertain: true,
            recovery_required: true,
            attempt_id: attempt.as_ref().map(|attempt| attempt.attempt_id.clone()),
            action: Some("compile_setup".into()),
            ..Default::default()
        },
    })?;

    let context = context.clone();
    state.run_blocking_job(move || {
    context.progress(crate::request::Stage::Evidence);
    let project_root = path.parent().expect("environment parent");
    let stdout = &execution.stdout.text;
    let stderr = &execution.stderr.text;

    let diagnostics = diagnostics.finish(
        execution.output_complete,
        execution.stdout.truncated_bytes == 0 && execution.stderr.truncated_bytes == 0,
    );

    let mut essential = crate::result::MutationOutcome {
        operation_ran: true,
        process_started: execution.process_started,
        process_stopped: execution.process_started,
        cleanup_complete: false,
        recovery_required: true,
        attempt_id: attempt.as_ref().map(|value| value.attempt_id.clone()),
        error_count: Some(diagnostics.error_count),
        ..Default::default()
    };
    let artifact_after = ArtifactSnapshot::capture_after(project_root, &dmb_path, || context.checkpoint()).map_err(|error| {
        crate::result::OutcomeError {
            error,
            outcome: essential.clone(),
        }
    })?;
    let process_succeeded =
        execution.termination == TerminationReason::Exited && execution.exit_code == Some(0);
    let compiler_succeeded = compile_succeeded(
        process_succeeded && diagnostics.complete,
        diagnostics.error_count,
    );
    let dmb_exists = artifact_after.exists;
    let compiler_produced_artifact = compiler_succeeded && dmb_exists;
    let dmb_updated = artifact_after.exists
        && (!artifact_before.exists
            || (artifact_after.sha256.is_some() && artifact_before.sha256 != artifact_after.sha256)
            || artifact_before.size != artifact_after.size
            || artifact_before.modified_unix_ms != artifact_after.modified_unix_ms);
    essential.outputs.push(crate::result::OutputMutation {
        request_index: 0,
        installed: dmb_updated,
        path_preview: Some(crate::result::path_preview(&artifact_after.path)),
        sha256: artifact_after.sha256.clone(),
        cleanup_complete: true,
        backup_preview: None,
    });
    let timed_out = execution.termination == TerminationReason::WallTimeout;
    let idle = execution.termination == TerminationReason::IdleTimeout;
    let provenance = record_compile_provenance(
        &context,
        attempt,
        &prepared,
        fixture.as_ref(),
        &path,
        &dmb_path,
        &artifact_after,
        compiler_produced_artifact,
        dmb_updated,
        deadline,
        if process_succeeded && !diagnostics.complete {
            "diagnostic_analysis_incomplete"
        } else if compiler_succeeded && !dmb_exists {
            "artifact_missing"
        } else if timed_out {
            "compile_timed_out"
        } else if idle {
            "compile_idle_timed_out"
        } else if execution.termination == TerminationReason::Cancelled {
            "compile_cancelled"
        } else {
            "compiler_failed"
        },
    )
    .map_err(|error| crate::result::OutcomeError {
        error,
        outcome: essential.clone(),
    })?;

    let interruption = provenance["interruption"].as_str();
    let success = compiler_produced_artifact && interruption.is_none();
    let termination = match interruption {
        Some("request_cancelled") => TerminationReason::Cancelled,
        Some("request_timed_out") => TerminationReason::WallTimeout,
        _ => execution.termination,
    };
    essential.operation_succeeded = Some(success);
    essential.provenance_status = provenance["status"].as_str().map(str::to_owned);
    context.progress(crate::request::Stage::Publication);
    lease
        .finish()
        .map_err(|error| crate::result::OutcomeError {
            error,
            outcome: essential.clone(),
        })?;
    essential.cleanup_complete = true;
    essential.recovery_required = false;
    let result = json!({
        "success": success,
        "compiler_succeeded": compiler_succeeded,
        "diagnostic_analysis_error": (!diagnostics.complete).then_some("Compiler output could not be fully analyzed; inspect oversized_lines and output_complete in diagnostic_summary."),
        "artifact_error": (compiler_succeeded && !dmb_exists).then_some("Compiler exited successfully without producing a DMB."),
        "timed_out": timed_out || interruption == Some("request_timed_out"),
        "idle": idle,
        "termination": termination,
        "process_termination": execution.termination,
        "finalization_interruption": interruption,
        "duration_ms": execution.duration_ms,
        "timeout_ms": timeout_ms,
        "idle_timeout_ms": idle_timeout_ms,
        "exit_code": execution.exit_code,
        "dme_argument": dme_argument,
        "spawn_working_directory": compiler_working_directory.display().to_string(),
        "dmb_exists": dmb_exists,
        "dmb_updated": dmb_updated,
        "dmb_path": if dmb_exists { Some(artifact_after.path.display().to_string()) } else { None },
        "artifact_before": artifact_before,
        "artifact_after": artifact_after,
        "compiler": compiler.display().to_string(),
        "working_directory": working_directory.map(|directory| directory.display().to_string()),
        "defines": defines,
        "errors": diagnostics.errors,
        "warnings": diagnostics.warnings,
        "diagnostic_summary": diagnostics.summary,
        "stdout": stdout,
        "stderr": stderr,
        "stdout_truncated_bytes": execution.stdout.truncated_bytes,
        "stderr_truncated_bytes": execution.stderr.truncated_bytes,
        "network_audit": execution.network_audit,
        "provenance_status": provenance["status"],
        "build_record_id": provenance["record_id"],
        "attempt_id": provenance["attempt_id"],
        "provenance_reasons": provenance["reasons"],
        "retained_dmb_sha256": provenance["retained_dmb_sha256"],
    });

    let data: crate::outputs::CompileData =
        serde_json::from_value(result).map_err(|error| crate::result::OutcomeError {
            error: error.into(),
            outcome: essential.clone(),
        })?;
    let data =
        response::project_compile(data, response).map_err(|error| crate::result::OutcomeError {
            error,
            outcome: essential.clone(),
        })?;
    Ok(crate::result::projection(data, false, !success).with_outcome(essential))
    }).await
}

#[allow(clippy::too_many_arguments)]
fn record_compile_provenance(
    context: &ToolExecutionContext,
    attempt: Option<BuildAttempt>,
    prepared: &PreparedBuild,
    fixture: Option<&VerifiedFixtureManifest>,
    dme_path: &Path,
    dmb_path: &Path,
    artifact_after: &ArtifactSnapshot,
    success: bool,
    dmb_updated: bool,
    deadline: tokio::time::Instant,
    failure_code: &str,
) -> Result<Value> {
    let Some(store) = context.build_provenance() else {
        return Ok(json!({
            "status": "unverified",
            "record_id": null,
            "reasons": [{"code": "private_state_unavailable"}],
            "retained_dmb_sha256": artifact_after.sha256,
        }));
    };
    let mut attempt = attempt.ok_or_else(|| anyhow!("managed build has no durable attempt"))?;
    let mut inputs = prepared.inputs.clone();
    let rsc_path = fixture
        .and_then(|fixture| fixture.rsc_path.clone())
        .unwrap_or_else(|| dme_path.with_extension("rsc"));
    let checkpoint = || context.finalization_reason(deadline);
    let mut verification_reason = prepared.finish_reason_checked(checkpoint).or_else(|| {
        (fixture.is_some_and(|fixture| fixture.rsc_path.is_some()) && !rsc_path.is_file())
            .then_some("required_rsc_missing")
    });
    let artifact_key = store.artifact_key(dmb_path)?;
    let created_at_unix_ms = unix_ms();

    let identities =
        if success && dmb_updated && verification_reason.is_none() && artifact_after.exists {
            let captured = (|| -> Result<_> {
                let dmb = FileIdentity::capture_checked(dmb_path, || context.checkpoint())?;
                let rsc = rsc_path
                    .exists()
                    .then(|| FileIdentity::capture_checked(&rsc_path, || context.checkpoint()))
                    .transpose()?;
                Ok((dmb, rsc))
            })();
            match captured {
                Ok(identities) => Some(identities),
                Err(_) if checkpoint().is_some() => {
                    verification_reason = checkpoint();
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
    if let Some((dmb, rsc)) = identities {
        inputs.sort_by(|left, right| {
            (&left.role, &left.relative_path).cmp(&(&right.role, &right.relative_path))
        });
        let record_id = random_id()?;
        let record = BuildRecord {
            schema: 2,
            record_id: record_id.clone(),
            artifact_key: artifact_key.clone(),
            mcp_build: crate::build_identity::current().clone(),
            compiler: prepared.compiler.clone(),
            project: store.project_identity(dmb_path)?,
            inputs: inputs.clone(),
            verification: Some(prepared.verification.clone()),
            dmb,
            rsc,
            fixture_manifest_sha256: fixture.map(|fixture| fixture.identity_sha256.clone()),
            created_at_unix_ms,
        };
        attempt.outcome = BuildAttemptOutcome::Succeeded;
        attempt.observed_inputs = inputs;
        attempt.retained_dmb_sha256 = artifact_after.sha256.clone();
        let interruption = store.finish_attempt_checked(&attempt, Some(&record), checkpoint)?;
        if let Some(code) = interruption {
            return compile_provenance_decision(
                context,
                store,
                dmb_path,
                &attempt,
                artifact_after,
                code,
                interruption,
            );
        }
        return Ok(json!({
            "status": "verified",
            "attempt_id": attempt.attempt_id,
            "record_id": record_id,
            "reasons": [],
            "retained_dmb_sha256": artifact_after.sha256,
        }));
    }

    if artifact_after.exists || fixture.is_some() || !success {
        let code = if success {
            verification_reason.unwrap_or("artifact_not_fresh")
        } else {
            failure_code
        };
        attempt.outcome = if success {
            BuildAttemptOutcome::Unverified {
                code: code.to_owned(),
            }
        } else {
            BuildAttemptOutcome::Failed {
                code: code.to_owned(),
            }
        };
        attempt.observed_inputs = inputs;
        attempt.retained_dmb_sha256 = artifact_after.sha256.clone();
        let interruption = store.finish_attempt_checked(&attempt, None, checkpoint)?;
        return compile_provenance_decision(
            context,
            store,
            dmb_path,
            &attempt,
            artifact_after,
            interruption.unwrap_or(code),
            interruption,
        );
    }

    Ok(json!({
        "status": "unverified",
        "record_id": null,
        "reasons": [{"code": verification_reason.unwrap_or("artifact_not_fresh")}],
        "retained_dmb_sha256": artifact_after.sha256,
    }))
}

fn compile_provenance_decision(
    context: &ToolExecutionContext,
    store: &crate::BuildProvenanceStore,
    dmb_path: &Path,
    attempt: &BuildAttempt,
    artifact_after: &ArtifactSnapshot,
    code: &str,
    interruption: Option<&str>,
) -> Result<Value> {
    let evaluated = store.evaluate_launch_checked(dmb_path, false, || context.checkpoint());
    let interruption =
        interruption.or_else(|| context.finalization_reason(context.total_deadline()));
    let mut decision = match evaluated {
        Ok(decision) => decision,
        Err(_) if interruption.is_some() => {
            return Ok(json!({
                "status":"unverified", "record_id":null, "attempt_id":attempt.attempt_id,
                "reasons":[{"code":interruption}], "retained_dmb_sha256":artifact_after.sha256,
                "interruption":interruption,
            }))
        }
        Err(error) => return Err(error),
    };
    decision
        .reasons
        .push(crate::build_provenance::ProvenanceReason {
            code: code.to_owned(),
            message: "the build did not establish complete stable compiler input evidence"
                .to_owned(),
            role: None,
            path: None,
        });
    Ok(json!({
        "status": match decision.status {
            ProvenanceStatus::Verified => "verified",
            ProvenanceStatus::Unverified => "unverified",
            ProvenanceStatus::Stale => "stale",
        },
        "record_id": decision.record_id,
        "attempt_id": attempt.attempt_id,
        "reasons": decision.reasons,
        "retained_dmb_sha256": artifact_after.sha256,
        "interruption": interruption,
    }))
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
