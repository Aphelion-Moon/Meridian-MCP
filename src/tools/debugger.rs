use crate::limits::ServerLimits;
use crate::mcp::ToolResult;
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
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::Command;

fn duration(args: &Value, key: &str, default: u64, maximum: u64) -> Duration {
    Duration::from_millis(
        args.get(key)
            .and_then(Value::as_u64)
            .unwrap_or(default)
            .min(maximum),
    )
}

pub async fn launch(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let _lifecycle = state.lifecycle().await;
    if state.runtime().await.is_game_running() {
        return Err(anyhow!(
            "a DreamDaemon runtime is active; stop it before launching the debugger"
        ));
    }
    let dmb_path = args
        .get("dmb_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing dmb_path"))?;
    let dmb_path = std::path::PathBuf::from(dmb_path);
    let require_verified = args
        .get("require_verified_provenance")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let launch_provenance =
        match super::require_launchable_artifact(context, &dmb_path, require_verified) {
            Ok(provenance) => provenance,
            Err(result) => return Ok(result),
        };
    let installation = context
        .debugger()
        .ok_or_else(|| anyhow!("auxtools debugger is unavailable"))?;
    let mut slot = state.debugger().await;
    if slot.is_some() {
        return Err(anyhow!("a debugger session is already active"));
    }
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
    let port = listener.local_addr()?.port();
    let host_mode = args
        .get("host_mode")
        .and_then(Value::as_str)
        .unwrap_or("interactive");
    let debugger_host = normalize_spawn_path(&debugger_host_executable(&args, installation)?);
    let dmb_spawn_path = normalize_spawn_path(&dmb_path);
    let working_directory = normalize_spawn_path(
        dmb_path
            .parent()
            .ok_or_else(|| anyhow!("DMB path has no parent"))?,
    );
    let memory_profile = match args.get("memory_profile") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| anyhow!("memory_profile must be a boolean"))?,
    };
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
    let containment = ProcessContainment::new()?;
    let mut process = command.spawn()?;
    if let Err(error) = containment.assign(process.id().unwrap_or_default()) {
        let _ = process.kill().await;
        return Err(error.context("refusing to run the debugger host outside process containment"));
    }
    let limits = ServerLimits::default();
    let accepted = tokio::time::timeout(
        duration(
            &args,
            "startup_timeout_ms",
            limits.max_debug_startup_ms,
            limits.max_debug_startup_ms,
        ),
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
    let generation = state.state_generation().await;
    *slot = Some(DebuggerSession {
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
    });
    Ok(json_success(
        ToolMetadata::complete(Some(generation)),
        json!({"lifecycle":"running","host_mode":host_mode,"port":port,"dmb_path":dmb_path,"dll_sha256":dll_sha256,"memory_profile":memory_profile,"launch_provenance":launch_provenance}),
    ))
}

fn debugger_host_executable(args: &Value, installation: &DebuggerInstallation) -> Result<PathBuf> {
    match args
        .get("host_mode")
        .and_then(Value::as_str)
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
    let session = state
        .debugger()
        .await
        .take()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let generation = session.state_generation;
    let launch_provenance = session.launch_provenance.clone();
    session.stop().await?;
    Ok(json_success(
        ToolMetadata::complete(Some(generation)),
        json!({"lifecycle":"stopped","launch_provenance":launch_provenance}),
    ))
}

async fn request(state: &ServerState, request: AuxRequest) -> Result<(u64, AuxResponse)> {
    let mut slot = state.debugger().await;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let response = session.connection.request(request).await?;
    Ok((session.state_generation, response))
}

pub async fn threads(state: &ServerState) -> Result<ToolResult> {
    let (generation, response) = request(state, AuxRequest::Stacks).await?;
    let AuxResponse::Stacks { stacks } = response else {
        return Err(anyhow!("unexpected stacks response"));
    };
    Ok(json_success(
        ToolMetadata::complete(Some(generation)),
        json!({"threads":stacks}),
    ))
}

pub async fn stack_trace(state: &ServerState, args: Value) -> Result<ToolResult> {
    let thread_id = required_u32(&args, "thread_id")?;
    let limits = ServerLimits::default();
    let start_frame = args
        .get("start_frame")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let count = args
        .get("count")
        .and_then(Value::as_u64)
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
    let mut metadata = ToolMetadata::complete(Some(generation));
    metadata.truncated = frames.len() < total_count as usize;
    if metadata.truncated {
        metadata.truncation_reasons.push("debug_frame_limit".into());
    }
    Ok(json_success(
        metadata,
        json!({"frames":frames,"total_count":total_count}),
    ))
}

pub async fn scopes(state: &ServerState, args: Value) -> Result<ToolResult> {
    let (generation, response) = request(
        state,
        AuxRequest::Scopes {
            frame_id: required_u32(&args, "frame_id")?,
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
        ToolMetadata::complete(Some(generation)),
        json!({"arguments":arguments,"locals":locals,"globals":globals}),
    ))
}

pub async fn variables(state: &ServerState, args: Value) -> Result<ToolResult> {
    let reference = args
        .get("variables_reference")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("Missing variables_reference"))?;
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
    let mut metadata = ToolMetadata::complete(Some(generation));
    metadata.truncated = truncated;
    if truncated {
        metadata
            .truncation_reasons
            .push("debug_variable_limit".into());
    }
    Ok(json_success(metadata, json!({"variables":vars})))
}

pub async fn evaluate(state: &ServerState, args: Value) -> Result<ToolResult> {
    let expression = required_string(&args, "expression", 16_384)?;
    validate_expression(&expression)?;
    let context = args
        .get("context")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if context
        .as_deref()
        .is_some_and(|value| !matches!(value, "watch" | "repl" | "hover"))
    {
        return Err(anyhow!("unknown evaluation context"));
    }
    let (generation, response) = request(
        state,
        AuxRequest::Eval {
            frame_id: args
                .get("frame_id")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            command: expression,
            context,
        },
    )
    .await?;
    let AuxResponse::Eval(result) = response else {
        return Err(anyhow!("unexpected evaluation response"));
    };
    Ok(json_success(
        ToolMetadata::complete(Some(generation)),
        json!({"result":result}),
    ))
}

fn validate_expression(expression: &str) -> Result<()> {
    if expression.trim_start().starts_with('#') {
        return Err(anyhow!("Debugger console commands are not DreamMaker expressions; use explicit MCP controls such as dm_debug_memory"));
    }
    Ok(())
}

pub async fn memory(state: &ServerState, args: Value) -> Result<ToolResult> {
    let control = crate::native_memory::MemoryControl::parse(args)?;
    let mut slot = state.debugger().await;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("No debugger session is active"))?;
    let hash = session.memory_helper_sha256.clone().ok_or_else(|| {
        anyhow!("Launch the debugger with memory_profile: true before using native memory controls")
    })?;
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
    let evidence = crate::native_memory::parse_response(&response.value)?;
    let mut metadata = ToolMetadata::complete(Some(session.state_generation));
    for (key, reason) in [
        ("rows_truncated", "native_memory_row_limit"),
        ("capacity_exceeded", "native_memory_record_limit"),
    ] {
        if evidence["evidence"]["result"][key] == true {
            metadata.truncated = true;
            metadata.truncation_reasons.push(reason.into());
        }
    }
    Ok(json_success(
        metadata,
        json!({
            "native_memory": evidence, "helper_sha256": hash,
            "helper_source_revision": crate::native_memory::SOURCE_REVISION,
            "launch_provenance":session.launch_provenance,
            "warning":"Observed requested allocation bytes are not retained-object sizes or proof of a leak. Cross-thread frees are not observed."
        }),
    ))
}

pub async fn set_exception_breakpoints(state: &ServerState, args: Value) -> Result<ToolResult> {
    let enabled = args
        .get("break_on_runtimes")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("Missing break_on_runtimes"))?;
    let mut slot = state.debugger().await;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let generation = session.state_generation;
    session
        .connection
        .send(AuxRequest::CatchRuntimes {
            should_catch: enabled,
        })
        .await?;
    Ok(json_success(
        ToolMetadata::complete(Some(generation)),
        json!({"break_on_runtimes":enabled}),
    ))
}

pub async fn control(state: &ServerState, args: Value) -> Result<ToolResult> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing action"))?;
    let thread = args
        .get("thread_id")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
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
    let (generation, response) = request(state, request_value).await?;
    if !matches!(response, AuxResponse::Ack) {
        return Err(anyhow!("debugger control was not acknowledged"));
    }
    Ok(json_success(
        ToolMetadata::complete(Some(generation)),
        json!({"action":action}),
    ))
}

pub async fn set_function_breakpoints(state: &ServerState, args: Value) -> Result<ToolResult> {
    let breakpoints = args
        .get("breakpoints")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("breakpoints must be an array"))?;
    if breakpoints.len() > 10_000 {
        return Err(anyhow!("breakpoint limit exceeded"));
    }
    let mut desired = Vec::new();
    for breakpoint in breakpoints {
        let path = required_string(breakpoint, "proc_path", 4096)?;
        if !path.starts_with('/') {
            return Err(anyhow!("proc_path must be canonical"));
        }
        let condition = optional_string(breakpoint, "condition", 4096)?;
        desired.push((
            InstructionRef {
                proc: ProcRef {
                    path,
                    override_id: breakpoint
                        .get("override_id")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                },
                offset: breakpoint
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
            },
            condition,
        ));
    }
    replace_breakpoints(state, desired).await
}

pub async fn set_breakpoints(state: &ServerState, args: Value) -> Result<ToolResult> {
    let source_path = std::path::PathBuf::from(
        args.get("source_path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Missing source_path"))?,
    );
    let snapshot = state.snapshot().await?;
    let breakpoints = args
        .get("breakpoints")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("breakpoints must be an array"))?;
    if breakpoints.len() > 10_000 {
        return Err(anyhow!("breakpoint limit exceeded"));
    }
    let mut slot = state.debugger().await;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    if session.state_generation != snapshot.generation {
        return Err(anyhow!("debugger session uses a stale analysis generation"));
    }
    let mut desired = Vec::new();
    for breakpoint in breakpoints {
        let line = required_u32(breakpoint, "line")?;
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
            optional_string(breakpoint, "condition", 4096)?,
        ));
    }
    replace_breakpoints_in_session(session, desired).await
}

async fn replace_breakpoints(
    state: &ServerState,
    desired: Vec<(InstructionRef, Option<String>)>,
) -> Result<ToolResult> {
    let mut slot = state.debugger().await;
    let session = slot
        .as_mut()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    replace_breakpoints_in_session(session, desired).await
}

async fn replace_breakpoints_in_session(
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
    for instruction in removed {
        let response = session
            .connection
            .request(AuxRequest::BreakpointUnset { instruction })
            .await?;
        if !matches!(response, AuxResponse::BreakpointUnset { success: true }) {
            return Err(anyhow!("auxtools failed to remove a stale breakpoint"));
        }
    }
    let mut results = Vec::new();
    let mut installed = HashSet::new();
    for (instruction, condition) in desired {
        let response = session
            .connection
            .request(AuxRequest::BreakpointSet {
                instruction: instruction.clone(),
                condition,
            })
            .await?;
        let verified = matches!(
            response,
            AuxResponse::BreakpointSet {
                result: crate::spaceman::debugger::BreakpointSetResult::Success { .. }
            }
        );
        if verified {
            installed.insert(instruction.clone());
        }
        results.push(json!({"instruction":instruction,"verified":verified}));
    }
    session.active_breakpoints = installed;
    Ok(json_success(
        ToolMetadata::complete(Some(session.state_generation)),
        json!({"breakpoints":results}),
    ))
}

pub async fn exception_info(state: &ServerState) -> Result<ToolResult> {
    let slot = state.debugger().await;
    let session = slot
        .as_ref()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    Ok(json_success(
        ToolMetadata::complete(Some(session.state_generation)),
        json!({"message":session.last_exception,"sequence":session.event_sequence}),
    ))
}

pub async fn source(state: &ServerState, args: Value) -> Result<ToolResult> {
    if args.get("source_reference").and_then(Value::as_u64) != Some(1) {
        return Err(anyhow!("unknown debugger source reference"));
    }
    let slot = state.debugger().await;
    let session = slot
        .as_ref()
        .ok_or_else(|| anyhow!("no debugger session is active"))?;
    let source = session.stddef_source.as_deref().unwrap_or("");
    if source.len() > ServerLimits::default().max_debug_output_bytes {
        return Err(anyhow!("debugger source exceeds output limit"));
    }
    Ok(json_success(
        ToolMetadata::complete(Some(session.state_generation)),
        json!({"source_reference":1,"name":"stddef.dm","content":source}),
    ))
}

pub async fn wait_for_event(state: &ServerState, args: Value) -> Result<ToolResult> {
    let timeout = duration(&args, "timeout_ms", 30_000, 300_000);
    let after_sequence = args
        .get("after_sequence")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let kinds = args
        .get("kinds")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut slot = state.debugger().await;
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
        ToolMetadata::complete(Some(session.state_generation)),
        json!({"event":event,"timed_out":timed_out,"dropped_events":session.dropped_events}),
    ))
}

fn required_u32(args: &Value, key: &str) -> Result<u32> {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .ok_or_else(|| anyhow!("Missing {key}"))
}

fn required_string(args: &Value, key: &str, maximum: usize) -> Result<String> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing {key}"))?;
    if value.len() > maximum {
        return Err(anyhow!("{key} exceeds maximum length"));
    }
    Ok(value.to_owned())
}

fn optional_string(args: &Value, key: &str, maximum: usize) -> Result<Option<String>> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|_| required_string(args, key, maximum))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let executable = debugger_host_executable(&json!({}), &debugger_installation_fixture())
            .expect("the default debugger host should be valid");

        assert_eq!(executable, PathBuf::from("dreamseeker.exe"));
    }

    #[test]
    fn debugger_host_supports_headless_dreamdaemon() {
        let executable = debugger_host_executable(
            &json!({"host_mode":"headless"}),
            &debugger_installation_fixture(),
        )
        .expect("the headless debugger host should be valid");

        assert_eq!(executable, PathBuf::from("dreamdaemon.exe"));
    }

    #[test]
    fn debugger_host_rejects_unknown_modes() {
        let error = debugger_host_executable(
            &json!({"host_mode":"detached"}),
            &debugger_installation_fixture(),
        )
        .expect_err("unknown debugger host modes must fail closed");

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
