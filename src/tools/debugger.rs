use crate::limits::ServerLimits;
use crate::mcp::ToolResult;
use crate::outputs::*;
use crate::process::ProcessContainment;
use crate::result::{json_success, ToolMetadata};
use crate::spaceman::debugger::{
    AuxConnection, AuxRequest, AuxResponse, BreakpointReason, ContinueKind, DebuggerEventRecord,
    DebuggerInstallation, DebuggerLifecycle, DebuggerSession, InstructionRef, ProcRef,
    VariablesRef,
};
use crate::state::ServerState;
use crate::tools::ToolExecutionContext;
use anyhow::{anyhow, Result};
#[cfg(test)]
use serde_json::json;
use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::Command;

pub async fn launch(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DebugLaunchParams,
) -> Result<ToolResult> {
    let _lifecycle = state.lifecycle().await;
    if state.runtime().await.is_game_running() {
        return Err(anyhow!(
            "a DreamDaemon runtime is active; stop it before launching the debugger"
        ));
    }
    let dmb_path = args.dmb_path.as_str();
    let dmb_path = std::path::PathBuf::from(dmb_path);
    let runtime_id = state.new_runtime_id()?;
    let analysis = state
        .active_snapshot()
        .await
        .filter(|snapshot| snapshot.environment_path.with_extension("dmb") == dmb_path)
        .map(|snapshot| snapshot.identity());
    let generation = analysis.as_ref().map_or(0, |identity| identity.generation);
    let require_verified = args.require_verified_provenance.unwrap_or(false);
    let mut lease = match context
        .execution_lease(
            state,
            &dmb_path,
            dmb_path
                .parent()
                .ok_or_else(|| anyhow!("DMB has no parent"))?,
            "debugger",
            context.deadline(args.startup_timeout_ms.unwrap_or(60_000)),
        )
        .await?
    {
        Ok(lease) => lease,
        Err(result) => return Ok(result),
    };
    let launch_provenance =
        match super::require_launchable_artifact_owned(context, state, &dmb_path, require_verified)
            .await?
        {
            Ok(provenance) => provenance,
            Err(result) => return Ok(result),
        };
    let installation = context
        .debugger()
        .ok_or_else(|| anyhow!("auxtools debugger is unavailable"))?;
    let mut slot = state.debugger_checked().await?;
    if slot.is_some() {
        return Err(anyhow!("a debugger session is already active"));
    }
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
    let port = listener.local_addr()?.port();
    let host_mode = args
        .host_mode
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("interactive");
    let debugger_host = normalize_spawn_path(&debugger_host_executable(&args, installation)?);
    let dmb_spawn_path = normalize_spawn_path(&dmb_path);
    let working_directory = normalize_spawn_path(
        dmb_path
            .parent()
            .ok_or_else(|| anyhow!("DMB path has no parent"))?,
    );
    let memory_profile = args.memory_profile.unwrap_or(false);
    let memory_helper = memory_profile
        .then(crate::native_memory::installed_memory_helper)
        .transpose()?;
    let debugger_dll = normalize_spawn_path(
        memory_helper
            .as_ref()
            .map_or(&installation.debug_server_dll, |helper| &helper.path),
    );
    let dll_sha256 = memory_helper
        .as_ref()
        .map_or(&installation.dll_sha256, |helper| &helper.sha256);
    let mut command = Command::new(debugger_host);
    command
        .arg(dmb_spawn_path)
        .arg("-trusted")
        .current_dir(working_directory)
        .env_clear()
        .envs(dreamseeker_environment())
        .env("AUXTOOLS_DEBUG_MODE", "LAUNCHED")
        .env("AUXTOOLS_DEBUG_PORT", port.to_string())
        .env("AUXTOOLS_DEBUG_DLL", debugger_dll)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    context.checkpoint()?;
    context.progress(crate::request::Stage::Execution);
    lease.mark_writer_started();
    let (mut process, containment) = crate::process::spawn_runtime_process(&mut command)?;
    let mut startup = DebuggerStartup {
        containment: std::sync::Arc::clone(&containment),
        lease: Some(lease),
    };
    let mut essential = crate::result::MutationOutcome {
        operation_ran: true,
        process_started: true,
        runtime_id: Some(runtime_id.clone()),
        action: Some("debug_launch".into()),
        recovery_required: true,
        ..Default::default()
    };
    let handshake = async {
        let limits = ServerLimits::default();
        let accepted = tokio::time::timeout(
        Duration::from_millis(args.startup_timeout_ms.unwrap_or(limits.max_debug_startup_ms)),
        async {
            tokio::select! {
                accepted = listener.accept() => accepted.map_err(anyhow::Error::from),
                status = process.wait() => Err(anyhow!("debugger host exited before connecting: {}", status?)),
            }
        },
    )
    .await;
        let (stream, peer) = match accepted {
            Ok(Ok(value)) if value.1.ip().is_loopback() => value,
            Ok(Ok(_)) => {
                let _ = process.kill().await;
                return Err(anyhow!("debugger rejected a non-loopback connection"));
            }
            Ok(Err(error)) => {
                let _ = process.kill().await;
                return Err(error);
            }
            Err(_) => {
                let _ = process.kill().await;
                return Err(anyhow!("debugger startup timed out"));
            }
        };
        let _ = peer;
        let mut connection = AuxConnection::new(
            stream,
            limits.max_debug_message_bytes,
            Duration::from_millis(limits.max_debug_request_ms),
        );
        let stddef_source = match connection.request(AuxRequest::StdDef).await? {
            AuxResponse::StdDef(source) => source,
            response => return Err(anyhow!("unexpected StdDef response: {response:?}")),
        };
        if !matches!(
            connection.request(AuxRequest::Configured).await?,
            AuxResponse::Ack
        ) {
            let _ = process.kill().await;
            return Err(anyhow!("debugger configuration was not acknowledged"));
        }
        if memory_profile {
            let response = connection
                .request(AuxRequest::Eval {
                    frame_id: None,
                    command: "#meridian_memory_v1 {\"action\":\"status\"}".into(),
                    context: Some("repl".into()),
                })
                .await?;
            let AuxResponse::Eval(response) = response else {
                return Err(anyhow!("Native memory capability handshake failed"));
            };
            crate::native_memory::parse_response(&response.value)?;
        }
        Ok::<_, anyhow::Error>((connection, stddef_source))
    };
    let handshake = context.admit(handshake).await;
    let (connection, stddef_source) = match handshake {
        Ok(value) => value,
        Err(error) => {
            let _ = containment.request_termination();
            essential.process_stopped = matches!(
                tokio::time::timeout(Duration::from_secs(2), process.wait()).await,
                Ok(Ok(_))
            );
            essential.cleanup_complete =
                crate::process::wait_for_cleanup(&containment).await.is_ok();
            if essential.cleanup_complete {
                if let Some(mut lease) = startup.lease.take() {
                    let (result, lease) = state
                        .run_blocking_job(move || Ok((lease.finish(), lease)))
                        .await
                        .map_err(|error| {
                            let mut outcome = essential.clone();
                            outcome.cleanup_complete = false;
                            outcome.recovery_required = true;
                            crate::result::OutcomeError { error, outcome }
                        })?;
                    essential.cleanup_complete = result.is_ok();
                    if result.is_err() {
                        startup.lease = Some(lease);
                    }
                }
                if essential.cleanup_complete {
                    startup.lease = None;
                }
            }
            essential.recovery_required = !essential.cleanup_complete;
            return Err(crate::result::OutcomeError {
                error,
                outcome: essential,
            }
            .into());
        }
    };
    *slot = Some(DebuggerSession {
        runtime_id: runtime_id.clone(),
        analysis: analysis.clone(),
        lifecycle: DebuggerLifecycle::Running,
        process,
        connection,
        port,
        dmb_path: dmb_path.clone(),
        stddef_source,
        state_generation: generation,
        event_sequence: 0,
        last_exception: None,
        active_breakpoints: HashSet::new(),
        events: VecDeque::new(),
        dropped_events: 0,
        launch_provenance: launch_provenance.clone(),
        memory_helper_sha256: memory_helper.as_ref().map(|helper| helper.sha256.clone()),
        containment,
        execution_lease: startup.lease.take(),
    });
    Ok(json_success(
        session_metadata(generation, Some(runtime_id.clone()), analysis),
        DebugLaunchData {
            lifecycle: "running",
            host_mode: host_mode.into(),
            port,
            dmb_path,
            dll_sha256: dll_sha256.clone(),
            memory_profile,
            launch_provenance,
        },
    )
    .with_outcome(crate::result::MutationOutcome {
        operation_ran: true,
        cleanup_complete: true,
        process_started: true,
        runtime_id: Some(runtime_id),
        action: Some("launch".into()),
        ..Default::default()
    }))
}

fn debugger_host_executable(
    args: &crate::parameters::DebugLaunchParams,
    installation: &DebuggerInstallation,
) -> Result<PathBuf> {
    match args
        .host_mode
        .as_ref()
        .map(|mode| mode.as_str())
        .unwrap_or("interactive")
    {
        "interactive" => Ok(installation.dreamseeker.clone()),
        "headless" => Ok(installation.dreamdaemon.clone()),
        mode => Err(anyhow!(
            "unsupported host_mode {mode:?}; expected interactive or headless"
        )),
    }
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

fn dreamseeker_environment() -> Vec<(String, OsString)> {
    [
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramData",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
    .collect()
}

pub async fn stop(state: &ServerState) -> Result<ToolResult> {
    let _lifecycle = state.lifecycle().await;
    stop_with_lifecycle(state).await
}

pub(super) async fn stop_with_lifecycle(state: &ServerState) -> Result<ToolResult> {
    let mut slot = state.debugger_checked().await?;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let launch_provenance = session.launch_provenance.clone();
    let runtime_id = session.runtime_id.clone();
    let metadata = debugger_metadata(session);
    session.stop_with_pool(Some(state.blocking_pool())).await?;
    *slot = None;
    Ok(json_success(
        metadata,
        DebugStopData {
            lifecycle: "stopped",
            launch_provenance,
        },
    )
    .with_outcome(crate::result::MutationOutcome {
        operation_ran: true,
        cleanup_complete: true,
        process_stopped: true,
        runtime_id: Some(runtime_id),
        action: Some("stop".into()),
        ..Default::default()
    }))
}

struct DebuggerStartup {
    containment: std::sync::Arc<ProcessContainment>,
    lease: Option<crate::execution_lease::ExecutionLease>,
}
impl Drop for DebuggerStartup {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            crate::execution_lease::cleanup_dropped_writer(
                std::sync::Arc::clone(&self.containment),
                lease,
            );
        }
    }
}

fn session_metadata(
    generation: u64,
    runtime_id: Option<String>,
    analysis: Option<crate::identity::AnalysisIdentity>,
) -> ToolMetadata {
    let mut metadata = ToolMetadata::complete(Some(generation));
    metadata.runtime_id = runtime_id;
    metadata.analysis = analysis;
    metadata
}
fn debugger_metadata(session: &DebuggerSession) -> ToolMetadata {
    session_metadata(
        session.state_generation,
        Some(session.runtime_id.clone()),
        session.analysis.clone(),
    )
}

async fn request(state: &ServerState, request: AuxRequest) -> Result<(ToolMetadata, AuxResponse)> {
    request_controlled(None, state, request).await
}

async fn request_controlled(
    context: Option<&ToolExecutionContext>,
    state: &ServerState,
    request: AuxRequest,
) -> Result<(ToolMetadata, AuxResponse)> {
    let mut slot = match context {
        Some(context) => context.admit(state.debugger_checked()).await?,
        None => state.debugger_checked().await?,
    };
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    if let Some(context) = context {
        context.checkpoint()?;
    }
    let response = session.connection.request(request).await?;
    Ok((debugger_metadata(session), response))
}

pub async fn threads(state: &ServerState) -> Result<ToolResult> {
    let (generation, response) = request(state, AuxRequest::Stacks).await?;
    let AuxResponse::Stacks { mut stacks } = response else {
        return Err(anyhow!("unexpected stacks response"));
    };
    let mut budget = crate::outputs::budget::Budget::default();
    let mut omissions = crate::outputs::Omissions::default();
    stacks.truncate(10000);
    for row in &mut stacks {
        row.name = budget.text(&row.name, 4096, "threads.name", &mut omissions);
    }
    Ok(json_success(
        generation,
        DebugThreadsData { threads: stacks },
    ))
}

pub async fn stack_trace(
    state: &ServerState,
    args: crate::parameters::DebugStackTraceParams,
) -> Result<ToolResult> {
    let thread_id = args.thread_id as u32;
    let limits = ServerLimits::default();
    let start_frame = args.start_frame.map(|v| v as u32);
    let count = args
        .count
        .map(|value| value.min(limits.max_debug_frames as u64) as u32);
    let (generation, response) = request(
        state,
        AuxRequest::StackFrames {
            stack_id: thread_id,
            start_frame,
            count,
        },
    )
    .await?;
    let AuxResponse::StackFrames {
        frames,
        total_count,
    } = response
    else {
        return Err(anyhow!("unexpected stack frames response"));
    };
    let mut metadata = generation;
    metadata.truncated = frames.len() < total_count as usize;
    if metadata.truncated {
        metadata.truncation_reasons.push("debug_frame_limit".into());
    }
    Ok(json_success(
        metadata,
        DebugStackTraceData {
            frames,
            total_count,
        },
    ))
}

pub async fn scopes(
    state: &ServerState,
    args: crate::parameters::DebugScopesParams,
) -> Result<ToolResult> {
    let (generation, response) = request(
        state,
        AuxRequest::Scopes {
            frame_id: args.frame_id as u32,
        },
    )
    .await?;
    let AuxResponse::Scopes {
        arguments,
        locals,
        globals,
    } = response
    else {
        return Err(anyhow!("unexpected scopes response"));
    };
    Ok(json_success(
        generation,
        DebugScopesData {
            arguments,
            locals,
            globals,
        },
    ))
}

pub async fn variables(
    state: &ServerState,
    args: crate::parameters::DebugVariablesParams,
) -> Result<ToolResult> {
    let reference = args.variables_reference;
    let (generation, response) = request(
        state,
        AuxRequest::Variables {
            vars: VariablesRef(reference as i32),
        },
    )
    .await?;
    let AuxResponse::Variables { mut vars } = response else {
        return Err(anyhow!("unexpected variables response"));
    };
    let limit = ServerLimits::default().max_debug_variables;
    let truncated = vars.len() > limit;
    vars.truncate(limit);
    let mut metadata = generation;
    metadata.truncated = truncated;
    if truncated {
        metadata
            .truncation_reasons
            .push("debug_variable_limit".into());
    }
    let mut budget = crate::outputs::budget::Budget::default();
    let mut omissions = crate::outputs::Omissions::default();
    let mut retained = 0;
    for row in &mut vars {
        if retained > 0 && budget.bytes < 256 {
            break;
        }
        row.name = budget.text(&row.name, 4096, "variables.name", &mut omissions);
        row.value = budget.text(&row.value, 8192, "variables.value", &mut omissions);
        retained += 1;
    }
    vars.truncate(retained);
    Ok(json_success(
        metadata,
        DebugVariablesData { variables: vars },
    ))
}

pub async fn evaluate(
    execution: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DebugEvaluateParams,
) -> Result<ToolResult> {
    let expression = args.expression.clone();
    validate_expression(&expression)?;
    let context = args
        .context
        .as_ref()
        .map(|value| value.as_str())
        .map(str::to_owned);
    if context
        .as_deref()
        .is_some_and(|value| !matches!(value, "watch" | "repl" | "hover"))
    {
        return Err(anyhow!("unknown evaluation context"));
    }
    let (generation, response) = request_controlled(
        Some(execution),
        state,
        AuxRequest::Eval {
            frame_id: args.frame_id.map(|v| v as u32),
            command: expression,
            context,
        },
    )
    .await?;
    let AuxResponse::Eval(mut result) = response else {
        return Err(anyhow!("unexpected evaluation response"));
    };
    let original_bytes = result.value.len();
    result.value = crate::result::bounded_text(&result.value, 128 * 1024, 128 * 1024, false).into();
    let mut generation = generation;
    if result.value.len() < original_bytes {
        generation.truncated = true;
        generation
            .truncation_reasons
            .push("evaluation_value_bytes".into());
    }
    Ok(
        json_success(generation, DebugEvaluateData { result }).with_outcome(
            crate::result::MutationOutcome {
                operation_ran: true,
                cleanup_complete: true,
                action: Some("evaluate".into()),
                ..Default::default()
            },
        ),
    )
}

fn validate_expression(expression: &str) -> Result<()> {
    if expression.trim_start().starts_with('#') {
        return Err(anyhow!("Debugger console commands are not DreamMaker expressions; use explicit MCP controls such as dm_debug_memory"));
    }
    Ok(())
}

pub async fn memory(
    execution: &ToolExecutionContext,
    state: &ServerState,
    args: crate::native_memory::MemoryControl,
) -> Result<ToolResult> {
    let control = args;
    let mut slot = execution.admit(state.debugger_checked()).await?;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("No debugger session is active"))?;
    let hash = session.memory_helper_sha256.clone().ok_or_else(|| {
        anyhow!("Launch the debugger with memory_profile: true before using native memory controls")
    })?;
    execution.checkpoint()?;
    let response = session
        .connection
        .request(AuxRequest::Eval {
            frame_id: None,
            command: control.command()?,
            context: Some("repl".into()),
        })
        .await?;
    let AuxResponse::Eval(response) = response else {
        return Err(anyhow!("Unexpected native memory response"));
    };
    let mut evidence: NativeMemoryEnvelope =
        serde_json::from_value(crate::native_memory::parse_response(&response.value)?)?;
    let mut metadata = debugger_metadata(session);
    if let NativeMemoryResult::Stop(stop) = &mut evidence.evidence.result {
        let mut bytes = crate::outputs::budget::DETAIL_BYTES;
        let mut retained = 0;
        let mut clipped = false;
        for row in &mut stop.procedures {
            if let Some(path) = &mut row.proc_path {
                let bounded = crate::result::bounded_text(path, 256, 256, false);
                if bounded.len() < path.len() {
                    row.proc_path_truncated = true;
                    clipped = true;
                }
                *path = bounded.into();
            }
            let size = crate::result::encoded_bytes(row, usize::MAX).unwrap_or(usize::MAX);
            if size > bytes {
                break;
            }
            bytes -= size;
            retained += 1;
        }
        if retained < stop.procedures.len() || clipped {
            stop.rows_truncated = true;
            metadata.truncated = true;
            metadata
                .truncation_reasons
                .push("native_memory_response_bytes".into());
        }
        stop.procedures.truncate(retained);

        for (present, reason) in [
            (stop.rows_truncated, "native_memory_row_limit"),
            (stop.capacity_exceeded, "native_memory_record_limit"),
        ] {
            if present {
                metadata.truncated = true;
                metadata.truncation_reasons.push(reason.into());
            }
        }
    }
    Ok(json_success(metadata,DebugMemoryData {native_memory:evidence,helper_sha256:hash,helper_source_revision:crate::native_memory::SOURCE_REVISION,launch_provenance:session.launch_provenance.clone(),warning:"Observed requested allocation bytes are not retained-object sizes or proof of a leak. Cross-thread frees are not observed."}).with_outcome(crate::result::MutationOutcome {operation_ran:true,cleanup_complete:true,action:Some("memory".into()),..Default::default()}))
}

pub async fn set_exception_breakpoints(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DebugSetExceptionBreakpointsParams,
) -> Result<ToolResult> {
    let enabled = args.break_on_runtimes;
    let mut slot = context.admit(state.debugger_checked()).await?;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let generation = debugger_metadata(session);
    context.checkpoint()?;
    session
        .connection
        .send(AuxRequest::CatchRuntimes {
            should_catch: enabled,
        })
        .await?;
    Ok(json_success(
        generation,
        DebugExceptionBreakpointsData {
            break_on_runtimes: enabled,
        },
    )
    .with_outcome(crate::result::MutationOutcome {
        operation_ran: true,
        cleanup_complete: true,
        action: Some("exception_breakpoints".into()),
        ..Default::default()
    }))
}

pub async fn control(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DebugControlParams,
) -> Result<ToolResult> {
    let action = args.action.as_str();
    let thread = args.thread_id.map(|v| v as u32);
    let request_value = match action {
        "pause" => AuxRequest::Pause,
        "continue" => AuxRequest::Continue {
            kind: ContinueKind::Continue,
        },
        "step_in" => AuxRequest::Continue {
            kind: ContinueKind::StepInto {
                stack_id: thread.ok_or_else(|| anyhow!("step_in requires thread_id"))?,
            },
        },
        "step_over" => AuxRequest::Continue {
            kind: ContinueKind::StepOver {
                stack_id: thread.ok_or_else(|| anyhow!("step_over requires thread_id"))?,
            },
        },
        "step_out" => AuxRequest::Continue {
            kind: ContinueKind::StepOut {
                stack_id: thread.ok_or_else(|| anyhow!("step_out requires thread_id"))?,
            },
        },
        _ => return Err(anyhow!("unknown debugger control action")),
    };
    context.checkpoint()?;
    let (generation, response) = request_controlled(Some(context), state, request_value).await?;
    if !matches!(response, AuxResponse::Ack) {
        return Err(anyhow!("debugger control was not acknowledged"));
    }
    Ok(json_success(
        generation,
        DebugControlData {
            action: action.into(),
        },
    )
    .with_outcome(crate::result::MutationOutcome {
        operation_ran: true,
        cleanup_complete: true,
        action: Some(action.into()),
        ..Default::default()
    }))
}

pub async fn set_function_breakpoints(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DebugSetFunctionBreakpointsParams,
) -> Result<ToolResult> {
    let breakpoints = &args.breakpoints;
    if breakpoints.len() > 10_000 {
        return Err(anyhow!("breakpoint limit exceeded"));
    }
    let mut desired = Vec::new();
    for breakpoint in breakpoints {
        let path = breakpoint.proc_path.clone();
        if !path.starts_with('/') {
            return Err(anyhow!("proc_path must be canonical"));
        }
        let condition = breakpoint.condition.clone();
        desired.push((
            InstructionRef {
                proc: ProcRef {
                    path,
                    override_id: breakpoint.override_id.unwrap_or(0) as u32,
                },
                offset: breakpoint.offset.unwrap_or(0) as u32,
            },
            condition,
        ));
    }
    replace_breakpoints_controlled(Some(context), state, desired).await
}

pub async fn set_breakpoints(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DebugSetBreakpointsParams,
) -> Result<ToolResult> {
    let source_path = std::path::PathBuf::from(args.source_path.as_str());
    let snapshot = state.snapshot().await?;
    let breakpoints = &args.breakpoints;
    if breakpoints.len() > 10_000 {
        return Err(anyhow!("breakpoint limit exceeded"));
    }
    let mut slot = context.admit(state.debugger_checked()).await?;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    if session
        .analysis
        .as_ref()
        .map(|identity| identity.snapshot_id.as_str())
        != Some(snapshot.snapshot_id.as_str())
    {
        return Err(anyhow!("debugger session uses a stale analysis generation"));
    }
    let mut desired = Vec::new();
    for breakpoint in breakpoints {
        context.checkpoint()?;
        let line = breakpoint.line as u32;
        let symbol = snapshot
            .language_index
            .proc_at(&source_path, line)
            .ok_or_else(|| anyhow!("breakpoint line is not inside a parsed procedure"))?;
        let crate::index::SymbolId::Proc {
            owner,
            name,
            override_index,
        } = &symbol.id
        else {
            unreachable!()
        };
        let proc = ProcRef {
            path: format!("{owner}/proc/{name}"),
            override_id: *override_index as u32,
        };
        let response = session
            .connection
            .request(AuxRequest::Offset {
                proc: proc.clone(),
                line,
            })
            .await?;
        let AuxResponse::Offset {
            offset: Some(offset),
        } = response
        else {
            return Err(anyhow!("auxtools could not resolve breakpoint line {line}"));
        };
        desired.push((
            InstructionRef { proc, offset },
            breakpoint.condition.clone(),
        ));
    }
    replace_breakpoints_in_session(Some(context), session, desired).await
}

async fn replace_breakpoints_controlled(
    context: Option<&ToolExecutionContext>,
    state: &ServerState,
    desired: Vec<(InstructionRef, Option<String>)>,
) -> Result<ToolResult> {
    let mut slot = match context {
        Some(context) => context.admit(state.debugger_checked()).await?,
        None => state.debugger_checked().await?,
    };
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    if let Some(snapshot) = state.admitted_analysis() {
        if session
            .analysis
            .as_ref()
            .map(|identity| identity.snapshot_id.as_str())
            != Some(snapshot.snapshot_id.as_str())
        {
            return Err(crate::identity::StaleIdentity {
                field: "expected_snapshot",
                expected: snapshot.snapshot_id.clone(),
                current: session
                    .analysis
                    .as_ref()
                    .map(|identity| identity.snapshot_id.clone()),
            }
            .into());
        }
    }
    if let Some(context) = context {
        context.checkpoint()?;
    }
    replace_breakpoints_in_session(context, session, desired).await
}

async fn replace_breakpoints_in_session(
    context: Option<&ToolExecutionContext>,
    session: &mut DebuggerSession,
    desired: Vec<(InstructionRef, Option<String>)>,
) -> Result<ToolResult> {
    let desired_set = desired
        .iter()
        .map(|(instruction, _)| instruction.clone())
        .collect::<HashSet<_>>();
    let removed = session
        .active_breakpoints
        .difference(&desired_set)
        .cloned()
        .collect::<Vec<_>>();
    let mut essential = crate::result::MutationOutcome {
        operation_ran: true,
        cleanup_complete: true,
        action: Some("replace_breakpoints".into()),
        runtime_id: Some(session.runtime_id.clone()),
        ..Default::default()
    };
    for instruction in removed {
        if let Some(context) = context {
            context
                .checkpoint()
                .map_err(|error| crate::result::OutcomeError {
                    error,
                    outcome: essential.clone(),
                })?;
        }
        let response = session
            .connection
            .request(AuxRequest::BreakpointUnset {
                instruction: instruction.clone(),
            })
            .await
            .map_err(|error| crate::result::OutcomeError {
                error: error.into(),
                outcome: essential.clone(),
            })?;
        if !matches!(response, AuxResponse::BreakpointUnset { success: true }) {
            essential.recovery_required = true;
            return Err(crate::result::OutcomeError {
                error: anyhow!("auxtools failed to remove a stale breakpoint"),
                outcome: essential,
            }
            .into());
        }
        session.active_breakpoints.remove(&instruction);
        essential.removed_breakpoints += 1;
    }
    let mut results = Vec::new();
    for (index, (instruction, condition)) in desired.into_iter().enumerate() {
        essential.failed_request_index = Some(index);
        if let Some(context) = context {
            context
                .checkpoint()
                .map_err(|error| crate::result::OutcomeError {
                    error,
                    outcome: essential.clone(),
                })?;
        }
        let response = session
            .connection
            .request(AuxRequest::BreakpointSet {
                instruction: instruction.clone(),
                condition,
            })
            .await
            .map_err(|error| crate::result::OutcomeError {
                error: error.into(),
                outcome: essential.clone(),
            })?;
        let verified = matches!(
            response,
            AuxResponse::BreakpointSet {
                result: crate::spaceman::debugger::BreakpointSetResult::Success { .. }
            }
        );
        if verified {
            session.active_breakpoints.insert(instruction.clone());
            essential.verified_request_indices.push(index);
        } else {
            essential.unverified_request_indices.push(index);
        }
        results.push(DebugBreakpoint {
            instruction,
            verified,
        });
    }
    essential.failed_request_index = None;
    Ok(json_success(
        debugger_metadata(session),
        DebugBreakpointsData {
            breakpoints: results,
        },
    )
    .with_outcome(essential))
}

pub async fn exception_info(state: &ServerState) -> Result<ToolResult> {
    let slot = state.debugger_checked().await?;
    let session = slot
        .as_ref()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    Ok(json_success(
        debugger_metadata(session),
        DebugExceptionInfoData {
            message: session.last_exception.clone(),
            sequence: session.event_sequence,
        },
    ))
}

pub async fn source(
    state: &ServerState,
    args: crate::parameters::DebugSourceParams,
) -> Result<ToolResult> {
    if args.source_reference != 1 {
        return Err(anyhow!("unknown debugger source reference"));
    }
    let slot = state.debugger_checked().await?;
    let session = slot
        .as_ref()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let source = session.stddef_source.as_deref().unwrap_or("");
    if source.len() > ServerLimits::default().max_debug_output_bytes {
        return Err(anyhow!("debugger source exceeds output limit"));
    }
    Ok(json_success(
        debugger_metadata(session),
        DebugSourceData {
            source_reference: 1,
            name: "stddef.dm",
            content: crate::result::bounded_text(source, 128 * 1024, 128 * 1024, false).into(),
            source_origin: "native_debugger_stddef",
        },
    ))
}

pub async fn wait_for_event(
    state: &ServerState,
    args: crate::parameters::DebugWaitForEventParams,
) -> Result<ToolResult> {
    let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(30_000));
    let after_sequence = args.after_sequence.unwrap_or(0);
    let kinds = args
        .kinds
        .as_ref()
        .map(|values| {
            values
                .iter()
                .map(|kind| kind.as_str())
                .map(str::to_owned)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut slot = state.debugger_checked().await?;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    if let Some(event) = session
        .events
        .iter()
        .find(|event| {
            event.sequence > after_sequence && (kinds.is_empty() || kinds.contains(&event.kind))
        })
        .cloned()
    {
        return event_result(session, Some(event), false);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return event_result(session, None, true);
        }
        let response = match session.connection.next_event(remaining).await {
            Ok(response) => response,
            Err(crate::spaceman::debugger::AuxProtocolError::Timeout) => {
                return event_result(session, None, true);
            }
            Err(error) => return Err(error.into()),
        };
        let event = debugger_event(session, response)?;
        if session.events.len() >= ServerLimits::default().max_debug_events {
            session.events.pop_front();
            session.dropped_events = session.dropped_events.saturating_add(1);
        }
        session.events.push_back(event.clone());
        if event.sequence > after_sequence && (kinds.is_empty() || kinds.contains(&event.kind)) {
            return event_result(session, Some(event), false);
        }
    }
}

fn debugger_event(
    session: &mut DebuggerSession,
    response: AuxResponse,
) -> Result<DebuggerEventRecord> {
    session.event_sequence = session.event_sequence.saturating_add(1);
    let (kind, message) = match response {
        AuxResponse::Notification { message } => ("output", Some(message)),
        AuxResponse::BreakpointHit {
            reason: BreakpointReason::Breakpoint,
        } => ("breakpoint", None),
        AuxResponse::BreakpointHit {
            reason: BreakpointReason::Step,
        } => ("step", None),
        AuxResponse::BreakpointHit {
            reason: BreakpointReason::Pause,
        } => ("pause", None),
        AuxResponse::BreakpointHit {
            reason: BreakpointReason::Runtime(message),
        } => {
            session.last_exception = Some(message.clone());
            ("runtime", Some(message))
        }
        AuxResponse::Disconnect => ("terminated", None),
        other => return Err(anyhow!("unexpected debugger event: {other:?}")),
    };
    Ok(DebuggerEventRecord {
        sequence: session.event_sequence,
        kind: kind.to_owned(),
        message,
    })
}

fn event_result(
    session: &DebuggerSession,
    event: Option<DebuggerEventRecord>,
    timed_out: bool,
) -> Result<ToolResult> {
    Ok(json_success(
        debugger_metadata(session),
        DebugEventData {
            event,
            timed_out,
            dropped_events: session
                .dropped_events
                .saturating_add(session.connection.dropped_events()),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_expectation_stays_out_of_the_native_memory_command() {
        let request: crate::parameters::DebugMemoryParams = crate::parameters::decode(
            json!({"action":"status","expected_runtime":format!("r1:{}", "a".repeat(64))}),
        )
        .unwrap();
        let command = request.into_control().command().unwrap();
        assert!(!command.contains("expected_runtime"));
        let native: serde_json::Value =
            serde_json::from_str(command.strip_prefix("#meridian_memory_v1 ").unwrap()).unwrap();
        assert_eq!(native["action"], "status");
        assert_eq!(native["duration_ms"], 10000);
        assert_eq!(native["max_records"], 20000);
        assert_eq!(native["row_limit"], 100);
    }

    #[tokio::test]
    async fn stale_debugger_control_cannot_reach_the_replacement_connection() {
        use tokio::io::AsyncReadExt;
        let (root, collector) = crate::tracy_collector::tests::owned_fixture().await;
        let _ = collector.stop(Duration::from_millis(100)).await;
        assert!(collector.cleanup_confirmed());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u32_le().await.unwrap();
            let mut bytes = vec![0; length as usize];
            stream.read_exact(&mut bytes).await.unwrap();
            let request: AuxRequest = bincode::deserialize(&bytes).unwrap();
            assert!(
                matches!(request, AuxRequest::Disconnect),
                "a stale control reached the replacement"
            );
        });
        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut command =
            Command::new(root.join(format!("collector{}", std::env::consts::EXE_SUFFIX)));
        command
            .env("COLLECTOR_MODE", "blocked")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (process, containment) = crate::process::spawn_runtime_process(&mut command).unwrap();
        let state = ServerState::new();
        let first = state.new_runtime_id().unwrap();
        let second = state.new_runtime_id().unwrap();
        *state.debugger().await = Some(DebuggerSession {
            runtime_id: second.clone(),
            analysis: None,
            lifecycle: DebuggerLifecycle::Running,
            process,
            containment,
            connection: AuxConnection::new(stream, 8192, Duration::from_secs(1)),
            port: address.port(),
            dmb_path: root.join("fixture.dmb"),
            stddef_source: None,
            state_generation: 0,
            event_sequence: 0,
            last_exception: None,
            active_breakpoints: HashSet::new(),
            events: VecDeque::new(),
            dropped_events: 0,
            launch_provenance: crate::LaunchProvenance {
                status: crate::ProvenanceStatus::Unverified,
                build_record_id: None,
                dmb_sha256: "00".repeat(32),
                warnings: vec![],
            },
            memory_helper_sha256: None,
            execution_lease: None,
        });
        let context = ToolExecutionContext::with_features(
            crate::CapabilityMode::Development,
            crate::PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
            crate::RiftBuildAccess::Disabled,
            None,
            Some(debugger_installation_fixture()),
            None,
        );
        let stale = crate::tools::call_tool(
            &context,
            &state,
            "dm_debug_control",
            json!({"action":"pause","expected_runtime":first}),
        )
        .await
        .unwrap();
        assert_eq!(stale.is_error, Some(true));
        let source = root.join("fixture.dme");
        std::fs::write(&source, "/datum/stale_control\n").unwrap();
        crate::tools::call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":source}),
        )
        .await
        .unwrap();
        let snapshot = state.snapshot().await.unwrap();
        let stale = crate::tools::call_tool(&context, &state, "dm_debug_set_breakpoints", json!({"source_path":source,"breakpoints":[],"expected_snapshot":snapshot.snapshot_id,"expected_runtime":first})).await.unwrap();
        assert_eq!(stale.is_error, Some(true));
        assert_eq!(stale.meta.unwrap().analysis, snapshot.identity());
        let bound = state.for_request(None, Some(Some(first)));
        assert!(control(
            &context,
            &bound,
            crate::parameters::decode(json!({"action":"pause"})).unwrap()
        )
        .await
        .unwrap_err()
        .is::<crate::identity::StaleIdentity>());
        assert_eq!(state.debugger().await.as_ref().unwrap().runtime_id, second);
        stop(&state).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), peer)
            .await
            .unwrap()
            .unwrap();
        drop(collector);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn expression_evaluation_cannot_bypass_explicit_memory_controls() {
        for command in [
            "#mem_profiler begin elsewhere",
            "  #meridian_memory_v1 {}",
            "\n#help",
        ] {
            assert!(validate_expression(command).is_err());
        }
        assert!(validate_expression("memory_allocate()").is_ok());
        assert!(validate_expression("\"#literal\"").is_ok());
    }

    fn debugger_installation_fixture() -> DebuggerInstallation {
        DebuggerInstallation {
            dreamseeker: PathBuf::from("dreamseeker.exe"),
            dreamdaemon: PathBuf::from("dreamdaemon.exe"),
            debug_server_dll: PathBuf::from("debug_server.dll"),
            dll_sha256: "fixture".to_owned(),
        }
    }

    #[test]
    fn debugger_host_defaults_to_interactive_dreamseeker() {
        let executable = debugger_host_executable(
            &crate::parameters::decode(json!({"dmb_path":"fixture.dmb"})).unwrap(),
            &debugger_installation_fixture(),
        )
        .expect("the default debugger host should be valid");

        assert_eq!(executable, PathBuf::from("dreamseeker.exe"));
    }

    #[test]
    fn debugger_host_supports_headless_dreamdaemon() {
        let executable = debugger_host_executable(
            &crate::parameters::decode(json!({"dmb_path":"fixture.dmb","host_mode":"headless"}))
                .unwrap(),
            &debugger_installation_fixture(),
        )
        .expect("the headless debugger host should be valid");

        assert_eq!(executable, PathBuf::from("dreamdaemon.exe"));
    }

    #[test]
    fn debugger_host_rejects_unknown_modes() {
        let error = crate::parameters::decode::<crate::parameters::DebugLaunchParams>(
            json!({"dmb_path":"fixture.dmb","host_mode":"detached"}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("host_mode"));
    }

    #[test]
    fn dreamseeker_environment_retains_system_runtime_without_credentials() {
        let environment = dreamseeker_environment();
        if cfg!(windows) {
            assert!(environment.iter().any(|(name, _)| name == "SystemRoot"));
        }
        for (name, _) in environment {
            let name = name.to_ascii_lowercase();
            assert!(!name.contains("token"));
            assert!(!name.contains("secret"));
            assert!(!name.contains("password"));
            assert!(!name.contains("authorization"));
            assert!(!name.contains("cookie"));
        }
    }

    #[test]
    fn debugger_normalizes_windows_extended_paths_for_byond() {
        if cfg!(windows) {
            assert_eq!(
                normalize_spawn_path(Path::new(r"\\?\C:\byond\debug_server.dll")),
                PathBuf::from(r"C:\byond\debug_server.dll")
            );
            assert_eq!(
                normalize_spawn_path(Path::new(r"\\?\UNC\server\share\world.dmb")),
                PathBuf::from(r"\\server\share\world.dmb")
            );
        }
    }
}
