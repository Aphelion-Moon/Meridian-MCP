mod output;

use super::{build_response as response, ToolExecutionContext};
use crate::artifact::ArtifactSnapshot;
use crate::build_provenance::{
    BuildAttempt, BuildAttemptOutcome, BuildInputIdentity, PreparedBuild,
};
use crate::mcp::ToolResult;
use crate::parameters::{RiftCompileParams, RiftNetworkMode};
use crate::process::{
    run_contained_process_observed, ProcessOutcome, ProcessSpec, TerminationReason,
};
use crate::state::ServerState;
use crate::{ProjectProfile, RiftBuildAccess};
#[cfg(windows)]
use anyhow::Context;
use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildEvidence {
    FreshArtifacts,
    ValidCacheHit,
    BuildFailed,
    InsufficientEvidence,
}

struct ValidatedProject {
    root: PathBuf,
    dme: PathBuf,
    human_build: PathBuf,
    rift_build: PathBuf,
    dmb: PathBuf,
    rsc: PathBuf,
}

struct ArtifactPair {
    dmb: ArtifactSnapshot,
    rsc: ArtifactSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ControllerTimeoutPolicy {
    inner_wall_seconds: u64,
    inner_idle_seconds: u64,
    outer_idle_timeout: Duration,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RiftResultArtifact {
    path: String,
    size: u64,
    sha256: String,
    freshness: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RiftResultRecord {
    schema_version: u64,
    run_id: String,
    command: String,
    status: String,
    evidence: String,
    exit_code: i64,
    reused: Option<bool>,
    artifacts: Vec<RiftResultArtifact>,
}

pub async fn compile(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let response = match response::ResponseOptions::parse(&args) {
        Ok(options) => options,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "invalid_arguments",
                error.to_string(),
                "Use output controls within the advertised types and bounds.",
            ))
        }
    };
    let params: RiftCompileParams = match serde_json::from_value(args) {
        Ok(params) => params,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "invalid_arguments",
                format!("invalid rift_compile arguments: {error}"),
                "Use only the fields advertised by the rift_compile schema.",
            ));
        }
    };
    let (timeout_ms, idle_timeout_ms) = match params.validated_timeouts() {
        Ok(timeouts) => timeouts,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "invalid_arguments",
                error,
                "Use timeout values within the advertised bounds.",
            ));
        }
    };
    if params.network_mode() == RiftNetworkMode::Allow
        && context.rift_build_access() != RiftBuildAccess::Network
    {
        return Ok(ToolResult::structured_error(
            "network_mode_denied",
            "network_mode=allow exceeds the immutable startup ceiling",
            "Restart Meridian-MCP with MERIDIAN_MCP_RIFT_BUILD=network only when network bootstrap is explicitly approved.",
        ));
    }

    let Ok(snapshot) = state.snapshot().await else {
        return Ok(ToolResult::structured_error(
            "project_not_parsed",
            "No parsed project profile is active.",
            "Call dm_parse_environment for the contained Meridian-Rift tgstation.dme first.",
        ));
    };
    let generation = snapshot.generation;
    let Some(profile) = snapshot.project_profile.clone() else {
        return Ok(ToolResult::structured_error(
            "project_not_parsed",
            "No parsed project profile is active.",
            "Call dm_parse_environment for the contained Meridian-Rift tgstation.dme first.",
        ));
    };
    if !profile.is_rift_build_qualified() {
        return Ok(ToolResult::structured_error(
            "project_not_qualified",
            "The active parsed project does not qualify for the Meridian-Rift full build.",
            "Use a contained tgstation.dme with root BUILD.cmd, RIFT_BUILD.cmd, and literal BYOND pins.",
        ));
    }

    let compiler = match context.policy().compiler_allowlist() {
        [] => {
            return Ok(ToolResult::structured_error(
                "compiler_not_configured",
                "rift_compile requires exactly one startup-allowlisted DreamMaker compiler.",
                "Restart Meridian-MCP with the intended dm.exe as the sole MERIDIAN_MCP_COMPILERS entry.",
            ));
        }
        [compiler] => compiler.clone(),
        _ => {
            return Ok(ToolResult::structured_error(
                "compiler_ambiguous",
                "rift_compile cannot select among multiple startup-allowlisted compilers.",
                "Restart Meridian-MCP with exactly one DreamMaker compiler for this compatibility run.",
            ));
        }
    };

    let project = match validate_project(context, &profile) {
        Ok(project) => project,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "project_not_qualified",
                error.to_string(),
                "Reparse the contained project after restoring its fixed root entry points.",
            ));
        }
    };
    let fixture = match params.fixture_manifest_path.as_deref() {
        Some(path) => Some(super::fixture::load_manifest(context, state, path).await),
        None => None,
    };
    let fixture = match fixture.transpose() {
        Ok(fixture) => fixture,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "fixture_manifest_invalid",
                error.to_string(),
                "Correct the contained declarative fixture manifest and retry.",
            ));
        }
    };
    if fixture.as_ref().is_some_and(|fixture| {
        fixture.dme_path != project.dme
            || fixture.dmb_path != project.dmb
            || fixture.rsc_path.as_ref() != Some(&project.rsc)
    }) {
        return Ok(ToolResult::structured_error(
            "fixture_manifest_mismatch",
            "fixture manifest paths do not match the qualified Meridian-Rift build outputs",
            "Select the exact manifest for this contained project.",
        ));
    }
    if state.state_generation().await != generation {
        return Ok(ToolResult::structured_error(
            "state_generation_changed",
            "The active parsed project changed during build preparation.",
            "Retry rift_compile against the current parsed generation.",
        ));
    }

    let before = match capture_artifacts(&project) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "insufficient_evidence",
                error.to_string(),
                "Restore canonical contained artifact paths and retry.",
            ));
        }
    };
    let command_processor = match system_command_processor() {
        Ok(command_processor) => command_processor,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "build_spawn_failed",
                format!("cannot resolve the trusted system command processor: {error}"),
                "Verify the Windows system directory and retry without changing the fixed wrapper path.",
            ));
        }
    };
    let command_processor = command_path(&command_processor);
    let timeout_policy = controller_timeout_policy(timeout_ms, idle_timeout_ms);
    let (environment, mut warnings) = build_environment(
        context,
        &command_processor,
        &compiler,
        params.network_mode(),
        params.force_rebuild,
        timeout_policy,
    );
    let script_argument = match cmd_script_argument(&project.rift_build) {
        Ok(argument) => argument,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "project_not_qualified",
                error.to_string(),
                "Move the checkout to a path without Windows command metacharacters and reparse it.",
            ));
        }
    };
    let mut prepared = PreparedBuild::capture(
        context.policy(),
        Some(&snapshot),
        fixture.as_ref(),
        &project.dme,
        &compiler,
        vec![
            "/D".to_owned(),
            "/S".to_owned(),
            "/C".to_owned(),
            "call".to_owned(),
            script_argument.to_string_lossy().into_owned(),
        ],
        command_path(&project.root),
    )?;
    prepared.reason = Some("rift_compiler_closure_not_proved");
    for path in [&project.rift_build, &project.human_build] {
        prepared.inputs.push(BuildInputIdentity::capture_authorized(
            context.policy(),
            &project.root,
            path,
            "build_entrypoint",
        )?);
    }
    let mut output = output::BuildOutput::new(response.diagnostic_limit());
    let attempt = context
        .build_provenance()
        .map(|store| store.begin_attempt(&project.dmb, prepared.inputs.clone()))
        .transpose()?;
    let outcome = match run_contained_process_observed(
        ProcessSpec {
            program: command_processor,
            arguments: vec![
                "/D".into(),
                "/S".into(),
                "/C".into(),
                "call".into(),
                script_argument,
            ],
            working_directory: command_path(&project.root),
            environment,
            stdin: None,
            timeout: Duration::from_millis(timeout_ms),
            idle_timeout: timeout_policy.outer_idle_timeout,
            capture_network: params.capture_network,
            cancellation: None,
        },
        |stream, bytes| output.observe(stream, bytes),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "build_spawn_failed",
                format!("contained process setup failed: {error}"),
                "Check Windows Job Object support and the fixed RIFT_BUILD.cmd entry point.",
            ));
        }
    };
    if state.state_generation().await != generation {
        return Ok(ToolResult::structured_error(
            "state_generation_changed",
            "The active parsed project changed while the full build was running.",
            "Reparse and retry against a stable project generation.",
        ));
    }

    if let Some(warning) = outcome.network_audit.warning.clone() {
        warnings.push(warning);
    }
    let after = match capture_artifacts(&project) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            return Ok(ToolResult::structured_error(
                "insufficient_evidence",
                error.to_string(),
                "Restore canonical contained build artifacts and rerun the full build.",
            ));
        }
    };
    let analysis = output.finish(
        outcome.output_complete,
        outcome.stdout.truncated_bytes == 0 && outcome.stderr.truncated_bytes == 0,
    );
    classify_result(
        context,
        &prepared,
        attempt,
        &profile,
        &project,
        generation,
        &params,
        timeout_ms,
        idle_timeout_ms,
        timeout_policy,
        before,
        after,
        outcome,
        analysis,
        response,
        warnings,
    )
}

fn validate_project(
    context: &ToolExecutionContext,
    profile: &ProjectProfile,
) -> Result<ValidatedProject> {
    let policy = context.policy();
    let root = policy.read_path(profile.root())?;
    if root != profile.root() {
        return Err(anyhow!("the canonical project root changed after parsing"));
    }
    let dme = policy.read_path(profile.dme_path())?;
    let human_build = policy.read_path(
        profile
            .human_build_entrypoint()
            .ok_or_else(|| anyhow!("BUILD.cmd is missing"))?,
    )?;
    let rift_build = policy.read_path(
        profile
            .rift_build_entrypoint()
            .ok_or_else(|| anyhow!("RIFT_BUILD.cmd is missing"))?,
    )?;
    require_direct_root_file(&root, &human_build, "BUILD.cmd")?;
    require_direct_root_file(&root, &rift_build, "RIFT_BUILD.cmd")?;
    let dmb = policy.output_path(root.join("tgstation.dmb"), true)?;
    let rsc = policy.output_path(root.join("tgstation.rsc"), true)?;
    Ok(ValidatedProject {
        root,
        dme,
        human_build,
        rift_build,
        dmb,
        rsc,
    })
}

fn require_direct_root_file(root: &Path, path: &Path, expected_name: &str) -> Result<()> {
    if path.parent() != Some(root) || path.file_name() != Some(OsStr::new(expected_name)) {
        return Err(anyhow!(
            "{expected_name} is not the canonical contained root entry point"
        ));
    }
    Ok(())
}

fn capture_artifacts(project: &ValidatedProject) -> Result<ArtifactPair> {
    Ok(ArtifactPair {
        dmb: ArtifactSnapshot::capture(&project.root, &project.dmb)?,
        rsc: ArtifactSnapshot::capture(&project.root, &project.rsc)?,
    })
}

fn build_environment(
    context: &ToolExecutionContext,
    command_processor: &Path,
    compiler: &Path,
    network_mode: RiftNetworkMode,
    force_rebuild: bool,
    timeout_policy: ControllerTimeoutPolicy,
) -> (Vec<(OsString, OsString)>, Vec<String>) {
    let mut environment = Vec::new();
    for name in [
        "SystemRoot",
        "SystemDrive",
        "WINDIR",
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
    ] {
        if let Some(value) = std::env::var_os(name) {
            environment.push((name.into(), value));
        }
    }
    environment.push(("ComSpec".into(), command_processor.as_os_str().to_owned()));
    environment.push(("DM_EXE".into(), command_path(compiler).into_os_string()));
    environment.push((
        "MERIDIAN_RIFT_BUILD_NETWORK".into(),
        match network_mode {
            RiftNetworkMode::Offline => "offline",
            RiftNetworkMode::Allow => "allow",
        }
        .into(),
    ));
    environment.push((
        "MERIDIAN_RIFT_FORCE_REBUILD".into(),
        if force_rebuild { "1" } else { "0" }.into(),
    ));
    environment.push((
        "MERIDIAN_RIFT_WALL_TIMEOUT_SECONDS".into(),
        timeout_policy.inner_wall_seconds.to_string().into(),
    ));
    environment.push((
        "MERIDIAN_RIFT_IDLE_TIMEOUT_SECONDS".into(),
        timeout_policy.inner_idle_seconds.to_string().into(),
    ));

    let mut warnings = Vec::new();
    if let Some(cache) = std::env::var_os("TG_BOOTSTRAP_CACHE") {
        match context.policy().read_path(PathBuf::from(&cache)) {
            Ok(cache) if cache.is_dir() => {
                environment.push((
                    "TG_BOOTSTRAP_CACHE".into(),
                    command_path(&cache).into_os_string(),
                ));
            }
            _ => warnings.push(
                "TG_BOOTSTRAP_CACHE was omitted because it is not a contained directory"
                    .to_string(),
            ),
        }
    }
    (environment, warnings)
}

fn controller_timeout_policy(timeout_ms: u64, idle_timeout_ms: u64) -> ControllerTimeoutPolicy {
    let outer_wall_seconds = (timeout_ms / 1_000).max(1);
    let cleanup_headroom_seconds = if outer_wall_seconds > 1 {
        (outer_wall_seconds / 10).clamp(1, 60)
    } else {
        0
    };
    ControllerTimeoutPolicy {
        inner_wall_seconds: outer_wall_seconds
            .saturating_sub(cleanup_headroom_seconds)
            .max(1),
        inner_idle_seconds: idle_timeout_ms.div_ceil(1_000).max(1),
        outer_idle_timeout: Duration::from_millis(timeout_ms),
    }
}

#[cfg(windows)]
fn system_command_processor() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err(std::io::Error::last_os_error()).context("cannot resolve system directory");
    }
    buffer.truncate(length as usize);
    let system_directory = OsString::from_wide(&buffer);
    PathBuf::from(system_directory)
        .join("cmd.exe")
        .canonicalize()
        .context("cannot canonicalize the system command processor")
}

#[cfg(not(windows))]
fn system_command_processor() -> Result<PathBuf> {
    Err(anyhow!("rift_compile is supported only on Windows"))
}

fn cmd_script_argument(script: &Path) -> Result<OsString> {
    let script = command_path(script);
    let text = script.to_string_lossy();
    if text
        .chars()
        .any(|character| "&|<>^()%!".contains(character))
    {
        return Err(anyhow!(
            "the fixed RIFT_BUILD.cmd path contains a Windows command metacharacter"
        ));
    }
    Ok(script.into_os_string())
}

fn command_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc_path) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc_path}"));
        }
        if let Some(dos_path) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(dos_path);
        }
    }
    path.to_owned()
}

#[allow(clippy::too_many_arguments)]
fn classify_result(
    context: &ToolExecutionContext,
    prepared: &PreparedBuild,
    attempt: Option<BuildAttempt>,
    profile: &ProjectProfile,
    project: &ValidatedProject,
    generation: u64,
    params: &RiftCompileParams,
    timeout_ms: u64,
    idle_timeout_ms: u64,
    timeout_policy: ControllerTimeoutPolicy,
    before: ArtifactPair,
    after: ArtifactPair,
    outcome: ProcessOutcome,
    analysis: output::Analysis,
    response: response::ResponseOptions,
    warnings: Vec<String>,
) -> Result<ToolResult> {
    let rift_result = analysis.rift_result;
    let cache_evidence = analysis.cache_evidence;
    let artifacts_valid = valid_artifact(&after.dmb) && valid_artifact(&after.rsc);
    let artifacts_changed =
        artifact_changed(&before.dmb, &after.dmb) || artifact_changed(&before.rsc, &after.rsc);

    let (evidence, failure_code) = if outcome.termination == TerminationReason::WallTimeout {
        (BuildEvidence::BuildFailed, Some("build_timed_out"))
    } else if outcome.termination == TerminationReason::IdleTimeout {
        (BuildEvidence::BuildFailed, Some("build_idle_timed_out"))
    } else if outcome.termination == TerminationReason::SpawnFailed {
        (BuildEvidence::BuildFailed, Some("build_spawn_failed"))
    } else if outcome.exit_code != Some(0) || analysis.error_count > 0 {
        (
            BuildEvidence::BuildFailed,
            analysis.wrapper_failure.or(Some("build_failed")),
        )
    } else if !analysis.complete {
        (
            BuildEvidence::InsufficientEvidence,
            Some("output_analysis_incomplete"),
        )
    } else if !artifacts_valid {
        (BuildEvidence::BuildFailed, Some("artifact_missing"))
    } else if rift_result.is_err() {
        (
            BuildEvidence::InsufficientEvidence,
            Some("wrapper_result_invalid"),
        )
    } else if let Some(result) = rift_result.as_ref().ok().and_then(Option::as_ref) {
        match validate_rift_result(result, &after, artifacts_changed, params.force_rebuild) {
            Ok(evidence) => (evidence, None),
            Err(_) => (
                BuildEvidence::InsufficientEvidence,
                Some("wrapper_result_invalid"),
            ),
        }
    } else if artifacts_changed {
        (BuildEvidence::FreshArtifacts, None)
    } else if cache_evidence.is_some() && !params.force_rebuild {
        (BuildEvidence::ValidCacheHit, None)
    } else {
        (
            BuildEvidence::InsufficientEvidence,
            Some("insufficient_evidence"),
        )
    };

    let recovery = failure_code.map(recovery_for).unwrap_or("");
    let provenance = record_rift_provenance(
        context,
        prepared,
        attempt,
        project,
        &after,
        &evidence,
        failure_code,
    )?;
    let result = json!({
        "success": failure_code.is_none(),
        "code": failure_code,
        "evidence": evidence,
        "project_root": project.root,
        "human_build_entrypoint": project.human_build,
        "rift_build_entrypoint": project.rift_build,
        "dme_path": project.dme,
        "state_generation": generation,
        "byond_version": profile.byond_version(),
        "startup_ceiling": access_name(context.rift_build_access()),
        "network_mode": network_mode_name(params.network_mode()),
        "force_rebuild": params.force_rebuild,
        "capture_network": params.capture_network,
        "timeout_ms": timeout_ms,
        "idle_timeout_ms": idle_timeout_ms,
        "controller_timeout": {
            "inner_wall_seconds": timeout_policy.inner_wall_seconds,
            "inner_idle_seconds": timeout_policy.inner_idle_seconds,
            "outer_idle_timeout_ms": timeout_policy.outer_idle_timeout.as_millis(),
        },
        "duration_ms": outcome.duration_ms,
        "termination": outcome.termination,
        "exit_code": outcome.exit_code,
        "stdout": outcome.stdout.text,
        "stderr": outcome.stderr.text,
        "stdout_truncated_bytes": outcome.stdout.truncated_bytes,
        "stderr_truncated_bytes": outcome.stderr.truncated_bytes,
        "diagnostics": analysis.diagnostics,
        "diagnostic_summary": analysis.summary,
        "cache_evidence": cache_evidence,
        "rift_result": rift_result.ok().flatten(),
        "artifact_before": {
            "dmb": before.dmb,
            "rsc": before.rsc,
        },
        "artifact_after": {
            "dmb": after.dmb,
            "rsc": after.rsc,
        },
        "network_audit": outcome.network_audit,
        "warnings": warnings,
        "recovery": recovery,
        "provenance_status": provenance["status"],
        "build_record_id": provenance["record_id"],
        "provenance_reasons": provenance["reasons"],
        "retained_dmb_sha256": provenance["retained_dmb_sha256"],
        "attempt_id": provenance["attempt_id"],
    });
    let text = response::format_rift(result, response)?;
    if failure_code.is_some() {
        Ok(ToolResult::error(text))
    } else {
        Ok(ToolResult::text(text))
    }
}

fn record_rift_provenance(
    context: &ToolExecutionContext,
    prepared: &PreparedBuild,
    attempt: Option<BuildAttempt>,
    project: &ValidatedProject,
    after: &ArtifactPair,
    evidence: &BuildEvidence,
    failure_code: Option<&str>,
) -> Result<Value> {
    let Some(store) = context.build_provenance() else {
        return Ok(json!({
            "status": "unverified",
            "record_id": null,
            "reasons": [{"code": "private_state_unavailable"}],
            "retained_dmb_sha256": after.dmb.sha256,
        }));
    };
    let mut attempt = attempt.ok_or_else(|| anyhow!("managed build has no durable attempt"))?;
    attempt.observed_inputs = prepared.inputs.clone();
    attempt.retained_dmb_sha256 = after.dmb.sha256.clone();

    if matches!(
        evidence,
        BuildEvidence::FreshArtifacts | BuildEvidence::ValidCacheHit
    ) {
        let code = prepared
            .finish_reason()
            .unwrap_or("rift_compiler_closure_not_proved");
        attempt.outcome = BuildAttemptOutcome::Unverified {
            code: code.to_owned(),
        };
        store.finish_attempt(&attempt, None)?;
        let decision = store.evaluate_launch(&project.dmb, false)?;
        return Ok(json!({
            "status": decision.status,
            "record_id": decision.record_id,
            "attempt_id": attempt.attempt_id,
            "reasons": [{"code": code}],
            "retained_dmb_sha256": after.dmb.sha256,
        }));
    }

    attempt.outcome = BuildAttemptOutcome::Failed {
        code: failure_code.unwrap_or("insufficient_evidence").to_owned(),
    };
    store.finish_attempt(&attempt, None)?;
    let decision = store.evaluate_launch(&project.dmb, false)?;
    Ok(json!({
        "status": decision.status,
        "record_id": decision.record_id,
        "attempt_id": attempt.attempt_id,
        "reasons": decision.reasons,
        "retained_dmb_sha256": after.dmb.sha256,
    }))
}

fn valid_artifact(snapshot: &ArtifactSnapshot) -> bool {
    snapshot.exists && snapshot.size.is_some_and(|size| size > 0) && snapshot.sha256.is_some()
}

fn artifact_changed(before: &ArtifactSnapshot, after: &ArtifactSnapshot) -> bool {
    after.exists
        && (!before.exists
            || before.size != after.size
            || before.modified_unix_ms != after.modified_unix_ms
            || before.sha256 != after.sha256)
}

fn parse_rift_result(output: &str) -> Result<Option<RiftResultRecord>, &'static str> {
    let mut records = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("RIFT_RESULT ").map(str::trim));
    let Some(record) = records.next() else {
        return Ok(None);
    };
    if records.next().is_some() {
        return Err("wrapper_result_multiple");
    }
    serde_json::from_str(record)
        .map(Some)
        .map_err(|_| "wrapper_result_malformed")
}

fn validate_rift_result(
    result: &RiftResultRecord,
    after: &ArtifactPair,
    artifacts_changed: bool,
    force_rebuild: bool,
) -> Result<BuildEvidence, &'static str> {
    if result.schema_version != 1
        || result.command != "compile"
        || result.status != "passed"
        || result.evidence != "full_build"
        || result.exit_code != 0
        || result.reused.is_none()
        || !Regex::new(r"^\d{8}T\d{6}Z-[0-9a-f]{8}$")
            .expect("run ID regex must be valid")
            .is_match(&result.run_id)
    {
        return Err("wrapper_result_contract_mismatch");
    }
    if result.artifacts.len() != 2 {
        return Err("wrapper_result_artifact_mismatch");
    }
    for (expected_path, snapshot) in [
        ("artifacts/tgstation.dmb", &after.dmb),
        ("artifacts/tgstation.rsc", &after.rsc),
    ] {
        let Some(artifact) = result
            .artifacts
            .iter()
            .find(|artifact| artifact.path == expected_path)
        else {
            return Err("wrapper_result_artifact_mismatch");
        };
        if artifact.size == 0
            || artifact.size != snapshot.size.unwrap_or_default()
            || snapshot.sha256.as_deref() != Some(artifact.sha256.as_str())
            || !artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || artifact.sha256.len() != 64
        {
            return Err("wrapper_result_artifact_mismatch");
        }
    }
    match result.reused {
        Some(true)
            if !force_rebuild
                && !artifacts_changed
                && result
                    .artifacts
                    .iter()
                    .all(|artifact| artifact.freshness == "reused") =>
        {
            Ok(BuildEvidence::ValidCacheHit)
        }
        Some(false)
            if artifacts_changed
                && result
                    .artifacts
                    .iter()
                    .all(|artifact| artifact.freshness == "rebuilt") =>
        {
            Ok(BuildEvidence::FreshArtifacts)
        }
        _ => Err("wrapper_result_freshness_mismatch"),
    }
}

fn dm_cache_regex() -> &'static Regex {
    static CACHE_REGEX: OnceLock<Regex> = OnceLock::new();
    CACHE_REGEX.get_or_init(|| {
        Regex::new(r#"(?i)Skipping\s+['\"]?dm['\"]?\s*\(up\s+to\s+date\)"#)
            .expect("cache marker regex must be valid")
    })
}

fn dm_cache_marker(output: &str) -> Option<String> {
    output
        .lines()
        .find(|line| dm_cache_regex().is_match(line))
        .map(str::trim)
        .map(str::to_owned)
}

fn diagnostic_regex() -> &'static Regex {
    static DIAGNOSTIC_REGEX: OnceLock<Regex> = OnceLock::new();
    DIAGNOSTIC_REGEX.get_or_init(|| {
        Regex::new(r"(?i):\d+(?::\d+)?:\s*error\b").expect("build diagnostic regex must be valid")
    })
}

fn explicit_wrapper_failure(output: &str) -> Option<&'static str> {
    ["offline_preflight_failed"]
        .into_iter()
        .find(|code| output.contains(&format!("[{code}]")))
}

fn recovery_for(code: &str) -> &'static str {
    match code {
        "output_analysis_incomplete" => {
            "Inspect the build's output source for oversized lines or incomplete pipe reads, then rerun to obtain complete build evidence."
        }
        "offline_preflight_failed" => {
            "Warm the pinned repository dependency cache using an explicitly approved network build, then retry offline."
        }
        "build_timed_out" => "Inspect bounded build output and retry with an approved timeout.",
        "build_idle_timed_out" => {
            "Inspect the last build output; verify the build is not waiting for interactive input."
        }
        "build_spawn_failed" => {
            "Verify the fixed wrapper and Windows process containment are available."
        }
        "artifact_missing" => {
            "Inspect build diagnostics and confirm both tgstation.dmb and tgstation.rsc are generated."
        }
        "wrapper_result_invalid" => {
            "Inspect the final RIFT_RESULT record and verify its versioned artifact hashes and freshness fields."
        }
        "insufficient_evidence" => {
            "Retry with force_rebuild=true or inspect why the wrapper produced neither changed artifacts nor the exact DM cache marker."
        }
        _ => "Inspect the bounded full-build output, correct the repository error, and retry.",
    }
}

fn access_name(access: RiftBuildAccess) -> &'static str {
    match access {
        RiftBuildAccess::Disabled => "disabled",
        RiftBuildAccess::Offline => "offline",
        RiftBuildAccess::Network => "network",
    }
}

fn network_mode_name(mode: RiftNetworkMode) -> &'static str {
    match mode {
        RiftNetworkMode::Offline => "offline",
        RiftNetworkMode::Allow => "allow",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_snapshot(name: &str, size: u64, sha256: &str) -> ArtifactSnapshot {
        ArtifactSnapshot {
            path: PathBuf::from(name),
            exists: true,
            size: Some(size),
            modified_unix_ms: Some(1),
            sha256: Some(sha256.to_owned()),
        }
    }

    #[test]
    fn cache_marker_is_specific_to_the_dm_target() {
        assert_eq!(
            dm_cache_marker("Skipping 'dm' (up to date)"),
            Some("Skipping 'dm' (up to date)".to_string())
        );
        assert_eq!(dm_cache_marker("Skipping 'tgui' (up to date)"), None);
    }

    #[test]
    fn versioned_result_validates_a_reused_artifact_pair() {
        let dmb_hash = "a".repeat(64);
        let rsc_hash = "b".repeat(64);
        let output = format!(
            "RIFT_RESULT {{\"schema_version\":1,\"run_id\":\"20260831T120000Z-0123abcd\",\"command\":\"compile\",\"status\":\"passed\",\"evidence\":\"full_build\",\"exit_code\":0,\"reused\":true,\"artifacts\":[{{\"path\":\"artifacts/tgstation.dmb\",\"size\":12,\"sha256\":\"{dmb_hash}\",\"freshness\":\"reused\"}},{{\"path\":\"artifacts/tgstation.rsc\",\"size\":13,\"sha256\":\"{rsc_hash}\",\"freshness\":\"reused\"}}]}}"
        );
        let after = ArtifactPair {
            dmb: artifact_snapshot("tgstation.dmb", 12, &dmb_hash),
            rsc: artifact_snapshot("tgstation.rsc", 13, &rsc_hash),
        };

        let result = parse_rift_result(&output).unwrap().unwrap();
        assert_eq!(
            validate_rift_result(&result, &after, false, false).unwrap(),
            BuildEvidence::ValidCacheHit
        );
    }

    #[test]
    fn versioned_result_rejects_mismatched_artifact_evidence() {
        let output = format!(
            "RIFT_RESULT {{\"schema_version\":1,\"run_id\":\"20260831T120000Z-0123abcd\",\"command\":\"compile\",\"status\":\"passed\",\"evidence\":\"full_build\",\"exit_code\":0,\"reused\":true,\"artifacts\":[{{\"path\":\"artifacts/tgstation.dmb\",\"size\":12,\"sha256\":\"{}\",\"freshness\":\"reused\"}},{{\"path\":\"artifacts/tgstation.rsc\",\"size\":13,\"sha256\":\"{}\",\"freshness\":\"reused\"}}]}}",
            "c".repeat(64),
            "b".repeat(64),
        );
        let after = ArtifactPair {
            dmb: artifact_snapshot("tgstation.dmb", 12, &"a".repeat(64)),
            rsc: artifact_snapshot("tgstation.rsc", 13, &"b".repeat(64)),
        };

        let result = parse_rift_result(&output).unwrap().unwrap();
        assert_eq!(
            validate_rift_result(&result, &after, false, false),
            Err("wrapper_result_artifact_mismatch")
        );
    }

    #[test]
    fn controller_timeout_policy_reserves_outer_cleanup_and_inner_idle() {
        let policy = controller_timeout_policy(1_800_000, 120_000);

        assert_eq!(policy.inner_wall_seconds, 1740);
        assert_eq!(policy.inner_idle_seconds, 120);
        assert_eq!(policy.outer_idle_timeout, Duration::from_millis(1_800_000));
        assert_eq!(
            controller_timeout_policy(2_000, 1_999).inner_idle_seconds,
            2
        );
    }

    #[cfg(windows)]
    #[test]
    fn command_paths_drop_the_windows_verbatim_prefix() {
        assert_eq!(
            command_path(Path::new(r"\\?\C:\repo\RIFT_BUILD.cmd")),
            PathBuf::from(r"C:\repo\RIFT_BUILD.cmd")
        );
        assert_eq!(
            command_path(Path::new(r"\\?\UNC\server\share\RIFT_BUILD.cmd")),
            PathBuf::from(r"\\server\share\RIFT_BUILD.cmd")
        );
    }
}
