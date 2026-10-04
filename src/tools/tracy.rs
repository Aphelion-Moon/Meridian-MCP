use super::ToolExecutionContext;
use crate::atomic_output::{write_atomic, OutputArtifact};
use crate::mcp::ToolResult;
use crate::result::{json_success, structured_error, ToolErrorCode, ToolMetadata};
use crate::tracy_artifact::{reserve_trace_set, validate_capture_result, ReservedTraceSet};
use crate::tracy_collector::{
    capture_failure_code, capture_window_started, TracyCollector, TracyCollectorSpec,
    TracySessionPhase,
};
use crate::tracy_experiment::{
    bind_workload, canonical_sha256, experiment_identity, finalize_executable,
    verify_locked_workload, ExecutableIdentity, ExperimentLaunchManifest, ExperimentState,
    HelperIdentity, NativeModuleIdentity, WorkloadInput,
};
use crate::tracy_protocol::{invoke_helper, TracyCommand, TracyInvocationSpec, TracyProtocolError};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

fn wake_client_executable(dreamdaemon: &Path) -> Result<PathBuf> {
    let parent = dreamdaemon
        .parent()
        .ok_or_else(|| anyhow!("DreamDaemon has no installation directory"))?;
    let client_name = if cfg!(windows) {
        "dreamseeker.exe"
    } else {
        "DreamSeeker"
    };
    let client = parent.join(client_name);
    if !client.is_file() {
        return Err(anyhow!(
            "DreamSeeker sibling not found beside the allowlisted DreamDaemon"
        ));
    }
    Ok(client.canonicalize()?)
}

fn spawn_wake_client(
    executable: &Path,
    working_directory: &Path,
    game_port: u16,
) -> Result<crate::state::TracyWakeClient> {
    let mut command = TokioCommand::new(executable);
    command
        .arg(crate::tracy_runtime_config::wake_client_url(game_port))
        .current_dir(working_directory)
        .env_clear()
        .envs(crate::process_environment::minimal_runtime_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let (process, containment) = crate::process::spawn_runtime_process(&mut command)?;
    Ok(crate::state::TracyWakeClient {
        process,
        containment,
    })
}

async fn stop_wake_client(client: &mut crate::state::TracyWakeClient) -> Result<()> {
    client.containment.request_termination()?;
    tokio::time::timeout(Duration::from_secs(2), client.process.wait()).await??;
    crate::process::wait_for_cleanup(&client.containment).await
}

pub async fn prepare(
    context: &ToolExecutionContext,
    args: crate::parameters::TracyPrepareParams,
) -> Result<ToolResult> {
    let installation = context
        .tracy()
        .ok_or_else(|| anyhow!("Tracy installation unavailable"))?;
    let dmb_path = Path::new(args.dmb_path.as_str());
    let parent = dmb_path
        .parent()
        .ok_or_else(|| anyhow!("dmb_path has no parent"))?;
    let hook_name = installation
        .hook
        .path
        .file_name()
        .ok_or_else(|| anyhow!("verified hook has no file name"))?;
    let destination = parent.join(hook_name);
    let overwrite = args.overwrite.unwrap_or(false);

    if destination.exists()
        && hash_file(&destination)?.eq_ignore_ascii_case(&installation.hook.sha256)
    {
        let artifact = OutputArtifact {
            path: destination.canonicalize()?,
            bytes: std::fs::metadata(&destination)?.len(),
            sha256: installation.hook.sha256.clone(),
        };
        return Ok(json_success(
            ToolMetadata::complete(None),
            json!({"state":"already_prepared","artifact":artifact,"source_revision":installation.hook.source_revision,"protocol_version":installation.hook.protocol_version}),
        ));
    }

    let source = installation.hook.path.clone();
    let artifact = write_atomic(context.policy(), &destination, overwrite, |output| {
        let mut input = std::fs::File::open(&source)?;
        std::io::copy(&mut input, output)?;
        Ok(())
    })?;
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({"state":"prepared","artifact":artifact,"source_revision":installation.hook.source_revision,"protocol_version":installation.hook.protocol_version}),
    ))
}

pub async fn launch(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyLaunchParams,
) -> Result<ToolResult> {
    let _lifecycle = state.lifecycle().await;
    let prior_journal_id = state
        .tracy_capture()
        .await
        .integrity_journal
        .as_ref()
        .map(|journal| journal.summary().journal_id);
    let result = launch_inner(context, state, args).await;
    let failed = result
        .as_ref()
        .map_or(true, |tool_result| tool_result.is_error == Some(true));
    let current_journal_id = state
        .tracy_capture()
        .await
        .integrity_journal
        .as_ref()
        .map(|journal| journal.summary().journal_id);
    let owns_lease = state
        .runtime()
        .await
        .execution_lease
        .as_ref()
        .is_some_and(|lease| {
            context
                .request_owner
                .as_ref()
                .is_some_and(|owner| lease.belongs_to(owner))
        });
    if failed
        && ((current_journal_id.is_some() && current_journal_id != prior_journal_id) || owns_lease)
    {
        let _ = stop_with_lifecycle(context, state).await;
    }
    result
}

async fn launch_inner(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyLaunchParams,
) -> Result<ToolResult> {
    let dmb_path = Path::new(args.dmb_path.as_str());
    let canonical_dmb = dmb_path.canonicalize()?;
    let game_port = u16::try_from(args.game_port.unwrap_or(1337))?;
    let readiness_timeout_ms = args.startup_timeout_ms.unwrap_or(60_000);
    let initialization_timeout_ms = args.initialization_timeout_ms.unwrap_or(180_000);
    let require_verified_provenance = args.require_verified_provenance.unwrap_or(false);
    let mut lease = match context
        .execution_lease(
            &canonical_dmb,
            canonical_dmb
                .parent()
                .ok_or_else(|| anyhow!("DMB has no parent"))?,
            "tracy",
            context.deadline(readiness_timeout_ms),
        )
        .await?
    {
        Ok(lease) => lease,
        Err(result) => return Ok(result),
    };
    let launch_provenance = match super::require_launchable_artifact(
        context,
        &canonical_dmb,
        require_verified_provenance,
    ) {
        Ok(provenance) => provenance,
        Err(result) => return Ok(result),
    };
    let installation = context
        .tracy()
        .ok_or_else(|| anyhow!("Tracy installation unavailable"))?;
    let hook_name = installation
        .hook
        .path
        .file_name()
        .ok_or_else(|| anyhow!("verified hook has no file name"))?;
    let hook_path = dmb_path
        .parent()
        .ok_or_else(|| anyhow!("dmb_path has no parent"))?
        .join(hook_name);
    if !hook_path.is_file()
        || !hash_file(&hook_path)?.eq_ignore_ascii_case(&installation.hook.sha256)
    {
        return Err(anyhow!(
            "the verified byond-tracy hook is not prepared beside the DMB; call dm_tracy_prepare"
        ));
    }
    {
        let mut runtime = state.runtime().await;
        if runtime.is_game_running() {
            return Err(anyhow!("an MCP-owned runtime is already active"));
        }
    }
    let integrity_root = git_workspace_root(
        canonical_dmb
            .parent()
            .ok_or_else(|| anyhow!("dmb_path has no parent"))?,
    )?;
    let integrity = crate::workspace_integrity::IntegrityBaseline::capture(&integrity_root)?;
    let repository_dirty_digest = integrity.digest.clone();
    let workload_draft = args.workload()?;
    let experiment_name = args.experiment_name.clone();
    if let Some(name) = &experiment_name {
        crate::tracy_experiment::validate_workload(WorkloadInput {
            external_run_id: Some(name.clone()),
            ..Default::default()
        })?;
    }
    let dreamdaemon =
        super::runtime::find_dreamdaemon_for_compilers(context.policy().compiler_allowlist())
            .ok_or_else(|| anyhow!("DreamDaemon not found. Please install BYOND."))?
            .canonicalize()?;
    let runtime_configuration = args
        .config_directory
        .clone()
        .map(|path| context.policy().read_path(path))
        .transpose()?
        .map(|path| crate::tracy_runtime_config::inspect_runtime_configuration(&path))
        .transpose()?;
    let wake_sleeping_world =
        runtime_configuration.is_some() && args.wake_sleeping_world.unwrap_or(true);
    let wake_client = wake_sleeping_world
        .then(|| wake_client_executable(&dreamdaemon))
        .transpose()?;
    let wake_client_sha256 = wake_client
        .as_ref()
        .map(|path| hash_file(path))
        .transpose()?;
    let launch_parameters_sha256 = canonical_sha256(&json!({
        "game_port": game_port,
        "startup_timeout_ms": readiness_timeout_ms,
        "profiler_transport": "loopback_ephemeral",
        "runtime_configuration": runtime_configuration.as_ref().map(|configuration| &configuration.identity),
        "wake_sleeping_world": wake_sleeping_world,
        "wake_fallback": wake_client_sha256.as_ref().map(|sha256| json!({
            "strategy":"owned_loopback_dreamseeker",
            "sha256":sha256,
        })),
        "initialization_timeout_ms": initialization_timeout_ms,
    }))?;
    let rsc_path = canonical_dmb.with_extension("rsc");
    let repository_revision = git_revision(&integrity_root);
    let dmb_sha256 = hash_file(&canonical_dmb)?;
    let rsc_sha256 = rsc_path
        .is_file()
        .then(|| hash_file(&rsc_path))
        .transpose()?;
    let byond_executable_sha256 = hash_file(&dreamdaemon)?;
    let hook_patch_sha256 = canonical_sha256(&installation.hook.patches)?;
    let experiment_directory = context
        .policy()
        .read_path(args.experiment_directory.as_str())?;
    if !experiment_directory.is_dir() {
        return Err(anyhow!(
            "experiment_directory must be an existing contained directory"
        ));
    }
    // Journal creation begins durable finalization obligations. Retain the
    // lease in shared state before the next cancellable await.
    {
        let mut runtime = state.runtime().await;
        lease.mark_writer_started();
        runtime.execution_lease = Some(lease);
    }
    let mut integrity_journal = match crate::workspace_integrity::IntegrityJournal::create(
        context.policy(),
        &experiment_directory,
        &integrity,
    ) {
        Ok(journal) => journal,
        Err(crate::workspace_integrity::IntegrityError::RecoveryRequired { last_action }) => {
            return Ok(structured_error(
                ToolErrorCode::RecoveryRequired,
                "The experiment directory contains an unfinished Tracy integrity journal.",
                Some("Inspect the recorded lifecycle state and use a new experiment directory or explicitly resolve the unfinished session before launch.".to_owned()),
                json!({"last_action": last_action}),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let launch_manifest_path = experiment_directory.join("experiment-launch.meridian.json");
    let identity_manifest_path =
        launch_manifest_path.with_file_name("experiment-identity.meridian.json");
    let final_manifest_path =
        launch_manifest_path.with_file_name("experiment-complete.meridian.json");
    let integrity_owned_paths = vec![
        canonical_dmb.with_extension("log"),
        launch_manifest_path.clone(),
        integrity_journal.path().to_owned(),
        identity_manifest_path.clone(),
        final_manifest_path.clone(),
    ];
    let pre_launch_checkpoint = integrity.checkpoint("pre_launch", &integrity_owned_paths)?;
    integrity_journal.record(context.policy(), pre_launch_checkpoint)?;
    {
        let mut capture = state.tracy_capture().await;
        capture.integrity = Some(integrity);
        capture.integrity_journal = Some(integrity_journal);
        capture.integrity_owned_paths = integrity_owned_paths;
        capture.experiment = None;
        capture.used_phases.clear();
        capture.capture_records.clear();
        capture.diagnostic_records.clear();
        capture.network_records.clear();
    }
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let profiler_port = listener.local_addr()?.port();
    drop(listener);
    let daemon_args = runtime_configuration
        .as_ref()
        .map(|configuration| vec!["-params".to_owned(), configuration.world_parameter()])
        .unwrap_or_default();
    let runtime_result = super::runtime::run_profiled_with_lifecycle(
        context,
        state,
        crate::parameters::RunParams {
            dmb_path: dmb_path.display().to_string(),
            port: Some(game_port as u64),
            require_verified_provenance: Some(require_verified_provenance),
            daemon_args: Some(daemon_args),
            working_directory: None,
            wait_for: None,
            wait_regex: None,
            startup_timeout_ms: None,
        },
        profiler_port,
    )
    .await?;
    if runtime_result.is_error == Some(true) {
        return Ok(runtime_result);
    }
    let experiment_started_at = tokio::time::Instant::now();
    let initial_process_identities = owned_process_identities(state).await;
    let (memory_series, memory_stop, memory_task) =
        start_memory_sampler(&initial_process_identities, experiment_started_at);
    {
        let mut capture = state.tracy_capture().await;
        capture.memory_series = Some(memory_series);
        capture.memory_stop = Some(memory_stop);
        capture.memory_task = Some(memory_task);
        capture.experiment_started_at = Some(experiment_started_at);
    }
    let mut runtime = state.runtime().await;
    let mut capture = state.tracy_capture().await;
    let collector = match TracyCollector::spawn(TracyCollectorSpec {
        helper: installation.helper.path.clone(),
        working_directory: dmb_path
            .parent()
            .ok_or_else(|| anyhow!("dmb_path has no parent"))?
            .to_owned(),
        environment: crate::process_environment::minimal_runtime_environment()
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
        request_timeout: Duration::from_secs(330),
    })
    .await
    {
        Ok(collector) => Arc::new(collector),
        Err(error) => {
            if let Some(lease) = runtime.execution_lease.as_mut() {
                lease.require_recovery();
            }
            return Err(error.into());
        }
    };
    capture.collector = Some(Arc::clone(&collector));
    capture.phase = Some(TracySessionPhase::CollectorConnecting);
    drop(capture);
    drop(runtime);
    let memory_series = state.tracy_capture().await.memory_series.clone();
    if let Some(memory_series) = memory_series {
        register_memory_identities(&memory_series, &owned_process_identities(state).await).await;
    }
    let readiness = match collector
        .session_start("127.0.0.1", profiler_port, readiness_timeout_ms)
        .await
    {
        Ok(readiness) => readiness,
        Err(error) => {
            let stderr_tail = collector.stderr_tail().await;
            let mut runtime = state.runtime().await;
            let _ = runtime.stop_game_process().await;
            return Err(anyhow!(
                "Tracy collector readiness failed: {error}; stderr tail: {stderr_tail:?}"
            ));
        }
    };
    let reported_byond_version = readiness["queue_health"]["byond_build"]
        .as_str()
        .ok_or_else(|| anyhow!("Tracy readiness omitted queue_health.byond_build"))?;
    let byond_version = installation.validate_byond_version(reported_byond_version)?;
    let executable = finalize_executable(ExecutableIdentity {
        schema: 1,
        executable_id: String::new(),
        repository_revision,
        repository_dirty_digest,
        dmb_sha256,
        rsc_sha256,
        byond_version,
        byond_executable_sha256,
        native_modules: vec![NativeModuleIdentity {
            name: hook_name.to_string_lossy().into_owned(),
            sha256: installation.hook.sha256.clone(),
        }],
        helper_identity: HelperIdentity {
            source_revision: installation.helper.source_revision.clone(),
            sha256: installation.helper.sha256.clone(),
            patch_sha256: installation.helper.patch_sha256.clone(),
        },
        hook_identity: HelperIdentity {
            source_revision: installation.hook.source_revision.clone(),
            sha256: installation.hook.sha256.clone(),
            patch_sha256: Some(hook_patch_sha256),
        },
        startup_mode: "tracy".to_owned(),
        launch_parameters_sha256,
        build_record_id: launch_provenance.build_record_id.clone(),
    })?;
    let launch_manifest = ExperimentLaunchManifest {
        schema: 1,
        experiment_name,
        meridian_mcp_build: crate::build_identity::current().clone(),
        executable: executable.clone(),
        workload_draft: workload_draft.clone(),
        runtime_configuration: runtime_configuration
            .as_ref()
            .map(|configuration| configuration.identity.clone()),
    };
    let launch_manifest_bytes = serde_json::to_vec_pretty(&launch_manifest)?;
    let launch_artifact = write_atomic(context.policy(), &launch_manifest_path, false, |output| {
        std::io::Write::write_all(output, &launch_manifest_bytes)?;
        std::io::Write::write_all(output, b"\n")?;
        Ok(())
    })?;
    {
        let mut capture = state.tracy_capture().await;
        capture.phase = Some(TracySessionPhase::HealthyIdle);
        capture.last_status = Some(readiness);
        capture.experiment = Some(ExperimentState {
            directory: experiment_directory,
            launch_manifest_sha256: launch_artifact.sha256.clone(),
            launch_manifest_path: launch_artifact.path,
            identity_manifest_path,
            final_manifest_path,
            executable,
            workload_draft,
            runtime_configuration: runtime_configuration
                .as_ref()
                .map(|configuration| configuration.identity.clone()),
            runtime_wake: None,
            locked_identity: None,
        });
    }
    let runtime_wake = if wake_sleeping_world {
        let initialization = super::runtime::wait_for_literal_output(
            state,
            "Initializations complete within",
            initialization_timeout_ms,
        )
        .await?;
        if initialization["matched"].as_bool() != Some(true) {
            let _ = stop_with_lifecycle(context, state).await;
            return Ok(structured_error(
                ToolErrorCode::TimedOut,
                "Meridian-Rift did not reach the fixed initialization-complete marker before the wake deadline.",
                Some("Inspect recent DreamDaemon output, correct initialization failures, and relaunch the profiling session.".to_owned()),
                json!({"initialization":initialization,"cleanup_attempted":true}),
            ));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        let address = format!("127.0.0.1:{game_port}");
        let mut attempts = Vec::new();
        let mut accepted = None;
        for attempt in 1..=3 {
            let before = collector
                .status()
                .await
                .ok()
                .and_then(|status| status["producer_progress"].as_u64())
                .unwrap_or(0);
            match super::runtime::send_topic(&address, "meridian_profiler_wake=1", 10_000).await {
                Ok(response) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let wake_observed = collector
                        .status()
                        .await
                        .ok()
                        .and_then(|status| status["producer_progress"].as_u64())
                        .unwrap_or(0);
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                    let sustained = collector
                        .status()
                        .await
                        .ok()
                        .and_then(|status| status["producer_progress"].as_u64())
                        .unwrap_or(0);
                    let continued = crate::tracy_runtime_config::sustained_scheduler_progress(
                        before,
                        wake_observed,
                        sustained,
                    );
                    attempts.push(json!({
                        "attempt":attempt,
                        "topic_processed":true,
                        "response_bytes":response.len(),
                        "producer_progress_before":before,
                        "producer_progress_after_wake":wake_observed,
                        "producer_progress_sustained":sustained,
                        "sustained":continued,
                    }));
                    if continued {
                        accepted = Some(sustained);
                        break;
                    }
                }
                Err(error) => attempts.push(json!({
                    "attempt":attempt,
                    "topic_processed":false,
                    "error":error.to_string(),
                    "producer_progress_before":before,
                    "sustained":false,
                })),
            }
        }
        let mut strategy = "post_initialization_topic";
        let mut wake_client_evidence = None;
        if accepted.is_none() {
            strategy = "owned_loopback_dreamseeker";
            let executable = wake_client
                .as_ref()
                .expect("wake client was qualified before launch");
            let mut capture = state.tracy_capture().await;
            let client = match spawn_wake_client(
                executable,
                canonical_dmb
                    .parent()
                    .expect("canonical DMB has a parent directory"),
                game_port,
            ) {
                Ok(client) => client,
                Err(error) => {
                    drop(capture);
                    if let Some(lease) = state.runtime().await.execution_lease.as_mut() {
                        lease.require_recovery();
                    }
                    let topic_processed = topic_was_processed(&attempts);
                    let runtime_wake = json!({
                        "strategy":strategy,
                        "initialization_marker":"Initializations complete within",
                        "attempts":attempts,
                        "topic_processed":topic_processed,
                        "sustained_producer_progress":false,
                        "wake_client_error":error.to_string(),
                    });
                    state
                        .tracy_capture()
                        .await
                        .experiment
                        .as_mut()
                        .expect("launch initialized experiment state")
                        .runtime_wake = Some(runtime_wake.clone());
                    let _ = stop_with_lifecycle(context, state).await;
                    return Ok(structured_error(
                        ToolErrorCode::ExternalToolFailure,
                        "DreamSeeker could not be started to keep the initialized headless world awake.",
                        Some("Verify the fixed DreamSeeker sibling installation and that the Windows session can launch a local BYOND client, then relaunch profiling.".to_owned()),
                        json!({"runtime_wake":runtime_wake,"cleanup_attempted":true}),
                    ));
                }
            };
            let client_pid = client.process.id();
            capture.wake_client = Some(client);
            drop(capture);
            let memory_series = state.tracy_capture().await.memory_series.clone();
            if let Some(memory_series) = memory_series {
                register_memory_identities(&memory_series, &owned_process_identities(state).await)
                    .await;
            }
            let before = collector
                .status()
                .await
                .ok()
                .and_then(|status| status["producer_progress"].as_u64())
                .unwrap_or(0);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            let mut observed = None;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let progress = collector
                    .status()
                    .await
                    .ok()
                    .and_then(|status| status["producer_progress"].as_u64())
                    .unwrap_or(0);
                match observed {
                    None if progress > before => observed = Some(progress),
                    Some(first) if progress > first => {
                        accepted = Some(progress);
                        break;
                    }
                    _ => {}
                }
            }
            wake_client_evidence = Some(json!({
                "pid":client_pid,
                "sha256":wake_client_sha256,
                "producer_progress_before":before,
                "producer_progress_after_connect":observed,
                "producer_progress_sustained":accepted,
                "sustained":accepted.is_some(),
                "timeout_ms":60_000,
            }));
        }
        let Some(producer_progress) = accepted else {
            let topic_processed = topic_was_processed(&attempts);
            let runtime_wake = json!({
                "strategy":strategy,
                "initialization_marker":"Initializations complete within",
                "initialization_timeout_ms":initialization_timeout_ms,
                "settle_ms":5_000,
                "attempts":attempts,
                "wake_client":wake_client_evidence,
                "topic_processed":topic_processed,
                "sustained_producer_progress":false,
            });
            state
                .tracy_capture()
                .await
                .experiment
                .as_mut()
                .expect("launch initialized experiment state")
                .runtime_wake = Some(runtime_wake.clone());
            let _ = stop_with_lifecycle(context, state).await;
            return Ok(structured_error(
                ToolErrorCode::ExternalToolFailure,
                "DreamDaemon did not sustain producer progress after the bounded post-initialization wake sequence.",
                Some("Inspect runtime_wake evidence for the Topic attempts and owned DreamSeeker connection, then correct the BYOND runtime or configuration before relaunching.".to_owned()),
                json!({"runtime_wake":runtime_wake,"cleanup_attempted":true}),
            ));
        };
        let topic_processed = topic_was_processed(&attempts);
        Some(json!({
            "strategy":strategy,
            "initialization_marker":"Initializations complete within",
            "initialization_timeout_ms":initialization_timeout_ms,
            "settle_ms":5_000,
            "attempts":attempts,
            "wake_client":wake_client_evidence,
            "topic_processed":topic_processed,
            "sustained_producer_progress":true,
            "producer_progress":producer_progress,
        }))
    } else {
        None
    };
    state
        .tracy_capture()
        .await
        .experiment
        .as_mut()
        .expect("launch initialized experiment state")
        .runtime_wake = runtime_wake.clone();
    let process_identities = owned_process_identities(state).await;
    {
        let mut capture = state.tracy_capture().await;
        let mut audit = crate::network_audit::NetworkAuditCollector::new(true);
        let process_ids = process_identities
            .iter()
            .map(|identity| identity.pid)
            .collect::<Vec<_>>();
        audit.sample(&process_ids, 0);
        capture.network_records.push(json!({
            "lifecycle": "launch",
            "evidence": crate::network_audit::tracy_network_evidence(
                audit.finish(),
                profiler_port,
                &process_identities,
                true,
                true,
            ),
        }));
    }
    let integrity_checkpoint = checkpoint_integrity(context, state, "post_launch").await?;
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({
            "lifecycle":"ready",
            "profiler_port":profiler_port,
            "collector":state.tracy_capture().await.last_status,
            "executable_identity":state.tracy_capture().await.experiment.as_ref().map(|experiment| &experiment.executable),
            "runtime_configuration":state.tracy_capture().await.experiment.as_ref().and_then(|experiment| experiment.runtime_configuration.as_ref()),
            "runtime_wake":runtime_wake,
            "integrity_checkpoint":integrity_checkpoint,
            "integrity_journal":state.tracy_capture().await.integrity_journal.as_ref().map(crate::workspace_integrity::IntegrityJournal::summary),
            "launch_provenance":launch_provenance,
        }),
    ))
}

pub async fn capture(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyCaptureParams,
) -> Result<ToolResult> {
    let _publication = state
        .try_tracy_publication()
        .map_err(|_| anyhow!("a Tracy capture or its finalization is already active"))?;
    let installation = context
        .tracy()
        .ok_or_else(|| anyhow!("Tracy installation unavailable"))?;
    let collector = {
        let mut runtime = state.runtime().await;
        if !runtime.is_game_running() || runtime.kind != Some(crate::state::RuntimeKind::Tracy) {
            return Err(anyhow!("no MCP-owned Tracy runtime is active"));
        }
        drop(runtime);
        state
            .tracy_capture()
            .await
            .collector
            .clone()
            .ok_or_else(|| anyhow!("active Tracy runtime has no collector"))?
    };
    let duration_ms = args.duration_ms;
    let memory_limit_mb = args.memory_limit_mb;
    if !(1..=300_000).contains(&duration_ms) || !(16..=4096).contains(&memory_limit_mb) {
        return Err(anyhow!(
            "capture duration or memory limit is outside the permitted range"
        ));
    }
    let phase = args.phase.as_str().to_owned();
    if !valid_phase(&phase) {
        return Err(anyhow!(
            "phase must contain 1-64 lowercase ASCII letters, digits, underscore, or hyphen"
        ));
    }
    let phase_iteration = args.phase_iteration;
    let phase_iteration = u32::try_from(phase_iteration)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("phase_iteration must be between 1 and 4294967295"))?;
    let supplied_workload = args.workload()?;
    let capture_annotations = args.capture_annotations.clone().unwrap_or_default();
    let capture_annotations = crate::tracy_experiment::validate_workload(WorkloadInput {
        annotations: capture_annotations,
        ..Default::default()
    })?
    .annotations;
    let output_path = Path::new(args.output_path.as_str());
    let overwrite = args.overwrite.unwrap_or(false);
    let capture_network = args.capture_network.unwrap_or(false);
    let mut network_audit = crate::network_audit::NetworkAuditCollector::new(capture_network);
    let process_identities = owned_process_identities(state).await;
    let profiler_port = {
        let runtime = state.runtime().await;
        runtime.profiler_port
    };
    let owned_process_ids = process_identities
        .iter()
        .map(|identity| identity.pid)
        .collect::<Vec<_>>();
    network_audit.sample(&owned_process_ids, 0);
    let (
        experiment,
        launch_manifest_sha256,
        experiment_manifest_sha256,
        experiment_directory,
        runtime_wake,
    ) = {
        let mut capture = state.tracy_capture().await;
        let (result, new_owned_path) = {
            let experiment = capture
                .experiment
                .as_mut()
                .ok_or_else(|| anyhow!("active Tracy runtime has no experiment identity"))?;
            let mut new_owned_path = None;
            if let Some(locked) = &experiment.locked_identity {
                verify_locked_workload(&locked.workload, &supplied_workload)?;
            } else {
                let workload = bind_workload(&experiment.workload_draft, &supplied_workload)?;
                let identity = experiment_identity(experiment.executable.clone(), workload)?;
                let identity_bytes = serde_json::to_vec_pretty(&identity)?;
                let artifact = write_atomic(
                    context.policy(),
                    &experiment.identity_manifest_path,
                    false,
                    |output| {
                        std::io::Write::write_all(output, &identity_bytes)?;
                        std::io::Write::write_all(output, b"\n")?;
                        Ok(())
                    },
                )?;
                new_owned_path = Some(artifact.path);
                experiment.locked_identity = Some(identity);
            }
            let identity = experiment
                .locked_identity
                .clone()
                .expect("identity was locked");
            let manifest_sha256 = hash_file(&experiment.identity_manifest_path)?;
            (
                (
                    identity,
                    experiment.launch_manifest_sha256.clone(),
                    manifest_sha256,
                    experiment.directory.clone(),
                    experiment.runtime_wake.clone(),
                ),
                new_owned_path,
            )
        };
        if let Some(path) = new_owned_path {
            capture.integrity_owned_paths.push(path);
        }
        result
    };
    let _ = checkpoint_integrity(context, state, "pre_capture").await?;
    let reserved = reserve_trace_set(context.policy(), output_path, overwrite)?;
    let temporary_path = reserved.temporary_trace_path().to_owned();
    {
        let mut capture = state.tracy_capture().await;
        match capture.begin_capture(&phase, phase_iteration) {
            Ok(()) => {}
            Err(crate::state::TracyCaptureStartError::Active) => {
                return Err(anyhow!("a Tracy capture is already active"));
            }
            Err(crate::state::TracyCaptureStartError::PhaseAlreadyUsed) => {
                return Err(anyhow!(
                    "phase and phase_iteration must be unique within one experiment"
                ));
            }
        }
        capture.output_path = Some(output_path.to_owned());
        capture.last_error = None;
        capture.phase = Some(TracySessionPhase::CaptureActive);
    }
    let capture_begin_ms = state
        .tracy_capture()
        .await
        .experiment_started_at
        .map(|started| started.elapsed().as_millis() as u64)
        .unwrap_or(0);
    let invocation = collector
        .capture_window(
            duration_ms,
            memory_limit_mb,
            &temporary_path,
            &phase,
            phase_iteration,
        )
        .await;
    let capture_end_ms = state
        .tracy_capture()
        .await
        .experiment_started_at
        .map(|started| started.elapsed().as_millis() as u64)
        .unwrap_or(capture_begin_ms.saturating_add(duration_ms));
    tokio::time::sleep(Duration::from_millis(550)).await;
    let mut memory_series = if let Some(series) = state.tracy_capture().await.memory_series.clone()
    {
        capture_memory_window(&series.lock().await, capture_begin_ms, capture_end_ms)
    } else {
        Vec::new()
    };
    network_audit.sample(&owned_process_ids, duration_ms as u128);
    let network_audit = crate::network_audit::tracy_network_evidence(
        network_audit.finish(),
        profiler_port.ok_or_else(|| anyhow!("profiled runtime omitted its listener port"))?,
        &process_identities,
        true,
        true,
    );
    let integrity_journal = state
        .tracy_capture()
        .await
        .integrity_journal
        .as_ref()
        .map(crate::workspace_integrity::IntegrityJournal::summary);
    let invocation = match invocation {
        Ok(invocation) => {
            let mut capture = state.tracy_capture().await;
            capture.finish_capture(&phase, phase_iteration, true);
            capture.phase = Some(TracySessionPhase::HealthyIdle);
            invocation
        }
        Err(TracyProtocolError::Helper {
            code,
            message,
            details,
        }) if code == "invalid_capture" => {
            {
                let mut capture = state.tracy_capture().await;
                capture.finish_capture(&phase, phase_iteration, true);
                capture.phase = Some(if details["collector_recovered"].as_bool() == Some(false) {
                    TracySessionPhase::RecoveryRequired
                } else {
                    TracySessionPhase::HealthyIdle
                });
                capture.last_error = Some(message.clone());
            }
            let diagnostic = retain_invalid_capture(
                context,
                reserved,
                InvalidCaptureContext {
                    experiment_directory: &experiment_directory,
                    experiment: &experiment,
                    phase: &phase,
                    phase_iteration,
                    integrity_journal: &integrity_journal,
                },
                details.clone(),
            )?;
            let integrity_checkpoint = record_invalid_capture(
                context,
                state,
                &diagnostic,
                &phase,
                phase_iteration,
                &details,
            )
            .await?;
            return Ok(structured_error(
                ToolErrorCode::InvalidCapture,
                message,
                Some("Inspect the retained diagnostic trace and repeat the phase with a new phase_iteration after correcting the reported invariants.".to_owned()),
                json!({
                    "validation": details,
                    "diagnostic": diagnostic,
                    "phase": phase,
                    "phase_iteration": phase_iteration,
                    "window_started": true,
                    "integrity_checkpoint": integrity_checkpoint,
                }),
            ));
        }
        Err(error) => {
            let stderr_tail = collector.stderr_tail().await;
            let window_started = capture_window_started(&error);
            let collector_recovered = match &error {
                TracyProtocolError::Helper { details, .. } => {
                    details["collector_recovered"].as_bool().unwrap_or(false)
                }
                _ => false,
            };
            let details = match &error {
                TracyProtocolError::Helper { details, .. } => details.clone(),
                _ => Value::Null,
            };
            let mut capture = state.tracy_capture().await;
            capture.finish_capture(&phase, phase_iteration, window_started);
            capture.phase = Some(if collector_recovered {
                TracySessionPhase::HealthyIdle
            } else {
                TracySessionPhase::RecoveryRequired
            });
            capture.last_error = Some(if stderr_tail.is_empty() {
                error.to_string()
            } else {
                format!("{error}; collector stderr tail: {stderr_tail:?}")
            });
            drop(capture);
            return Ok(structured_error(
                capture_failure_code(&error),
                error.to_string(),
                Some(if collector_recovered {
                    "Retry the same phase_iteration only when window_started is false; otherwise use the next iteration.".to_owned()
                } else {
                    "Stop the profiling session, inspect collector status and diagnostics, then relaunch before another capture.".to_owned()
                }),
                json!({
                    "helper_details": details,
                    "collector_stderr_tail": stderr_tail,
                    "phase": phase,
                    "phase_iteration": phase_iteration,
                    "window_started": window_started,
                    "collector_recovered": collector_recovered,
                }),
            ));
        }
    };
    if let Err(error_codes) = validate_capture_result(&invocation) {
        let validation = invocation["validation"].clone();
        let diagnostic = retain_invalid_capture(
            context,
            reserved,
            InvalidCaptureContext {
                experiment_directory: &experiment_directory,
                experiment: &experiment,
                phase: &phase,
                phase_iteration,
                integrity_journal: &integrity_journal,
            },
            validation.clone(),
        )?;
        let integrity_checkpoint = record_invalid_capture(
            context,
            state,
            &diagnostic,
            &phase,
            phase_iteration,
            &validation,
        )
        .await?;
        return Ok(structured_error(
            ToolErrorCode::InvalidCapture,
            "The collector returned a capture that failed the MCP publication contract.",
            Some("Inspect the retained diagnostic trace and correct the reported capture invariants before collecting another iteration.".to_owned()),
            json!({
                "validation": validation,
                "error_codes": error_codes,
                "diagnostic": diagnostic,
                "phase": phase,
                "phase_iteration": phase_iteration,
                "window_started": true,
                "integrity_checkpoint": integrity_checkpoint,
            }),
        ));
    }
    if let (Some(raw_begin), Some(raw_end)) = (
        invocation["validation"]["raw_begin"].as_u64(),
        invocation["validation"]["raw_end"].as_u64(),
    ) {
        let monotonic_span = capture_end_ms.saturating_sub(capture_begin_ms).max(1);
        let raw_span = raw_end.saturating_sub(raw_begin);
        for series in &mut memory_series {
            for sample in &mut series.samples {
                if (capture_begin_ms..=capture_end_ms).contains(&sample.monotonic_offset_ms) {
                    let relative = sample.monotonic_offset_ms.saturating_sub(capture_begin_ms);
                    sample.aligned_tracy_offset = Some(
                        raw_begin
                            .saturating_add(raw_span.saturating_mul(relative) / monotonic_span),
                    );
                }
            }
        }
    }
    let trace_sha256 = hash_file(&temporary_path)?;
    let trace_bytes = std::fs::metadata(&temporary_path)?.len();
    let sidecar = json!({
        "schema":2,
        "created_at_unix_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis(),
        "requested_duration_ms":duration_ms,
        "trace_sha256":trace_sha256,
        "trace_bytes":trace_bytes,
        "helper_identity":{"source_revision":installation.helper.source_revision,"sha256":installation.helper.sha256,"patch_sha256":installation.helper.patch_sha256},
        "hook_identity":{"source_revision":installation.hook.source_revision,"sha256":installation.hook.sha256,"patches":installation.hook.patches},
        "tracy_protocol":installation.helper.protocol_version,
        "capture":invocation,
        "owned_process_ids":owned_process_ids,
        "process_identities":process_identities,
        "memory_series":memory_series,
        "memory_summary":crate::process_metrics::summarize_memory(&memory_series),
        "network_evidence":network_audit,
        "integrity_status":"baseline_recorded",
        "integrity_journal":integrity_journal,
        "phase":phase,
        "phase_iteration":phase_iteration,
        "capture_annotations":capture_annotations,
        "experiment_identity":experiment,
        "meridian_mcp_build":crate::build_identity::current(),
        "launch_manifest_sha256":launch_manifest_sha256,
        "experiment_manifest_sha256":experiment_manifest_sha256,
        "runtime_wake":runtime_wake,
    });
    let artifacts = reserved.promote(&sidecar)?;
    {
        let mut capture = state.tracy_capture().await;
        capture
            .integrity_owned_paths
            .extend([artifacts.trace.path.clone(), artifacts.sidecar.path.clone()]);
        capture.capture_records.push(json!({
            "trace_path": artifacts.trace.path,
            "trace_sha256": artifacts.trace.sha256,
            "sidecar_path": artifacts.sidecar.path,
            "sidecar_sha256": artifacts.sidecar.sha256,
            "phase": phase,
            "phase_iteration": phase_iteration,
        }));
        capture.network_records.push(json!({
            "lifecycle": "capture",
            "phase": phase,
            "phase_iteration": phase_iteration,
            "evidence": sidecar["network_evidence"],
        }));
    }
    let integrity_checkpoint = checkpoint_integrity(context, state, "post_capture").await?;
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({
            "artifact":artifacts.trace,
            "sidecar":artifacts.sidecar,
            "capture":sidecar["capture"],
            "network_audit":sidecar["network_evidence"],
            "helper_revision":installation.helper.source_revision,
            "protocol_version":installation.helper.protocol_version,
            "integrity_checkpoint":integrity_checkpoint,
        }),
    ))
}

struct InvalidCaptureContext<'a> {
    experiment_directory: &'a Path,
    experiment: &'a crate::tracy_experiment::ExperimentIdentity,
    phase: &'a str,
    phase_iteration: u32,
    integrity_journal: &'a Option<crate::workspace_integrity::IntegrityJournalSummary>,
}

fn retain_invalid_capture(
    context: &ToolExecutionContext,
    reserved: ReservedTraceSet,
    capture_context: InvalidCaptureContext<'_>,
    validation: Value,
) -> Result<crate::tracy_artifact::DiagnosticTraceSet> {
    let diagnostics = capture_context
        .experiment_directory
        .join(".meridian-tracy-diagnostics");
    std::fs::create_dir_all(&diagnostics)?;
    let diagnostics = context.policy().read_path(&diagnostics)?;
    let created_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let diagnostic_trace = diagnostics.join(format!(
        "{}-{}-{created_at_unix_ms}.invalid.tracy",
        capture_context.phase, capture_context.phase_iteration
    ));
    let trace_sha256 = hash_file(reserved.temporary_trace_path())?;
    let trace_bytes = std::fs::metadata(reserved.temporary_trace_path())?.len();
    reserved
        .promote_diagnostic(
            context.policy(),
            &diagnostic_trace,
            &json!({
                "schema": 2,
                "authoritative": false,
                "diagnostic_reason": "invalid_capture",
                "created_at_unix_ms": created_at_unix_ms,
                "trace_sha256": trace_sha256,
                "trace_bytes": trace_bytes,
                "phase": capture_context.phase,
                "phase_iteration": capture_context.phase_iteration,
                "validation": validation,
                "experiment_identity": capture_context.experiment,
                "meridian_mcp_build": crate::build_identity::current(),
                "integrity_journal": capture_context.integrity_journal,
            }),
        )
        .map_err(Into::into)
}

async fn record_invalid_capture(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    diagnostic: &crate::tracy_artifact::DiagnosticTraceSet,
    phase: &str,
    phase_iteration: u32,
    validation: &Value,
) -> Result<Option<crate::workspace_integrity::IntegrityCheckpoint>> {
    let mut capture = state.tracy_capture().await;
    capture.record_diagnostic(
        json!({
            "authoritative": false,
            "trace": &diagnostic.trace,
            "sidecar": &diagnostic.sidecar,
            "phase": phase,
            "phase_iteration": phase_iteration,
            "validation": validation,
        }),
        [
            diagnostic.trace.path.clone(),
            diagnostic.sidecar.path.clone(),
        ],
    );
    drop(capture);
    checkpoint_integrity(context, state, "invalid_capture").await
}

async fn checkpoint_integrity(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    action: &str,
) -> Result<Option<crate::workspace_integrity::IntegrityCheckpoint>> {
    let (baseline, owned_paths) = {
        let capture = state.tracy_capture().await;
        (
            capture.integrity.clone(),
            capture.integrity_owned_paths.clone(),
        )
    };
    let Some(baseline) = baseline else {
        return Ok(None);
    };
    let checkpoint = baseline.checkpoint(action, &owned_paths)?;
    let mut capture = state.tracy_capture().await;
    if let Some(journal) = capture.integrity_journal.as_mut() {
        journal.record(context.policy(), checkpoint.clone())?;
    }
    Ok(Some(checkpoint))
}

pub async fn status(state: &crate::state::ServerState) -> Result<ToolResult> {
    let (
        running,
        kind,
        game_port,
        profiler_port,
        pid,
        last_exit_code,
        recent_output,
        launch_provenance,
    ) = {
        let mut runtime = state.runtime().await;
        let running = runtime.is_game_running();
        (
            running,
            runtime.kind,
            runtime.game_port,
            runtime.profiler_port,
            runtime
                .game_process
                .as_ref()
                .and_then(|process| process.id()),
            runtime.last_exit_code,
            runtime.recent_output(50),
            runtime.launch_provenance.clone(),
        )
    };
    let collector = state.tracy_capture().await.collector.clone();
    let mut collector_stderr_tail = Vec::new();
    let mut collector_exit_code = None;
    let mut helper_status = None;
    let mut collector_stopped = false;
    if let Some(collector) = collector {
        if collector.is_running().await {
            helper_status = collector.status().await.ok();
        } else {
            collector_stopped = true;
            collector_stderr_tail = collector.stderr_tail().await;
            collector_exit_code = collector.exit_code().await;
        }
    }
    let mut capture = state.tracy_capture().await;
    if let Some(helper_status) = helper_status {
        capture.last_status = Some(helper_status);
    }
    if collector_stopped {
        capture.phase = Some(TracySessionPhase::Stopped);
    }
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({
            "running":running,
            "runtime_kind":kind,
            "game_port":game_port,
            "profiler_port":profiler_port,
            "pid":pid,
            "last_exit_code":last_exit_code,
            "recent_output":recent_output,
            "capture_active":capture.active,
            "capture_output_path":capture.output_path,
            "last_capture_error":capture.last_error,
            "collector_phase":capture.phase,
            "collector_status":capture.last_status,
            "runtime_wake":capture.experiment.as_ref().and_then(|experiment| experiment.runtime_wake.as_ref()),
            "integrity_journal":capture.integrity_journal.as_ref().map(crate::workspace_integrity::IntegrityJournal::summary),
            "collector_stderr_tail":collector_stderr_tail,
            "collector_exit_code":collector_exit_code,
            "launch_provenance":launch_provenance,
        }),
    ))
}

pub async fn stop(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
) -> Result<ToolResult> {
    let _lifecycle = state.lifecycle().await;
    stop_with_lifecycle(context, state).await
}

pub(super) async fn stop_with_lifecycle(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
) -> Result<ToolResult> {
    let cleanup_only = state.tracy_capture().await.integrity_journal.is_some()
        || state
            .runtime()
            .await
            .execution_lease
            .as_ref()
            .is_some_and(|lease| lease.kind() == "tracy");
    let pre_stop_checkpoint = checkpoint_integrity(context, state, "pre_stop").await;
    let stop_identities = owned_process_identities(state).await;
    let stop_profiler_port = state.runtime().await.profiler_port;
    let launch_provenance = state.runtime().await.launch_provenance.clone();
    let mut stop_network_audit = crate::network_audit::NetworkAuditCollector::new(true);
    stop_network_audit.sample(
        &stop_identities
            .iter()
            .map(|identity| identity.pid)
            .collect::<Vec<_>>(),
        0,
    );
    let collector = state.tracy_capture().await.collector.clone();
    if let Some(collector) = collector {
        let _ = collector.cancel().await;
    }
    // The active window can end before trace/sidecar/journal publication. This
    // guard proves the entire capture operation ended before releasing scope.
    let _publication = tokio::time::timeout(Duration::from_secs(10), state.tracy_publication())
        .await
        .map_err(|_| {
            anyhow!("Tracy capture publication remains active; execution ownership is retained")
        })?;
    let collector = state.tracy_capture().await.collector.clone();
    let mut collector_cleanup_error = None;
    if let Some(collector) = collector {
        let stopped = collector.stop(Duration::from_secs(10)).await;
        if collector.cleanup_confirmed() {
            state.tracy_capture().await.collector = None;
        } else {
            collector_cleanup_error = Some(anyhow!(
                "collector cleanup remains unconfirmed: {:?}",
                stopped.err()
            ));
        }
    }
    {
        let mut capture = state.tracy_capture().await;
        if let Some(client) = capture.wake_client.as_mut() {
            stop_wake_client(client).await?;
            capture.wake_client = None;
        }
    }
    let mut runtime = state.runtime().await;
    let runtime_was_running = runtime.is_game_running();
    let runtime_was_profiled = runtime.kind == Some(crate::state::RuntimeKind::Tracy);
    if runtime_was_running && runtime_was_profiled {
        runtime.stop_game_process().await?;
    }
    if !runtime_was_running {
        runtime.finish_runtime_cleanup().await?;
    }
    drop(runtime);
    if let Some(error) = collector_cleanup_error {
        return Err(error);
    }
    let (
        memory_series,
        memory_task,
        experiment,
        capture_records,
        diagnostic_records,
        mut network_records,
    ) = {
        let mut capture = state.tracy_capture().await;
        if let Some(stop) = capture.memory_stop.take() {
            let _ = stop.send(true);
        }
        (
            capture.memory_series.take(),
            capture.memory_task.take(),
            capture.experiment.take(),
            std::mem::take(&mut capture.capture_records),
            std::mem::take(&mut capture.diagnostic_records),
            std::mem::take(&mut capture.network_records),
        )
    };
    if let Some(port) = stop_profiler_port {
        network_records.push(json!({
            "lifecycle": "stop",
            "evidence": crate::network_audit::tracy_network_evidence(
                stop_network_audit.finish(),
                port,
                &stop_identities,
                true,
                true,
            ),
        }));
    }
    if let Some(task) = memory_task {
        let _ = task.await;
    }
    let complete_memory = if let Some(series) = memory_series {
        series.lock().await.clone()
    } else {
        Vec::new()
    };
    if experiment.is_none() && !runtime_was_profiled && !cleanup_only {
        return Err(anyhow!("no MCP-owned Tracy runtime is active"));
    }
    let experiment_manifest = if let Some(experiment) = experiment {
        let document = json!({
            "schema": 2,
            "experiment_identity": experiment.locked_identity,
            "runtime_configuration": experiment.runtime_configuration,
            "runtime_wake": experiment.runtime_wake,
            "meridian_mcp_build": crate::build_identity::current(),
            "launch_manifest_sha256": experiment.launch_manifest_sha256,
            "captures": capture_records,
            "diagnostics": diagnostic_records,
            "network_evidence": network_records,
            "memory_series": complete_memory,
            "memory_summary": crate::process_metrics::summarize_memory(&complete_memory),
        });
        let bytes = serde_json::to_vec_pretty(&document)?;
        Some(write_atomic(
            context.policy(),
            &experiment.final_manifest_path,
            false,
            |output| {
                std::io::Write::write_all(output, &bytes)?;
                std::io::Write::write_all(output, b"\n")?;
                Ok(())
            },
        )?)
    } else {
        None
    };
    let post_stop_checkpoint = checkpoint_integrity(context, state, "post_stop").await;
    let integrity_errors = [pre_stop_checkpoint.as_ref(), post_stop_checkpoint.as_ref()]
        .into_iter()
        .filter_map(|result| result.err().map(ToString::to_string))
        .collect::<Vec<_>>();
    let journal_summary = if integrity_errors.is_empty() {
        let mut capture = state.tracy_capture().await;
        if let Some(journal) = capture.integrity_journal.as_mut() {
            journal.finalize(context.policy())?;
        }
        let summary = capture
            .integrity_journal
            .as_ref()
            .map(crate::workspace_integrity::IntegrityJournal::summary);
        capture.integrity = None;
        capture.integrity_journal = None;
        capture.integrity_owned_paths.clear();
        summary
    } else {
        state
            .tracy_capture()
            .await
            .integrity_journal
            .as_ref()
            .map(crate::workspace_integrity::IntegrityJournal::summary)
    };
    if integrity_errors.is_empty() {
        let mut runtime = state.runtime().await;
        if let Some(lease) = runtime
            .execution_lease
            .as_mut()
            .filter(|lease| lease.kind() == "tracy")
        {
            lease.finish()?;
            runtime.execution_lease = None;
        }
    }
    if !integrity_errors.is_empty() {
        return Ok(structured_error(
            ToolErrorCode::WorkspaceIntegrityViolation,
            "The profiling lifecycle stopped its owned processes but failed workspace integrity checks.",
            Some("Inspect the unfinished integrity journal and affected relative paths; do not automatically repair or delete source files.".to_owned()),
            json!({
                "lifecycle": "stopped",
                "errors": integrity_errors,
                "experiment_manifest": experiment_manifest,
                "integrity_journal": journal_summary,
            }),
        ));
    }
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({
            "lifecycle":"stopped",
            "runtime_was_running":runtime_was_running,
            "experiment_manifest":experiment_manifest,
            "pre_stop_integrity_checkpoint":pre_stop_checkpoint.ok().flatten(),
            "post_stop_integrity_checkpoint":post_stop_checkpoint.ok().flatten(),
            "integrity_journal":journal_summary,
            "launch_provenance":launch_provenance,
        }),
    ))
}

pub async fn hotspots(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyHotspotsParams,
) -> Result<ToolResult> {
    invoke_analysis(
        context,
        TracyCommand::Hotspots,
        json!({
            "trace_path": args.trace_path.as_str(),
            "limit": args.limit.unwrap_or(100),
            "sort": args.sort.as_ref().map(|value| value.as_str()).unwrap_or("inclusive"),
        }),
        Duration::from_secs(120),
        state,
        None,
    )
    .await
}

pub async fn zone(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyZoneParams,
) -> Result<ToolResult> {
    invoke_analysis(
        context,
        TracyCommand::Zone,
        json!({
            "trace_path": args.trace_path.as_str(),
            "name": args.name.as_str(),
            "limit": args.limit.unwrap_or(100),
        }),
        Duration::from_secs(120),
        state,
        None,
    )
    .await
}

pub async fn frame_stats(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyFrameStatsParams,
) -> Result<ToolResult> {
    invoke_analysis(
        context,
        TracyCommand::FrameStats,
        json!({"trace_path":args.trace_path.as_str()}),
        Duration::from_secs(120),
        state,
        None,
    )
    .await
}

pub async fn compare(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyCompareParams,
) -> Result<ToolResult> {
    let baseline_path = Path::new(args.baseline_path.as_str());
    let current_path = Path::new(args.current_path.as_str());
    let mode = match args
        .comparison_mode
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("same_experiment_same_phase")
    {
        "same_experiment_same_phase" => {
            crate::tracy_artifact::ComparisonMode::SameExperimentSamePhase
        }
        "cross_experiment" => crate::tracy_artifact::ComparisonMode::CrossExperiment,
        _ => return Err(anyhow!("comparison_mode is not supported")),
    };
    let baseline_metadata = load_trace_metadata(context, state, baseline_path).await?;
    let current_metadata = load_trace_metadata(context, state, current_path).await?;
    let compatibility = match (&baseline_metadata, &current_metadata) {
        (Some(baseline), Some(current)) => {
            crate::tracy_artifact::compare_metadata(baseline, current, mode)
        }
        (None, None) if baseline_path == current_path => {
            crate::tracy_artifact::ComparisonCompatibility {
                compatible: true,
                mode,
                checked_fields: Vec::new(),
                mismatches: Vec::new(),
                warnings: vec!["identity_verification_unavailable".into()],
            }
        }
        _ => {
            return Err(anyhow!(
                "cross-trace comparison requires valid Meridian sidecars"
            ))
        }
    };
    if !compatibility.compatible {
        return Err(anyhow!(
            "trace identities are incompatible: {}",
            serde_json::to_string(&compatibility)?
        ));
    }
    let mut compare_params = json!({
        "baseline_path": args.baseline_path.as_str(),
        "current_path": args.current_path.as_str(),
        "minimum_delta_ns": args.minimum_delta_ns.unwrap_or(0),
        "limit": args.limit.unwrap_or(100),
    });
    if let (Some(baseline), Some(current), Some(object)) = (
        baseline_metadata.as_ref(),
        current_metadata.as_ref(),
        compare_params.as_object_mut(),
    ) {
        object.extend(serde_json::Map::from_iter([
            (
                "baseline_range_begin_ns".into(),
                json!(baseline.trace_range_ns.raw_begin),
            ),
            (
                "baseline_range_end_ns".into(),
                json!(baseline.trace_range_ns.raw_end),
            ),
            (
                "current_range_begin_ns".into(),
                json!(current.trace_range_ns.raw_begin),
            ),
            (
                "current_range_end_ns".into(),
                json!(current.trace_range_ns.raw_end),
            ),
        ]));
    }
    invoke_analysis(
        context,
        TracyCommand::Compare,
        compare_params,
        Duration::from_secs(180),
        state,
        Some(json!({"compatibility":compatibility})),
    )
    .await
}

pub async fn control_stats(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    args: crate::parameters::TracyControlStatsParams,
) -> Result<ToolResult> {
    let installation = context
        .tracy()
        .ok_or_else(|| anyhow!("Tracy installation unavailable"))?;
    let requested = &args.trace_paths;
    if !(3..=20).contains(&requested.len()) {
        return Err(anyhow!("trace_paths must contain 3-20 controls"));
    }
    let mut trace_paths = Vec::new();
    let mut unique = std::collections::BTreeSet::new();
    for value in requested {
        let path = context.policy().read_path(value.as_str())?;
        if !unique.insert(path.clone()) {
            return Err(anyhow!("trace_paths contains a duplicate path"));
        }
        trace_paths.push(path);
    }
    let percentile = args
        .frame_percentile
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("p95");
    let percentile_key = match percentile {
        "p50" => "p50_ns",
        "p95" => "p95_ns",
        "p99" => "p99_ns",
        _ => return Err(anyhow!("frame_percentile is not supported")),
    };
    let zone_keys = args.zone_keys.clone().unwrap_or_default();
    if zone_keys.len() > 32 {
        return Err(anyhow!("zone_keys exceeds the fixed entry limit"));
    }
    let mut metadata = Vec::with_capacity(trace_paths.len());
    for path in &trace_paths {
        metadata.push(load_trace_metadata(context, state, path).await?);
    }
    if metadata.iter().any(Option::is_none) {
        return Err(anyhow!(
            "control statistics require schema-2 Meridian sidecars"
        ));
    }
    let metadata = metadata.into_iter().flatten().collect::<Vec<_>>();
    let comparison_mode = match args
        .comparison_mode
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("same_experiment_same_phase")
    {
        "same_experiment_same_phase" => {
            crate::tracy_artifact::ComparisonMode::SameExperimentSamePhase
        }
        "cross_experiment" => crate::tracy_artifact::ComparisonMode::CrossExperiment,
        _ => return Err(anyhow!("comparison_mode is not supported")),
    };
    let incomplete_count = metadata
        .iter()
        .filter(|item| !crate::tracy_artifact::is_complete_control_capture(item))
        .count();
    let mut compatibility = crate::tracy_artifact::ComparisonCompatibility {
        compatible: true,
        mode: comparison_mode,
        checked_fields: Vec::new(),
        mismatches: Vec::new(),
        warnings: Vec::new(),
    };
    if metadata.iter().any(|item| item.queue_saturated) {
        compatibility
            .warnings
            .push("queue_saturation_observed_without_data_loss".to_owned());
    }
    for current in metadata.iter().skip(1) {
        let result =
            crate::tracy_artifact::compare_metadata(&metadata[0], current, comparison_mode);
        compatibility.checked_fields.extend(result.checked_fields);
        compatibility.mismatches.extend(result.mismatches);
    }
    compatibility.checked_fields.sort();
    compatibility.checked_fields.dedup();
    compatibility.compatible = compatibility.mismatches.is_empty();
    if !compatibility.compatible {
        return Err(anyhow!(
            "control identities are incompatible: {}",
            serde_json::to_string(&compatibility)?
        ));
    }
    let working_directory = context
        .policy()
        .workspace_roots()
        .first()
        .ok_or_else(|| anyhow!("workspace root unavailable"))?;
    let mut values = Vec::new();
    for (index, path) in trace_paths.iter().enumerate() {
        let invocation = invoke_helper(TracyInvocationSpec {
            helper: &installation.helper.path,
            working_directory,
            id: index as u64 + 1,
            command: TracyCommand::FrameStats,
            params: json!({"trace_path":path,"range_begin_ns":metadata[index].trace_range_ns.raw_begin,"range_end_ns":metadata[index].trace_range_ns.raw_end}),
            timeout: Duration::from_secs(120),
            capture_network: false,
            environment: Vec::new(),
            cancellation: None,
        })
        .await?;
        values.push(
            invocation.result[percentile_key]
                .as_u64()
                .ok_or_else(|| anyhow!("frame statistics omitted {percentile_key}"))?,
        );
    }
    let (frame_time, noise) = crate::tracy_statistics::summarize_controls(&values)
        .ok_or_else(|| anyhow!("insufficient_complete_samples"))?;
    let mut zones = serde_json::Map::new();
    let mut request_id = trace_paths.len() as u64 + 1;
    for key in zone_keys {
        let parts = key.split('|').collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(anyhow!(
                "zone key must be file|line|name|inclusive_or_self|p50_p95_or_p99"
            ));
        }
        let line = parts[1]
            .parse::<u64>()
            .map_err(|_| anyhow!("zone key line is invalid"))?;
        let metric = match (parts[3], parts[4]) {
            ("inclusive", "p50") => "p50_ns",
            ("inclusive", "p95") => "p95_ns",
            ("inclusive", "p99") => "p99_ns",
            ("self", "p50") => "self_p50_ns",
            ("self", "p95") => "self_p95_ns",
            ("self", "p99") => "self_p99_ns",
            _ => return Err(anyhow!("zone key metric is invalid")),
        };
        let mut zone_values = Vec::new();
        for (index, path) in trace_paths.iter().enumerate() {
            let invocation = invoke_helper(TracyInvocationSpec {
                helper: &installation.helper.path,
                working_directory,
                id: request_id,
                command: TracyCommand::Zone,
                params: json!({"trace_path":path,"name":parts[2],"limit":1000,"range_begin_ns":metadata[index].trace_range_ns.raw_begin,"range_end_ns":metadata[index].trace_range_ns.raw_end}),
                timeout: Duration::from_secs(120),
                capture_network: false,
                environment: Vec::new(),
                cancellation: None,
            })
            .await?;
            request_id += 1;
            let item = invocation.result["items"]
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item["file"] == parts[0] && item["line"] == line)
                })
                .ok_or_else(|| anyhow!("requested exact zone was absent: {key}"))?;
            zone_values.push(
                item[metric]
                    .as_u64()
                    .ok_or_else(|| anyhow!("zone result omitted {metric}"))?,
            );
        }
        let (summary, zone_noise) = crate::tracy_statistics::summarize_controls(&zone_values)
            .ok_or_else(|| anyhow!("insufficient_complete_samples"))?;
        zones.insert(key, json!({"distribution":summary,"noise":zone_noise}));
    }
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({
            "schema":2,
            "input_count":trace_paths.len(),
            "valid_count":trace_paths.len() - incomplete_count,
            "incomplete_count":incomplete_count,
            "establishes_control_baseline":incomplete_count == 0 && !noise.noisy && zones.values().all(|value| !value["noise"]["noisy"].as_bool().unwrap_or(true)),
            "compatibility":compatibility,
            "frame_percentile":percentile,
            "frame_time":frame_time,
            "zones":zones,
            "noise":noise,
        }),
    ))
}

async fn load_trace_metadata(
    context: &ToolExecutionContext,
    state: &crate::state::ServerState,
    path: &Path,
) -> Result<Option<crate::tracy_artifact::TraceMetadata>> {
    let policy = context.policy().clone();
    let path = path.to_owned();
    state
        .run_asset_job(move || Ok(crate::tracy_artifact::read_trace_metadata(&policy, &path)?))
        .await
}

async fn invoke_analysis(
    context: &ToolExecutionContext,
    command: TracyCommand,
    mut params: Value,
    timeout: Duration,
    state: &crate::state::ServerState,
    context_fields: Option<Value>,
) -> Result<ToolResult> {
    let installation = context
        .tracy()
        .ok_or_else(|| anyhow!("Tracy installation unavailable"))?;
    let working_directory = context
        .policy()
        .workspace_roots()
        .first()
        .ok_or_else(|| anyhow!("workspace root unavailable"))?;
    let metadata = match params.get("trace_path").and_then(Value::as_str) {
        Some(path) => load_trace_metadata(context, state, Path::new(path)).await?,
        None => None,
    };
    if let (Some(metadata), Some(object)) = (&metadata, params.as_object_mut()) {
        object.insert(
            "range_begin_ns".into(),
            json!(metadata.trace_range_ns.raw_begin),
        );
        object.insert(
            "range_end_ns".into(),
            json!(metadata.trace_range_ns.raw_end),
        );
    }
    let invocation = invoke_helper(TracyInvocationSpec {
        helper: &installation.helper.path,
        working_directory,
        id: 1,
        command,
        params,
        timeout,
        capture_network: false,
        environment: Vec::new(),
        cancellation: None,
    })
    .await?;
    let mut result = invocation.result;
    let statistics = result.clone();
    let object = result
        .as_object_mut()
        .ok_or_else(|| anyhow!("Tracy helper result must be an object"))?;
    object.insert(
        "helper_revision".into(),
        Value::String(installation.helper.source_revision.clone()),
    );
    if let Some(Value::Object(fields)) = context_fields {
        object.extend(fields);
    }
    match metadata {
        Some(metadata) => {
            let native_counts = object.get("counts").cloned().unwrap_or_else(|| json!({}));
            object.extend(serde_json::Map::from_iter([
                ("schema".into(), json!(2)),
                (
                    "experiment_id".into(),
                    json!(metadata.experiment_identity.experiment_id),
                ),
                ("capture_id".into(), json!(metadata.trace_sha256)),
                ("phase".into(), json!(metadata.phase)),
                ("phase_iteration".into(), json!(metadata.phase_iteration)),
                ("range".into(), json!(metadata.range)),
                ("counts".into(), native_counts),
                ("statistics".into(), statistics),
                ("warnings".into(), json!([])),
                ("identity_verification".into(), json!("verified")),
                ("window_source".into(), json!("meridian_sidecar")),
            ]));
        }
        None => {
            object.insert("schema".into(), json!(2));
            object.insert("statistics".into(), statistics);
            object.insert(
                "warnings".into(),
                json!(["identity_verification_unavailable"]),
            );
            object.insert("identity_verification".into(), json!("unavailable"));
            object.insert("window_source".into(), json!("full_trace_legacy"));
        }
    }
    object.insert(
        "protocol_version".into(),
        json!(installation.helper.protocol_version),
    );
    correlate_sources(state, &mut result).await;
    Ok(json_success(ToolMetadata::complete(None), result))
}

async fn correlate_sources(state: &crate::state::ServerState, result: &mut Value) {
    let Some(snapshot) = state.active_snapshot().await else {
        return;
    };
    let Some(items) = result.get_mut("items").and_then(Value::as_array_mut) else {
        return;
    };
    let root = snapshot
        .environment_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    for item in items {
        let Some(file) = item.get("file").and_then(Value::as_str) else {
            continue;
        };
        let Some(line) = item.get("line").and_then(Value::as_u64) else {
            continue;
        };
        let reported = Path::new(file);
        let candidate = if reported.is_absolute() {
            reported.to_owned()
        } else {
            root.join(reported)
        };
        let Ok(source_path) = candidate.canonicalize() else {
            continue;
        };
        if !source_path.starts_with(root) {
            continue;
        }
        if let Some(object) = item.as_object_mut() {
            object.insert(
                "source_correlation".into(),
                json!({
                    "match":"file_line",
                    "path":source_path,
                    "line":line,
                    "state_generation":snapshot.generation,
                }),
            );
        }
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let mut input = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn git_revision(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn valid_phase(phase: &str) -> bool {
    !phase.is_empty()
        && phase.len() <= 64
        && phase.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

async fn owned_process_identities(
    state: &crate::state::ServerState,
) -> Vec<crate::process_metrics::ProcessIdentity> {
    let (collector, wake_client_pid) = {
        let capture = state.tracy_capture().await;
        (
            capture.collector.clone(),
            capture
                .wake_client
                .as_ref()
                .and_then(|client| client.process.id()),
        )
    };
    let collector_pid = match collector {
        Some(collector) => collector.process_id().await,
        None => None,
    };
    let game_pid = state
        .runtime()
        .await
        .game_process
        .as_ref()
        .and_then(|process| process.id());
    [
        (game_pid, crate::process_metrics::ProcessRole::DreamDaemon),
        (
            wake_client_pid,
            crate::process_metrics::ProcessRole::DreamSeeker,
        ),
        (
            collector_pid,
            crate::process_metrics::ProcessRole::Collector,
        ),
    ]
    .into_iter()
    .filter_map(|(pid, role)| {
        pid.and_then(|pid| crate::process_metrics::process_identity(pid, role).ok())
    })
    .collect()
}

fn capture_memory_window(
    all_series: &[crate::process_metrics::RoleMemorySeries],
    begin_ms: u64,
    end_ms: u64,
) -> Vec<crate::process_metrics::RoleMemorySeries> {
    all_series
        .iter()
        .map(|series| {
            let previous = series
                .samples
                .iter()
                .filter(|sample| sample.monotonic_offset_ms < begin_ms)
                .map(|sample| sample.monotonic_offset_ms)
                .max();
            let next = series
                .samples
                .iter()
                .filter(|sample| sample.monotonic_offset_ms > end_ms)
                .map(|sample| sample.monotonic_offset_ms)
                .min();
            let mut selected = series.clone();
            selected.samples.retain(|sample| {
                (begin_ms..=end_ms).contains(&sample.monotonic_offset_ms)
                    || previous == Some(sample.monotonic_offset_ms)
                    || next == Some(sample.monotonic_offset_ms)
            });
            selected
        })
        .collect()
}

fn topic_was_processed(attempts: &[Value]) -> bool {
    attempts
        .iter()
        .any(|attempt| attempt["topic_processed"].as_bool() == Some(true))
}

async fn register_memory_identities(
    series: &Arc<tokio::sync::Mutex<Vec<crate::process_metrics::RoleMemorySeries>>>,
    identities: &[crate::process_metrics::ProcessIdentity],
) {
    let mut series = series.lock().await;
    for identity in identities {
        if series.iter().any(|entry| entry.identity == *identity) {
            continue;
        }
        series.push(crate::process_metrics::RoleMemorySeries {
            identity: identity.clone(),
            operating_system: std::env::consts::OS.to_owned(),
            sampling_interval_ms: 500,
            samples: Vec::new(),
            missed_samples: 0,
        });
    }
}

fn start_memory_sampler(
    identities: &[crate::process_metrics::ProcessIdentity],
    started: tokio::time::Instant,
) -> (
    Arc<tokio::sync::Mutex<Vec<crate::process_metrics::RoleMemorySeries>>>,
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let series = Arc::new(tokio::sync::Mutex::new(
        identities
            .iter()
            .cloned()
            .map(|identity| crate::process_metrics::RoleMemorySeries {
                identity,
                operating_system: std::env::consts::OS.to_owned(),
                sampling_interval_ms: 500,
                samples: Vec::new(),
                missed_samples: 0,
            })
            .collect::<Vec<_>>(),
    ));
    let (stop, mut stop_rx) = tokio::sync::watch::channel(false);
    let task_series = Arc::clone(&series);
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        let mut inactive_identities = Vec::new();
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let offset = started.elapsed().as_millis() as u64;
                    let mut all_series = task_series.lock().await;
                    for series in all_series.iter_mut() {
                        if inactive_identities.contains(&series.identity) {
                            continue;
                        }
                        match crate::process_metrics::sample_process(&series.identity, offset) {
                            Ok(samples) if series.samples.len().saturating_add(samples.len()) <= 20_000 => series.samples.extend(samples),
                            _ => {
                                series.missed_samples = series.missed_samples.saturating_add(1);
                                inactive_identities.push(series.identity.clone());
                            }
                        }
                    }
                }
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() { break; }
                }
            }
        }
    });
    (series, stop, task)
}

fn git_workspace_root(path: &Path) -> Result<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &path.to_string_lossy(),
            "rev-parse",
            "--show-toplevel",
        ])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let root = String::from_utf8(output.stdout)?.trim().to_owned();
            if !root.is_empty() {
                return Ok(std::path::PathBuf::from(root).canonicalize()?);
            }
        }
    }
    Ok(path.canonicalize()?)
}

#[cfg(test)]
mod tests {
    use super::{launch, register_memory_identities, stop, topic_was_processed};
    use crate::process_metrics::{ProcessIdentity, ProcessRole, RoleMemorySeries};
    use crate::tools::ToolExecutionContext;
    use crate::{CapabilityMode, PathPolicy, RiftBuildAccess};
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn identity(pid: u32, role: ProcessRole) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            started_at_identity: u64::from(pid),
            role,
        }
    }

    #[tokio::test]
    async fn memory_sampler_registers_late_owned_processes_without_duplicates() {
        let initial = identity(10, ProcessRole::DreamDaemon);
        let collector = identity(11, ProcessRole::Collector);
        let series = Arc::new(tokio::sync::Mutex::new(vec![RoleMemorySeries {
            identity: initial.clone(),
            operating_system: std::env::consts::OS.to_owned(),
            sampling_interval_ms: 500,
            samples: Vec::new(),
            missed_samples: 0,
        }]));

        register_memory_identities(&series, &[initial, collector.clone()]).await;

        let series = series.lock().await;
        assert_eq!(series.len(), 2);
        assert_eq!(series[1].identity, collector);
    }

    #[test]
    fn topic_summary_reflects_observed_attempts() {
        assert!(!topic_was_processed(&[json!({"topic_processed":false})]));
        assert!(topic_was_processed(&[
            json!({"topic_processed":false}),
            json!({"topic_processed":true}),
        ]));
    }

    #[tokio::test]
    async fn failed_launch_cleanup_finalizes_a_pre_manifest_integrity_journal() {
        let root = std::env::temp_dir().join(format!(
            "meridian-tracy-launch-cleanup-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let experiment_directory = root.join("experiment");
        std::fs::create_dir_all(&experiment_directory).unwrap();
        let context = ToolExecutionContext::with_features(
            CapabilityMode::Development,
            PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
            RiftBuildAccess::Disabled,
            None,
            None,
            None,
        );
        let baseline = crate::workspace_integrity::IntegrityBaseline::capture(&root).unwrap();
        let journal = crate::workspace_integrity::IntegrityJournal::create(
            context.policy(),
            &experiment_directory,
            &baseline,
        )
        .unwrap();
        let journal_path = journal.path().to_owned();
        let state = crate::state::ServerState::new();
        {
            let mut capture = state.tracy_capture().await;
            capture.integrity = Some(baseline);
            capture.integrity_owned_paths.push(journal_path.clone());
            capture.integrity_journal = Some(journal);
        }

        let publication = state.tracy_publication().await;
        let mut stopping = Box::pin(stop(&context, &state));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut stopping)
                .await
                .is_err(),
            "stop bypassed an unfinished capture publication"
        );
        assert!(state.tracy_capture().await.integrity_journal.is_some());
        drop(publication);
        let result = stopping.await.unwrap();

        assert_ne!(result.is_error, Some(true));
        let journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        assert_eq!(journal["status"], "finalized");
        assert!(state.tracy_capture().await.integrity_journal.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unconfirmed_collector_cleanup_preserves_integrity_journal_for_retry() {
        let (root, collector) = crate::tracy_collector::tests::owned_fixture().await;
        let experiment_directory = root.join("experiment");
        std::fs::create_dir_all(&experiment_directory).unwrap();
        let context = ToolExecutionContext::with_features(
            CapabilityMode::Development,
            PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
            RiftBuildAccess::Disabled,
            None,
            None,
            None,
        );
        let baseline = crate::workspace_integrity::IntegrityBaseline::capture(&root).unwrap();
        let journal = crate::workspace_integrity::IntegrityJournal::create(
            context.policy(),
            &experiment_directory,
            &baseline,
        )
        .unwrap();
        let state = crate::state::ServerState::new();
        {
            let mut capture = state.tracy_capture().await;
            capture.integrity = Some(baseline);
            capture
                .integrity_owned_paths
                .push(journal.path().to_owned());
            capture.integrity_journal = Some(journal);
            capture.collector = Some(collector.clone());
        }
        collector.inject_cleanup_fault(2);
        let result = stop(&context, &state).await;
        let retained = state.tracy_capture().await.integrity_journal.is_some();
        let retained_collector = state.tracy_capture().await.collector.is_some();
        collector.inject_cleanup_fault(0);
        let _ = collector.stop(std::time::Duration::from_millis(100)).await;
        assert!(result.is_err());
        assert!(retained && retained_collector);
        stop(&context, &state).await.unwrap();
        assert!(state.tracy_capture().await.integrity_journal.is_none());
        drop(collector);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn launch_waits_for_exclusive_runtime_lifecycle_access() {
        let root = std::env::temp_dir().join(format!(
            "meridian-tracy-launch-lifecycle-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let context = ToolExecutionContext::with_features(
            CapabilityMode::Development,
            PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
            RiftBuildAccess::Disabled,
            None,
            None,
            None,
        );
        let state = crate::state::ServerState::new();
        let lifecycle = state.lifecycle().await;

        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            launch(
                &context,
                &state,
                crate::parameters::decode(
                    json!({"dmb_path":root.join("missing.dmb"),"experiment_directory":root}),
                )
                .unwrap(),
            ),
        )
        .await;

        assert!(
            blocked.is_err(),
            "launch bypassed the runtime lifecycle lock"
        );
        drop(lifecycle);
        assert!(launch(
            &context,
            &state,
            crate::parameters::decode(
                json!({"dmb_path":root.join("missing.dmb"),"experiment_directory":root})
            )
            .unwrap()
        )
        .await
        .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn stop_waits_for_exclusive_runtime_lifecycle_access() {
        let root = std::env::temp_dir().join(format!(
            "meridian-tracy-stop-lifecycle-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let context = ToolExecutionContext::with_features(
            CapabilityMode::Development,
            PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
            RiftBuildAccess::Disabled,
            None,
            None,
            None,
        );
        let state = crate::state::ServerState::new();
        let lifecycle = state.lifecycle().await;

        let blocked =
            tokio::time::timeout(std::time::Duration::from_millis(20), stop(&context, &state))
                .await;

        assert!(blocked.is_err(), "stop bypassed the runtime lifecycle lock");
        drop(lifecycle);
        assert!(stop(&context, &state).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
