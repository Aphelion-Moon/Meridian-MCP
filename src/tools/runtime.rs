use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::net::TcpStream;
use tokio::process::Command;
use tracing::info;

use crate::process_environment::minimal_runtime_environment;

mod arguments;
use arguments::{OutputPattern, RunOptions, WaitOptions};

#[cfg(all(test, any(windows, target_os = "linux")))]
mod launch_tests;
#[cfg(test)]
mod topic_tests;

const DEFAULT_OUTPUT_WAIT_TIMEOUT_MS: u64 = 30_000;
const MAX_OUTPUT_WAIT_TIMEOUT_MS: u64 = 300_000;
const MAX_TOPIC_TIMEOUT_MS: u64 = 60_000;
const OUTPUT_READ_CHUNK_BYTES: usize = 8 * 1024;
const LOG_FILE_POLL_INTERVAL_MS: u64 = 50;

struct StartupOwnership(Option<Arc<crate::process::ProcessContainment>>);

impl Drop for StartupOwnership {
    fn drop(&mut self) {
        if let Some(containment) = self.0.take() {
            let _ = containment.terminate(1);
        }
    }
}

use crate::mcp::ToolResult;
use crate::state::{
    OutputLog, RuntimeState, ServerState, OUTPUT_LINE_MAX_BYTES, OUTPUT_TRUNCATED_SUFFIX,
};

/// Find the DreamDaemon executable
pub(crate) fn find_dreamdaemon() -> Option<PathBuf> {
    if let Ok(path) = which::which("dreamdaemon") {
        return Some(path);
    }
    if let Ok(path) = which::which("DreamDaemon") {
        return Some(path);
    }

    let possible_paths = [
        r"C:\Program Files (x86)\BYOND\bin\dreamdaemon.exe",
        r"C:\Program Files\BYOND\bin\dreamdaemon.exe",
        "/usr/local/byond/bin/DreamDaemon",
        "/opt/byond/bin/DreamDaemon",
    ];

    for path in &possible_paths {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    None
}

pub(crate) fn find_dreamdaemon_for_compilers(compilers: &[PathBuf]) -> Option<PathBuf> {
    for compiler in compilers {
        let Some(directory) = compiler.parent() else {
            continue;
        };
        for executable_name in ["dreamdaemon.exe", "DreamDaemon"] {
            let candidate = directory.join(executable_name);
            if candidate.is_file() {
                return Some(
                    candidate
                        .canonicalize()
                        .map(|path| normalize_spawn_path(&path))
                        .unwrap_or(candidate),
                );
            }
        }
    }
    find_dreamdaemon()
}

fn build_dreamdaemon_args(
    dmb_path: &Path,
    working_directory: &Path,
    port: u16,
    extra_args: &[String],
) -> Vec<String> {
    let mut arguments = vec![
        dmb_path.display().to_string(),
        port.to_string(),
        "-trusted".to_string(),
        "-logself".to_string(),
        "-ip".to_string(),
        "127.0.0.1".to_string(),
        "-cd".to_string(),
        working_directory.display().to_string(),
    ];
    arguments.extend(extra_args.iter().cloned());
    arguments
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

fn readiness_succeeded(readiness: &Value) -> bool {
    readiness
        .get("matched")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Start DreamDaemon with a .dmb file
pub async fn run(
    context: &super::ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let options = match RunOptions::parse(&args) {
        Ok(options) => options,
        Err(error) => return Ok(invalid_arguments(error)),
    };
    let lifecycle = state.lifecycle().await;
    if state.debugger().await.is_some() {
        return Err(anyhow!(
            "a debugger session is active; stop it before launching DreamDaemon"
        ));
    }
    run_internal(context, state, options, None, Some(lifecycle)).await
}

pub(crate) async fn run_profiled_with_lifecycle(
    context: &super::ToolExecutionContext,
    state: &ServerState,
    args: Value,
    profiler_port: u16,
) -> Result<ToolResult> {
    let options = match RunOptions::parse(&args) {
        Ok(options) => options,
        Err(error) => return Ok(invalid_arguments(error)),
    };
    if state.debugger().await.is_some() {
        return Err(anyhow!(
            "a debugger session is active; stop it before launching Tracy"
        ));
    }
    run_internal(context, state, options, Some(profiler_port), None).await
}

fn invalid_arguments(error: anyhow::Error) -> ToolResult {
    crate::result::structured_error(
        crate::result::ToolErrorCode::InvalidInput,
        error.to_string(),
        Some("Correct the named runtime argument and retry.".to_owned()),
        json!({}),
    )
}

async fn run_internal(
    context: &super::ToolExecutionContext,
    server_state: &ServerState,
    options: RunOptions,
    profiler_port: Option<u16>,
    lifecycle: Option<tokio::sync::MutexGuard<'_, ()>>,
) -> Result<ToolResult> {
    let RunOptions {
        dmb_path,
        port,
        working_directory: requested_working_directory,
        daemon_args: mut extra_args,
        readiness,
        startup_timeout_ms,
        require_verified,
    } = options;
    let active_snapshot = if profiler_port.is_none() {
        server_state.active_snapshot().await
    } else {
        None
    };
    let mut state = server_state.runtime().await;
    // Check if already running
    if state.is_game_running() {
        return Ok(ToolResult::error(
            "A game instance is already running. Use dm_stop first.",
        ));
    }

    finalize_standard_integrity(&mut state, "natural_exit").await?;

    let requested_path = PathBuf::from(&dmb_path);
    let path = if requested_path.is_absolute() {
        requested_path
    } else if let Some(working_directory) = &requested_working_directory {
        working_directory.join(requested_path)
    } else {
        requested_path
    };
    if !path.exists() {
        return Ok(ToolResult::error(format!("DMB file not found: {dmb_path}")));
    }
    let path = path.canonicalize()?;
    let spawn_path = normalize_spawn_path(&path);
    let artifact_directory = spawn_path.parent().ok_or_else(|| {
        anyhow!(
            "DMB file has no working directory: {}",
            spawn_path.display()
        )
    })?;
    let working_directory = requested_working_directory
        .as_ref()
        .map(|directory| {
            directory
                .canonicalize()
                .map(|path| normalize_spawn_path(&path))
        })
        .transpose()?
        .unwrap_or_else(|| artifact_directory.to_owned());

    let mut launch_lease = if profiler_port.is_none() {
        drop(state);
        let lease = match context
            .execution_lease(
                &path,
                &working_directory,
                "standard",
                context.deadline(startup_timeout_ms),
            )
            .await?
        {
            Ok(lease) => lease,
            Err(result) => return Ok(result),
        };
        state = server_state.runtime().await;
        Some(lease)
    } else {
        None
    };

    if profiler_port.is_some() {
        extra_args.extend(["-params".to_owned(), "tracy".to_owned()]);
    }

    let launch_provenance =
        match super::require_launchable_artifact(context, &path, require_verified) {
            Ok(provenance) => provenance,
            Err(result) => return Ok(result),
        };
    let dreamdaemon = find_dreamdaemon_for_compilers(context.policy().compiler_allowlist())
        .ok_or_else(|| anyhow!("DreamDaemon not found. Please install BYOND."))?;

    info!(
        "Starting DreamDaemon with {} on port {}",
        path.display(),
        port
    );
    state.clear_runtime_diagnostics();
    state.integrity_summary = None;

    // DreamDaemon changes directory itself unless -cd is supplied. Set both
    // the process and engine directories so relative game inputs agree.
    let daemon_args = build_dreamdaemon_args(&spawn_path, &working_directory, port, &extra_args);
    let log_path = spawn_path.with_extension("log");
    let log_start_offset = std::fs::metadata(&log_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if profiler_port.is_none() {
        if let Some(store) = context.private_state_arc() {
            let protected_root = active_snapshot
                .as_ref()
                .filter(|snapshot| snapshot.environment_path.with_extension("dmb") == path)
                .and_then(|snapshot| snapshot.environment_path.parent())
                .unwrap_or(artifact_directory)
                .to_owned();
            let session = crate::runtime_integrity::RuntimeIntegritySession::create(
                store,
                &protected_root,
                launch_provenance.clone(),
                Arc::clone(&state.output_log),
                vec![log_path.clone()],
            )?;
            state.integrity = Some(Arc::new(tokio::sync::Mutex::new(session)));
        }
    }
    let mut command = Command::new(&dreamdaemon);
    command
        .args(&daemon_args)
        .current_dir(&working_directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(profiler_port) = profiler_port {
        command
            .env_clear()
            .envs(minimal_runtime_environment())
            .env("UTRACY_BIND_ADDRESS", "127.0.0.1")
            .env("UTRACY_BIND_PORT", profiler_port.to_string());
    }
    let mut lease = launch_lease
        .take()
        .or_else(|| state.execution_lease.take())
        .ok_or_else(|| anyhow!("runtime execution lease is missing"))?;
    lease.mark_writer_started();
    let (mut child, containment) = match crate::process::spawn_runtime_process(&mut command) {
        Ok(child) => child,
        Err(error) => {
            let _ = finalize_standard_integrity(&mut state, "spawn_failed").await;
            return Err(error);
        }
    };
    let mut startup_ownership = StartupOwnership(Some(Arc::clone(&containment)));
    state.containment = Some(containment);
    state.execution_lease = Some(lease);

    let pid = child.id();

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(profiler_port) = profiler_port {
        state.set_profiled_game_process(child, port, profiler_port, launch_provenance.clone());
    } else {
        state.set_game_process(child, port, launch_provenance.clone());
        if let Some(session) = state.integrity.clone() {
            session.lock().await.set_process_id(pid)?;
            let (stop, receiver) = tokio::sync::watch::channel(false);
            state.integrity_stop = Some(stop);
            state.integrity_task = Some(crate::runtime_integrity::spawn_monitor(session, receiver));
        }
    }

    if let Some(stdout) = stdout {
        let output_log = Arc::clone(&state.output_log);
        let task = tokio::spawn(async move {
            capture_output_stream(stdout, output_log).await;
        });
        state.add_runtime_output_task(task);
    }

    if let Some(stderr) = stderr {
        let output_log = Arc::clone(&state.output_log);
        let task = tokio::spawn(async move {
            capture_output_stream(stderr, output_log).await;
        });
        state.add_runtime_output_task(task);
    }

    let output_log = Arc::clone(&state.output_log);
    let task = tokio::spawn(capture_output_file(
        log_path.clone(),
        log_start_offset,
        output_log,
    ));
    state.add_runtime_output_task(task);

    server_state.observe_runtime(&mut state);
    let observation = Arc::clone(&state.output_log);
    drop(state);
    drop(lifecycle);

    // Give the child process a short window to fail before returning control to the caller.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Check if it's actually running
    let mut state = server_state.runtime().await;
    if !Arc::ptr_eq(&observation, &state.output_log) {
        return Ok(ToolResult::error(
            "Runtime session was replaced during launch.",
        ));
    }
    if !state.is_game_running() {
        let integrity = finalize_standard_integrity(&mut state, "launch_failed").await?;
        return Ok(ToolResult::error(
            json!({
                "message": "DreamDaemon process exited immediately. Check the DMB file.",
                "last_exit_code": state.last_exit_code,
                "recent_output": state.recent_output(50),
                "integrity": integrity
            })
            .to_string(),
        ));
    }

    let mut result = json!({
        "success": true,
        "pid": pid,
        "port": port,
        "dmb_path": dmb_path,
        "working_directory": working_directory.display().to_string(),
        "runtime_kind": if profiler_port.is_some() { "tracy" } else { "standard" },
        "profiler_port": profiler_port,
        "launch_provenance": launch_provenance,
        "message": format!("DreamDaemon started on port {}", port)
    });
    drop(state);

    if let Some(pattern) = &readiness {
        let wait_result = wait_for_output_value(&observation, pattern, startup_timeout_ms).await?;
        result["readiness"] = wait_result;
        if !readiness_succeeded(&result["readiness"]) {
            let _lifecycle = server_state.lifecycle().await;
            let mut state = server_state.runtime().await;
            result["success"] = json!(false);
            if Arc::ptr_eq(&observation, &state.output_log) {
                state.stop_game_process().await?;
                result["process_stopped"] = json!(true);
                result["integrity"] =
                    json!(finalize_standard_integrity(&mut state, "launch_failed").await?);
            }
            return Ok(ToolResult::error(result.to_string()));
        }
    }

    let mut state = server_state.runtime().await;
    if !Arc::ptr_eq(&observation, &state.output_log) || !state.is_game_running() {
        return Ok(ToolResult::error("Runtime session ended during launch."));
    }
    if let Some(session) = &state.integrity {
        let summary = session.lock().await.observe_now("launch_ready").await?;
        state.integrity_summary = Some(summary.clone());
        result["integrity"] = json!(summary);
    }

    startup_ownership.0 = None;
    Ok(ToolResult::text(result.to_string()))
}

async fn finalize_standard_integrity(
    state: &mut RuntimeState,
    action: &'static str,
) -> Result<Option<crate::runtime_integrity::RuntimeIntegritySummary>> {
    state.finish_runtime_cleanup().await?;
    let terminal_summary = state
        .integrity_summary
        .as_ref()
        .filter(|summary| {
            summary.status != crate::runtime_integrity::RuntimeIntegrityStatus::Active
        })
        .cloned();
    if let Some(stop) = state.integrity_stop.take() {
        let _ = stop.send(true);
    }
    if let Some(task) = state.integrity_task.take() {
        let _ = task.await;
    }
    let summary = if terminal_summary.is_some() {
        terminal_summary
    } else if let Some(session) = &state.integrity {
        let summary = session.lock().await.finalize(action).await?;
        state.integrity_summary = Some(summary.clone());
        Some(summary)
    } else {
        None
    };
    if state
        .execution_lease
        .as_ref()
        .is_some_and(|lease| lease.kind() == "standard")
    {
        state
            .execution_lease
            .as_mut()
            .expect("checked standard lease")
            .finish()?;
        state.execution_lease = None;
    }
    Ok(summary)
}

async fn capture_output_stream<R>(mut stream: R, output_log: OutputLog)
where
    R: AsyncRead + Unpin,
{
    let mut read_buffer = [0_u8; OUTPUT_READ_CHUNK_BYTES];
    let mut line_buffer = Vec::with_capacity(OUTPUT_READ_CHUNK_BYTES);
    let mut line_truncated = false;
    let line_content_limit = OUTPUT_LINE_MAX_BYTES.saturating_sub(OUTPUT_TRUNCATED_SUFFIX.len());

    loop {
        let bytes_read = match stream.read(&mut read_buffer).await {
            Ok(0) => break,
            Ok(bytes_read) => bytes_read,
            Err(_) => break,
        };

        for byte in &read_buffer[..bytes_read] {
            if *byte == b'\n' {
                push_captured_output_line(&output_log, &mut line_buffer, line_truncated);
                line_truncated = false;
                continue;
            }

            if line_buffer.len() < line_content_limit {
                line_buffer.push(*byte);
            } else {
                line_truncated = true;
            }
        }
    }

    if !line_buffer.is_empty() || line_truncated {
        push_captured_output_line(&output_log, &mut line_buffer, line_truncated);
    }
}

async fn capture_output_file(path: PathBuf, start_offset: u64, output_log: OutputLog) {
    let mut file = loop {
        match OpenOptions::new().read(true).open(&path).await {
            Ok(file) => break file,
            Err(_) => tokio::time::sleep(Duration::from_millis(LOG_FILE_POLL_INTERVAL_MS)).await,
        }
    };

    let mut offset = start_offset;
    if file.seek(SeekFrom::Start(offset)).await.is_err() {
        return;
    }

    let mut read_buffer = [0_u8; OUTPUT_READ_CHUNK_BYTES];
    let mut line_buffer = Vec::with_capacity(OUTPUT_READ_CHUNK_BYTES);
    let mut line_truncated = false;
    let line_content_limit = OUTPUT_LINE_MAX_BYTES.saturating_sub(OUTPUT_TRUNCATED_SUFFIX.len());

    loop {
        match file.read(&mut read_buffer).await {
            Ok(0) => {
                if let Ok(metadata) = file.metadata().await {
                    if metadata.len() < offset {
                        offset = 0;
                        line_buffer.clear();
                        line_truncated = false;
                        if file.seek(SeekFrom::Start(0)).await.is_err() {
                            return;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(LOG_FILE_POLL_INTERVAL_MS)).await;
            }
            Ok(bytes_read) => {
                offset += bytes_read as u64;
                for byte in &read_buffer[..bytes_read] {
                    if *byte == b'\n' {
                        push_captured_output_line(&output_log, &mut line_buffer, line_truncated);
                        line_truncated = false;
                        continue;
                    }

                    if line_buffer.len() < line_content_limit {
                        line_buffer.push(*byte);
                    } else {
                        line_truncated = true;
                    }
                }
            }
            Err(_) => return,
        }
    }
}

fn push_captured_output_line(
    output_log: &OutputLog,
    line_buffer: &mut Vec<u8>,
    line_truncated: bool,
) {
    if line_buffer.last() == Some(&b'\r') {
        line_buffer.pop();
    }

    let mut line = String::from_utf8_lossy(line_buffer).to_string();
    if line_truncated {
        line.push_str(OUTPUT_TRUNCATED_SUFFIX);
    }
    crate::state::push_output_line(output_log, line);
    line_buffer.clear();
}

async fn wait_for_output_value(
    output_log: &OutputLog,
    pattern: &OutputPattern,
    timeout_ms: u64,
) -> Result<Value> {
    let timeout_ms = timeout_ms.min(MAX_OUTPUT_WAIT_TIMEOUT_MS);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut changes = output_log
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .changes
        .subscribe();

    loop {
        changes.borrow_and_update();
        let (output, running, drained, exit_code) = {
            let buffer = output_log
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                buffer
                    .entries
                    .iter()
                    .map(|entry| entry.text.clone())
                    .collect::<Vec<_>>(),
                buffer.running,
                buffer.drained,
                buffer.last_exit_code,
            )
        };
        let text = output.join("\n");
        let matched = pattern.matches(&text);
        let recent_output = output
            .into_iter()
            .rev()
            .take(50)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        if matched {
            return Ok(json!({
                "matched": true,
                "pattern": pattern.text,
                "regex": pattern.is_regex(),
                "timed_out": false,
                "recent_output": recent_output
            }));
        }

        if !running && drained {
            return Ok(json!({
                "matched": false,
                "pattern": pattern.text,
                "regex": pattern.is_regex(),
                "timed_out": false,
                "process_exited": true,
                "last_exit_code": exit_code,
                "recent_output": recent_output
            }));
        }

        if tokio::time::Instant::now() >= deadline {
            return Ok(json!({
                "matched": false,
                "pattern": pattern.text,
                "regex": pattern.is_regex(),
                "timed_out": true,
                "process_exited": false,
                "recent_output": recent_output
            }));
        }

        tokio::select! {
            _ = changes.changed() => {},
            () = tokio::time::sleep_until(deadline) => {},
        }
    }
}

pub(crate) async fn wait_for_literal_output(
    state: &ServerState,
    pattern: &str,
    timeout_ms: u64,
) -> Result<Value> {
    let mut runtime = state.runtime().await;
    state.observe_runtime(&mut runtime);
    let output = Arc::clone(&runtime.output_log);
    drop(runtime);
    wait_for_output_value(
        &output,
        &OutputPattern::new(pattern.to_owned(), false, "pattern")?,
        timeout_ms,
    )
    .await
}

/// Wait until DreamDaemon output contains a literal or regular-expression marker.
pub async fn wait_for_output(server_state: &ServerState, args: Value) -> Result<ToolResult> {
    let options = match WaitOptions::parse(&args) {
        Ok(options) => options,
        Err(error) => return Ok(invalid_arguments(error)),
    };
    let mut state = server_state.runtime().await;
    let running = state.is_game_running();
    let has_runtime_diagnostics =
        state.last_exit_code.is_some() || !state.recent_output(1).is_empty();
    if !running && !has_runtime_diagnostics {
        return Ok(ToolResult::error(
            json!({
                "message": "No game instance is currently running.",
                "last_exit_code": state.last_exit_code,
                "recent_output": state.recent_output(50)
            })
            .to_string(),
        ));
    }

    server_state.observe_runtime(&mut state);
    let output = Arc::clone(&state.output_log);
    let provenance = state.launch_provenance.clone();
    drop(state);
    let result = wait_for_output_value(&output, &options.pattern, options.timeout_ms).await?;
    let mut state = server_state.runtime().await;
    if !Arc::ptr_eq(&output, &state.output_log) {
        let mut result = result;
        result["launch_provenance"] = json!(provenance);
        result["recent_output_entries"] = json!(output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .iter()
            .rev()
            .take(50)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>());
        return Ok(ToolResult::text(result.to_string()));
    }
    let integrity = if state.is_game_running() {
        if let Some(session) = &state.integrity {
            Some(session.lock().await.observe_now("wait_for_output").await?)
        } else {
            None
        }
    } else {
        finalize_standard_integrity(&mut state, "natural_exit").await?
    };
    let mut result = result;
    result["integrity"] = json!(integrity);
    result["launch_provenance"] = json!(state.launch_provenance);
    result["recent_output_entries"] = json!(state.recent_output_entries(50));
    Ok(ToolResult::text(result.to_string()))
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::ToolContent;
    use crate::state::push_output_line;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn invalid_launch_options_are_rejected_before_lifecycle_access() {
        let state = ServerState::new();
        let root = std::env::temp_dir();
        let context = crate::tools::ToolExecutionContext::new(
            crate::CapabilityMode::Development,
            crate::PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
        );
        let lifecycle = state.lifecycle().await;
        for (key, value) in [
            ("require_verified_provenance", json!("true")),
            ("require_verified_provenance", Value::Null),
            ("wait_regex", json!("false")),
            ("wait_regex", Value::Null),
            ("wait_for", json!(false)),
            ("wait_for", Value::Null),
            ("working_directory", json!(7)),
            ("working_directory", Value::Null),
            ("daemon_args", json!("-close")),
            ("daemon_args", json!(["-close", 4])),
            ("daemon_args", Value::Null),
            ("startup_timeout_ms", json!(0)),
            ("port", json!(65536)),
            ("unknown_launch_option", json!(true)),
        ] {
            let mut args = json!({"dmb_path":root.join("unused-invalid-launch.dmb")});
            args[key] = value;
            let outcome =
                tokio::time::timeout(Duration::from_millis(100), run(&context, &state, args))
                    .await
                    .unwrap_or_else(|_| panic!("invalid {key} waited for runtime lifecycle access"))
                    .unwrap();
            assert_eq!(outcome.is_error, Some(true), "{key}");
            let ToolContent::Text { text } = &outcome.content[0];
            assert!(
                text.contains("invalid_input") && text.contains(key),
                "{text}"
            );
        }
        drop(lifecycle);
    }

    #[tokio::test]
    async fn invalid_output_wait_options_are_rejected_with_retained_output() {
        let state = ServerState::new();
        push_output_line(&state.runtime().await.output_log, "fixture ready".into());
        for (key, value) in [
            ("regex", json!("false")),
            ("regex", Value::Null),
            ("timeout_ms", json!("10")),
            ("timeout_ms", json!(-1)),
            ("timeout_ms", json!(1.5)),
            ("timeout_ms", Value::Null),
            ("pattern", json!(42)),
            ("pattern", Value::Null),
        ] {
            let mut args = json!({"pattern":"fixture ready", "timeout_ms":0});
            args[key] = value;
            let outcome = wait_for_output(&state, args).await.unwrap();
            assert_eq!(outcome.is_error, Some(true), "invalid {key} was accepted");
            let ToolContent::Text { text } = &outcome.content[0];
            assert!(
                text.contains("invalid_input") && text.contains(key),
                "{text}"
            );
        }
        for timeout in [0, MAX_OUTPUT_WAIT_TIMEOUT_MS, u64::MAX] {
            let outcome = wait_for_output(
                &state,
                json!({"pattern":"fixture ready", "regex":true, "timeout_ms":timeout}),
            )
            .await
            .unwrap();
            assert_ne!(outcome.is_error, Some(true));
        }
    }

    #[cfg(windows)]
    #[test]
    fn dreamdaemon_spawn_paths_drop_windows_verbatim_prefix() {
        let verbatim = Path::new(r"\\?\C:\workspace\tgstation.dmb");
        assert_eq!(
            normalize_spawn_path(verbatim),
            PathBuf::from(r"C:\workspace\tgstation.dmb")
        );

        let ordinary = Path::new(r"C:\workspace\tgstation.dmb");
        assert_eq!(normalize_spawn_path(ordinary), ordinary);

        let unc_verbatim = Path::new(r"\\?\UNC\server\share\tgstation.dmb");
        assert_eq!(
            normalize_spawn_path(unc_verbatim),
            PathBuf::from(r"\\server\share\tgstation.dmb")
        );
    }

    #[test]
    fn configured_compiler_resolves_its_sibling_dreamdaemon() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "meridian-mcp-byond-installation-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let compiler = root.join("dm.exe");
        let daemon = root.join("dreamdaemon.exe");
        std::fs::write(&compiler, b"compiler").unwrap();
        std::fs::write(&daemon, b"daemon").unwrap();

        assert_eq!(
            find_dreamdaemon_for_compilers(std::slice::from_ref(&compiler)),
            Some(normalize_spawn_path(&daemon.canonicalize().unwrap()))
        );

        std::fs::remove_file(compiler).unwrap();
        std::fs::remove_file(daemon).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn stop_waits_for_exclusive_runtime_lifecycle_access() {
        let state = ServerState::new();
        let lifecycle = state.lifecycle().await;

        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            stop(&state, json!({})),
        )
        .await;

        assert!(blocked.is_err(), "stop bypassed the runtime lifecycle lock");
        drop(lifecycle);
        let result = stop(&state, json!({})).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn readiness_requires_a_matching_marker() {
        assert!(readiness_succeeded(&json!({"matched": true})));
        assert!(!readiness_succeeded(&json!({"matched": false})));
        assert!(!readiness_succeeded(&json!({"timed_out": true})));
    }

    #[test]
    fn daemon_arguments_preserve_extra_runtime_arguments() {
        let arguments = build_dreamdaemon_args(
            PathBuf::from("tgstation.test.dmb").as_path(),
            Path::new("game checkout"),
            1337,
            &[
                "-close".to_string(),
                "-params".to_string(),
                "log-directory=ci".to_string(),
            ],
        );

        assert_eq!(
            arguments,
            vec![
                "tgstation.test.dmb",
                "1337",
                "-trusted",
                "-logself",
                "-ip",
                "127.0.0.1",
                "-cd",
                "game checkout",
                "-close",
                "-params",
                "log-directory=ci",
            ]
        );
    }

    #[tokio::test]
    async fn wait_for_output_can_match_retained_output_after_exit() {
        let state = ServerState::new();
        {
            let mut runtime = state.runtime().await;
            runtime.last_exit_code = Some(1);
            push_output_line(
                &runtime.output_log,
                "fatal: initialization failed".to_string(),
            );
        }

        let result = wait_for_output(
            &state,
            json!({"pattern": "initialization failed", "timeout_ms": 10}),
        )
        .await
        .expect("waiting for retained output should succeed");

        assert_eq!(result.is_error, None);
        let ToolContent::Text { text } = &result.content[0];
        let payload: Value = serde_json::from_str(text).expect("wait result should be JSON");
        assert_eq!(payload["matched"], true);
    }

    #[tokio::test]
    async fn exited_session_drains_final_output_before_reporting_missing_marker() {
        let state = ServerState::new();
        {
            let mut runtime = state.runtime().await;
            runtime.last_exit_code = Some(7);
            let output = Arc::clone(&runtime.output_log);
            output.lock().unwrap().drained = false;
            runtime.add_runtime_output_task(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(75)).await;
                capture_output_stream(&b"FINAL_READY"[..], output).await;
            }));
        }
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_output(
                &state,
                json!({"pattern":"FINAL_READY", "timeout_ms":300000}),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let ToolContent::Text { text } = &result.content[0];
        let result: Value = serde_json::from_str(text).unwrap();
        assert_eq!(result["matched"], true);
        assert_eq!(result["recent_output"], json!(["FINAL_READY"]));
    }

    #[tokio::test]
    async fn output_capture_emits_complete_lines_and_final_unterminated_line() {
        let log = crate::state::OutputLog::default();
        let input = b"ready\npartial without newline";

        capture_output_stream(&input[..], Arc::clone(&log)).await;

        let lines: Vec<String> = log
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect();
        assert_eq!(
            lines,
            vec!["ready".to_string(), "partial without newline".to_string(),]
        );
    }

    #[tokio::test]
    async fn output_capture_truncates_oversized_unterminated_lines() {
        let log = crate::state::OutputLog::default();
        let input = "x".repeat(16_384 + 1_000);

        capture_output_stream(input.as_bytes(), Arc::clone(&log)).await;

        let lines: Vec<String> = log
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].len() <= 16_384);
        assert!(lines[0].ends_with("... [truncated]"));
    }

    #[test]
    fn topic_packet_matches_the_byond_export_wire_format() {
        assert_eq!(
            build_topic_packet("ping").expect("topic packet should build"),
            vec![
                0x00, 0x83, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, b'p', b'i', b'n', b'g', 0x00,
            ]
        );
    }

    #[test]
    fn topic_response_decodes_strings_and_floats() {
        assert_eq!(
            decode_topic_response(&[0x06, b'p', b'o', b'n', b'g', 0x00])
                .expect("string response should decode"),
            "pong"
        );
        assert_eq!(
            decode_topic_response(&[0x2a, 0x00, 0x00, 0x20, 0x40])
                .expect("float response should decode"),
            "2.5"
        );
    }

    #[tokio::test]
    async fn log_capture_starts_at_the_recorded_offset_and_follows_appends() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "meridian-mcp-runtime-log-{}-{unique}.log",
            std::process::id()
        ));
        std::fs::write(&path, "old run\n").unwrap();
        let start_offset = std::fs::metadata(&path).unwrap().len();
        let log = crate::state::OutputLog::default();
        let task = tokio::spawn(capture_output_file(
            path.clone(),
            start_offset,
            Arc::clone(&log),
        ));

        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "new run ready").unwrap();
        file.flush().unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if log
                    .lock()
                    .unwrap()
                    .entries
                    .iter()
                    .any(|line| line.text == "new run ready")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("appended log output should be captured");

        task.abort();
        assert!(!log
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|line| line.text == "old run"));
        std::fs::remove_file(path).unwrap();
    }
}

/// Stop the running DreamDaemon instance
pub async fn stop(state: &ServerState, _args: Value) -> Result<ToolResult> {
    let _lifecycle = state.lifecycle().await;
    stop_with_lifecycle(state).await
}

pub(super) async fn stop_with_lifecycle(state: &ServerState) -> Result<ToolResult> {
    let mut state = state.runtime().await;
    let was_running = state.is_game_running();
    if !was_running
        && state.integrity.is_none()
        && state.containment.is_none()
        && state.execution_lease.is_none()
    {
        return Ok(ToolResult::error("No game instance is currently running."));
    }

    let process_stopped = if was_running {
        state.stop_game_process().await?;
        true
    } else {
        false
    };
    let integrity = finalize_standard_integrity(&mut state, "stopped").await?;
    let result = json!({
        "success": !was_running || process_stopped,
        "process_stopped": process_stopped,
        "message": "DreamDaemon stopped",
        "launch_provenance": state.launch_provenance,
        "integrity": integrity,
        "warnings": integrity.as_ref().map(|summary| &summary.warnings).unwrap_or(&Vec::new()),
    });
    if integrity
        .as_ref()
        .is_some_and(|summary| !summary.violations.is_empty())
    {
        Ok(ToolResult::error(result.to_string()))
    } else {
        Ok(ToolResult::text(result.to_string()))
    }
}

/// Get status of the running game
pub async fn status(state: &ServerState, _args: Value) -> Result<ToolResult> {
    let mut state = state.runtime().await;
    if !state.is_game_running() {
        let integrity = finalize_standard_integrity(&mut state, "natural_exit").await?;
        return Ok(ToolResult::text(
            json!({
                "running": false,
                "runtime_kind": state.kind,
                "last_exit_code": state.last_exit_code,
                "launch_provenance": state.launch_provenance,
                "integrity": integrity,
                "recent_output": state.recent_output(50),
                "recent_output_entries": state.recent_output_entries(50)
            })
            .to_string(),
        ));
    }

    let port = state.game_port.unwrap_or(0);
    let pid = state.game_process.as_ref().map(|p| p.id());
    let integrity = if let Some(session) = &state.integrity {
        Some(session.lock().await.observe_now("status").await?)
    } else {
        None
    };

    Ok(ToolResult::text(
        json!({
            "running": true,
            "runtime_kind": state.kind,
            "port": port,
            "profiler_port": state.profiler_port,
            "pid": pid,
            "recent_output": state.recent_output(50),
            "recent_output_entries": state.recent_output_entries(50),
            "launch_provenance": state.launch_provenance,
            "integrity": integrity
        })
        .to_string(),
    ))
}

/// Send a Topic() call to the running game server
pub async fn topic(state: &ServerState, args: Value) -> Result<ToolResult> {
    let request = (|| -> Result<_> {
        let topic = args
            .get("topic")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("topic must be a string"))?;
        let packet = build_topic_packet(topic.strip_prefix('?').unwrap_or(topic))?;
        let timeout_ms = super::bounded_u64(&args, "timeout_ms", 5000, 1, MAX_TOPIC_TIMEOUT_MS)?;
        Ok((packet, timeout_ms))
    })();
    let (packet, timeout_ms) = match request {
        Ok(request) => request,
        Err(error) => return Ok(invalid_arguments(error)),
    };
    let mut runtime = state.runtime().await;
    if !runtime.is_game_running() {
        return Ok(ToolResult::error("No game instance is currently running."));
    }
    state.observe_runtime(&mut runtime);
    let port = runtime.game_port.unwrap_or(1337);
    let output = Arc::clone(&runtime.output_log);
    drop(runtime);
    info!("Sending Topic to port {port}");
    let address = format!("127.0.0.1:{port}");
    let response = tokio::select! {
        biased;
        () = runtime_session_ended(&output) => {
            return Ok(ToolResult::error("Runtime session ended during Topic request."));
        }
        response = send_topic_packet(&address, &packet, timeout_ms) => response,
    };
    match response {
        Ok(response) => Ok(ToolResult::text(
            json!({
                "success": true,
                "response": response
            })
            .to_string(),
        )),
        Err(error) => Ok(crate::result::structured_error(
            if error.is::<tokio::time::error::Elapsed>() {
                crate::result::ToolErrorCode::TimedOut
            } else {
                crate::result::ToolErrorCode::ExternalToolFailure
            },
            format!("Topic call failed: {error}"),
            Some(
                "Check the running world's Topic handler and retry within the timeout limit."
                    .to_owned(),
            ),
            json!({}),
        )),
    }
}

async fn runtime_session_ended(output: &OutputLog) {
    let mut changes = output
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .changes
        .subscribe();
    loop {
        changes.borrow_and_update();
        if !output
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running
        {
            return;
        }
        if changes.changed().await.is_err() {
            return;
        }
    }
}

/// Send a BYOND Topic packet and get response
pub(crate) async fn send_topic(address: &str, topic: &str, timeout_ms: u64) -> Result<String> {
    let topic_clean = topic.strip_prefix('?').unwrap_or(topic);
    let packet = build_topic_packet(topic_clean)?;
    send_topic_packet(address, &packet, timeout_ms).await
}

async fn send_topic_packet(address: &str, packet: &[u8], timeout_ms: u64) -> Result<String> {
    use anyhow::Context;
    anyhow::ensure!(
        (1..=MAX_TOPIC_TIMEOUT_MS).contains(&timeout_ms),
        "timeout_ms must be between 1 and {MAX_TOPIC_TIMEOUT_MS}"
    );
    // One deadline covers connection, request and every response fragment.
    // Dropping this future also drops the socket; no blocking worker remains.
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut stream = TcpStream::connect(address).await?;
        stream.write_all(packet).await?;
        let mut header = [0; 4];
        stream.read_exact(&mut header).await?;
        anyhow::ensure!(header[..2] == [0, 0x83], "Invalid response header");
        let length = u16::from_be_bytes([header[2], header[3]]) as usize;
        if length == 0 {
            return Ok(String::new());
        }
        let mut response = vec![0; length];
        stream.read_exact(&mut response).await?;
        decode_topic_response(&response)
    })
    .await
    .with_context(|| format!("Topic request timed out after {timeout_ms} ms"))?
}

fn build_topic_packet(topic: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(!topic.contains('\0'), "topic must not contain a NUL byte");
    let topic_bytes = topic.as_bytes();
    let data_len = topic_bytes.len() + 6;
    if data_len > u16::MAX as usize {
        return Err(anyhow!("Topic is too long"));
    }

    let mut packet = Vec::with_capacity(4 + data_len);
    packet.extend_from_slice(&[0x00, 0x83, (data_len >> 8) as u8, (data_len & 0xff) as u8]);
    packet.extend_from_slice(&[0x00; 5]);
    packet.extend_from_slice(topic_bytes);
    packet.push(0x00);
    Ok(packet)
}

fn decode_topic_response(response_data: &[u8]) -> Result<String> {
    let Some(response_type) = response_data.first() else {
        return Err(anyhow!("BYOND returned an empty Topic response"));
    };

    match response_type {
        0x00 if response_data.len() == 1 => Ok(String::new()),
        0x06 => {
            let string_data = &response_data[1..];
            let null_pos = string_data
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(string_data.len());
            Ok(String::from_utf8_lossy(&string_data[..null_pos]).to_string())
        }
        0x2a if response_data.len() >= 5 => {
            let float_bytes = response_data[1..5]
                .try_into()
                .expect("length was checked before conversion");
            Ok(f32::from_le_bytes(float_bytes).to_string())
        }
        0x2a => Err(anyhow!("BYOND returned a truncated float Topic response")),
        _ => Err(anyhow!(
            "BYOND returned an unknown Topic response type: 0x{response_type:02x}"
        )),
    }
}
