use crate::tracy_protocol::{TracyProtocolError, TRACY_HELPER_SCHEMA_VERSION};
use serde_json::Value;
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, watch, Mutex};

pub const MAX_TRACY_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_TRACY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn capture_window_started(error: &TracyProtocolError) -> bool {
    matches!(
        error,
        TracyProtocolError::Helper { details, .. }
            if details["window_started"].as_bool() == Some(true)
    )
}

pub(crate) fn capture_failure_code(error: &TracyProtocolError) -> crate::result::ToolErrorCode {
    match error {
        TracyProtocolError::Helper { code, .. }
            if !capture_window_started(error)
                && matches!(
                    code.as_str(),
                    "connect_timeout"
                        | "handshake_dropped"
                        | "client_disconnected"
                        | "profiler_busy"
                        | "health_timeout"
                ) =>
        {
            crate::result::ToolErrorCode::CaptureNotReady
        }
        _ => crate::result::ToolErrorCode::HelperFailure,
    }
}

type PendingResult = Result<Value, String>;
const MAX_PENDING_REQUESTS: usize = 256;
const MAX_ABANDONED_REQUESTS: usize = 256;
const MAX_STDERR_LINE_BYTES: usize = 4096;
const MAX_STDERR_LINES: usize = 64;

struct Registry {
    pending: BTreeMap<u64, oneshot::Sender<PendingResult>>,
    abandoned: BTreeSet<u64>,
    terminal: Option<String>,
    shutdown: watch::Sender<bool>,
}

struct TransportOwner(Arc<StdMutex<Registry>>);

impl Drop for TransportOwner {
    fn drop(&mut self) {
        fail_transport(&self.0, "collector transport dropped".into());
    }
}

struct RequestGuard {
    registry: Arc<StdMutex<Registry>>,
    id: u64,
    complete: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if !self.complete {
            fail_transport(
                &self.registry,
                "collector request frame was abandoned during write".into(),
            );
            return;
        }
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.pending.remove(&self.id).is_some() && registry.terminal.is_none() {
            if registry.abandoned.len() == MAX_ABANDONED_REQUESTS {
                drop(registry);
                fail_transport(
                    &self.registry,
                    "collector abandoned-request capacity exceeded".into(),
                );
            } else {
                registry.abandoned.insert(self.id);
            }
        }
    }
}

pub struct CollectorTransport<W> {
    writer: Arc<Mutex<Option<W>>>,
    next_request_id: Arc<AtomicU64>,
    owner: Arc<TransportOwner>,
    request_timeout: Duration,
}

impl<W> Clone for CollectorTransport<W> {
    fn clone(&self) -> Self {
        Self {
            writer: self.writer.clone(),
            next_request_id: self.next_request_id.clone(),
            owner: self.owner.clone(),
            request_timeout: self.request_timeout,
        }
    }
}

impl<W: AsyncWrite + Unpin + Send + 'static> CollectorTransport<W> {
    pub fn new<R: AsyncRead + Unpin + Send + 'static>(
        reader: R,
        writer: W,
        request_timeout: Duration,
    ) -> Self {
        let (shutdown, mut stopped) = watch::channel(false);
        let registry = Arc::new(StdMutex::new(Registry {
            pending: BTreeMap::new(),
            abandoned: BTreeSet::new(),
            terminal: None,
            shutdown,
        }));
        let reader_registry = registry.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            loop {
                let frame = tokio::select! {
                    biased;
                    _ = stopped.changed() => return,
                    frame = bounded_line(&mut reader, MAX_TRACY_RESPONSE_BYTES + 2, false) => frame,
                };
                let result = (|| -> Result<(), String> {
                    let Some((mut bytes, _)) =
                        frame.map_err(|e| format!("collector read failed: {e}"))?
                    else {
                        return Err("collector closed stdout".into());
                    };
                    if bytes.last() == Some(&b'\n') {
                        bytes.pop();
                    }
                    if bytes.last() == Some(&b'\r') {
                        bytes.pop();
                    }
                    if bytes.len() > MAX_TRACY_RESPONSE_BYTES {
                        return Err("collector response exceeded the fixed byte limit".into());
                    }
                    let document: Value = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("collector returned invalid JSON: {e}"))?;
                    if document["schema_version"].as_u64()
                        != Some(TRACY_HELPER_SCHEMA_VERSION.into())
                    {
                        return Err("collector response schema mismatch".into());
                    }
                    let id = document["id"]
                        .as_u64()
                        .ok_or("collector response omitted its id")?;
                    let mut registry = reader_registry.lock().unwrap_or_else(|e| e.into_inner());
                    if registry.terminal.is_some() {
                        return Ok(());
                    }
                    if let Some(sender) = registry.pending.remove(&id) {
                        drop(registry);
                        let _ = sender.send(Ok(document));
                    } else if !registry.abandoned.remove(&id) {
                        return Err(format!(
                            "collector returned unknown or duplicate response id {id}"
                        ));
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    fail_transport(&reader_registry, error);
                    return;
                }
            }
        });
        Self {
            writer: Arc::new(Mutex::new(Some(writer))),
            next_request_id: Arc::new(AtomicU64::new(1)),
            owner: Arc::new(TransportOwner(registry)),
            request_timeout,
        }
    }

    pub async fn request(&self, command: &str, params: Value) -> Result<Value, TracyProtocolError> {
        self.request_with_timeout(command, params, self.request_timeout)
            .await
    }

    pub async fn request_with_timeout(
        &self,
        command: &str,
        params: Value,
        request_timeout: Duration,
    ) -> Result<Value, TracyProtocolError> {
        let deadline = tokio::time::Instant::now() + request_timeout;
        if !params.is_object() {
            return Err(TracyProtocolError::InvalidParams);
        }
        let id = self
            .next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| TracyProtocolError::Transport("collector request IDs exhausted".into()))?;
        let mut request = serde_json::to_vec(
            &serde_json::json!({"schema_version":TRACY_HELPER_SCHEMA_VERSION,"id":id,"command":command,"params":params}),
        )?;
        request.push(b'\n');
        if request.len() > MAX_TRACY_REQUEST_BYTES {
            return Err(TracyProtocolError::RequestTooLarge);
        }
        let mut writer = tokio::time::timeout_at(deadline, self.writer.lock())
            .await
            .map_err(|_| TracyProtocolError::Timeout { id })?;
        let output = writer
            .as_mut()
            .ok_or_else(|| TracyProtocolError::Transport("collector stdin closed".into()))?;
        let (sender, receiver) = oneshot::channel();
        {
            let mut registry = self.owner.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(error) = &registry.terminal {
                return Err(TracyProtocolError::Transport(error.clone()));
            }
            if registry.pending.len() == MAX_PENDING_REQUESTS {
                return Err(TracyProtocolError::Transport(
                    "collector pending-request capacity exceeded".into(),
                ));
            }
            registry.pending.insert(id, sender);
        }
        // Declared after writer: abandoned-frame cleanup runs before unlocking.
        let mut guard = RequestGuard {
            registry: self.owner.0.clone(),
            id,
            complete: false,
        };
        tokio::time::timeout_at(deadline, output.write_all(&request))
            .await
            .map_err(|_| TracyProtocolError::Timeout { id })?
            .map_err(|e| TracyProtocolError::Transport(format!("collector write failed: {e}")))?;
        guard.complete = true;
        drop(writer);
        let document = tokio::time::timeout_at(deadline, receiver)
            .await
            .map_err(|_| TracyProtocolError::Timeout { id })?
            .map_err(|_| TracyProtocolError::Transport("collector response channel closed".into()))?
            .map_err(TracyProtocolError::Transport)?;
        if document["ok"].as_bool() == Some(false) {
            return Err(TracyProtocolError::Helper {
                code: document["error"]["code"]
                    .as_str()
                    .unwrap_or("unknown")
                    .into(),
                message: document["error"]["message"]
                    .as_str()
                    .unwrap_or("helper failed")
                    .into(),
                details: document["error"]
                    .get("details")
                    .cloned()
                    .unwrap_or(Value::Null),
            });
        }
        document
            .get("result")
            .cloned()
            .ok_or(TracyProtocolError::MissingResult)
    }

    async fn close_writer(&self, deadline: tokio::time::Instant) {
        let _ = tokio::time::timeout_at(deadline, async { self.writer.lock().await.take() }).await;
    }
}

fn fail_transport(registry: &StdMutex<Registry>, message: String) {
    let requests = {
        let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.terminal.is_some() {
            return;
        }
        registry.terminal = Some(message.clone());
        registry.abandoned.clear();
        registry.shutdown.send_replace(true);
        std::mem::take(&mut registry.pending)
    };
    for (_, sender) in requests {
        let _ = sender.send(Err(message.clone()));
    }
}

async fn bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
    drain: bool,
) -> std::io::Result<Option<(Vec<u8>, bool)>> {
    let mut bytes = Vec::with_capacity(limit);
    let mut truncated = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok((!bytes.is_empty() || truncated).then_some((bytes, truncated)));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let retained = consumed.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&available[..retained]);
        truncated |= retained < consumed;
        reader.consume(consumed);
        if truncated && !drain {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "collector frame exceeded fixed byte limit",
            ));
        }
        if newline.is_some() {
            return Ok(Some((bytes, truncated)));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TracySessionPhase {
    ProcessStarting,
    HookFailed,
    ListenerWaiting,
    CollectorConnecting,
    HealthyIdle,
    CaptureActive,
    ProducerStalled,
    Saturated,
    RecoveryRequired,
    Stopping,
    Stopped,
}

pub struct TracyCollectorSpec {
    pub helper: PathBuf,
    pub working_directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub request_timeout: Duration,
}

#[derive(Clone)]
struct CollectorStatus {
    running: bool,
    process_id: u32,
    exit_code: Option<i32>,
    cleanup_error: Option<String>,
}

#[derive(Clone, Copy)]
struct StopSchedule {
    force_at: tokio::time::Instant,
    deadline: tokio::time::Instant,
}

pub struct TracyCollector {
    transport: CollectorTransport<ChildStdin>,
    stderr_tail: Arc<StdMutex<VecDeque<String>>>,
    stderr_task: tokio::task::JoinHandle<()>,
    stop_schedule: watch::Sender<Option<StopSchedule>>,
    process_status: watch::Receiver<CollectorStatus>,
    _supervisor: tokio::task::JoinHandle<()>,
    _containment: Arc<crate::process::ProcessContainment>,
    #[cfg(test)]
    kill_blocked: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for TracyCollector {
    fn drop(&mut self) {
        let now = tokio::time::Instant::now();
        self.stop_schedule.send_replace(Some(StopSchedule {
            force_at: now,
            deadline: now + Duration::from_millis(250),
        }));
        fail_transport(&self.transport.owner.0, "collector dropped".into());
        self.stderr_task.abort();
        // The supervisor retains the actual child until cleanup completes.
    }
}

async fn supervise_child(
    mut child: Child,
    containment: Arc<crate::process::ProcessContainment>,
    mut schedule: watch::Receiver<Option<StopSchedule>>,
    status: watch::Sender<CollectorStatus>,
    #[cfg(test)] kill_blocked: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut stopping = None::<StopSchedule>;
    let mut forced = false;
    let mut control_open = true;
    let exit = loop {
        let force_at = stopping.filter(|_| !forced).map(|stop| stop.force_at);
        tokio::select! {
            biased;
            changed = schedule.changed(), if control_open => {
                if changed.is_err() {
                    control_open = false;
                    forced = false;
                    let now = tokio::time::Instant::now();
                    stopping = Some(StopSchedule {force_at:now, deadline:now + Duration::from_millis(250)});
                } else if let Some(next) = *schedule.borrow_and_update() {
                    if stopping.is_none_or(|current| current.deadline <= tokio::time::Instant::now() || next.force_at < current.force_at) {
                        stopping = Some(next);
                        forced = false;
                    }
                }
            }
            _ = async { match force_at { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {
                forced = true;
                #[cfg(test)]
                if kill_blocked.load(Ordering::SeqCst) {
                    status.send_modify(|state| state.cleanup_error = Some("injected owned-child kill failure".into()));
                    continue;
                }
                let contained = containment.request_termination();
                // Also revoke the root child while its tree is being stopped.
                let killed = child.start_kill();
                let error = contained.and_then(|_| killed.map_err(Into::into)).err().map(|error| format!("collector termination failed: {error}"));
                status.send_modify(|state| state.cleanup_error = error);
            }
            result = child.wait() => break result,
        }
    };
    drop(child);
    let wait_error = exit
        .as_ref()
        .err()
        .map(|error| format!("collector wait failed: {error}"));
    let exit_code = exit.as_ref().ok().and_then(|status| status.code());
    loop {
        let mut error = wait_error.clone();
        if let Err(failure) = containment.request_termination() {
            error = Some(format!(
                "collector containment termination failed: {failure}"
            ));
        }
        let deadline = stopping.map_or_else(
            || tokio::time::Instant::now() + Duration::from_millis(250),
            |stop| stop.deadline,
        );
        loop {
            match containment.is_terminated() {
                Ok(true) => break,
                Ok(false) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(1)).await
                }
                Ok(false) => {
                    error = Some("collector containment cleanup timed out".into());
                    break;
                }
                Err(failure) => {
                    error = Some(format!("collector containment check failed: {failure}"));
                    break;
                }
            }
        }
        status.send_modify(|state| {
            state.running = exit.is_err();
            state.exit_code = exit_code;
            state.cleanup_error = error.clone();
        });
        if error.is_none() || schedule.changed().await.is_err() {
            return;
        }
        stopping = *schedule.borrow_and_update();
    }
}

impl TracyCollector {
    pub async fn spawn(spec: TracyCollectorSpec) -> Result<Self, TracyProtocolError> {
        let mut command = Command::new(&spec.helper);
        command
            .arg("--session")
            .current_dir(&spec.working_directory)
            .env_clear()
            .envs(spec.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let (mut child, containment) = crate::process::spawn_runtime_process(&mut command)
            .map_err(|error| {
                TracyProtocolError::Transport(format!("failed to spawn collector: {error}"))
            })?;
        let process_id = child.id().ok_or_else(|| {
            TracyProtocolError::Transport("collector process id unavailable".to_owned())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            TracyProtocolError::Transport("collector stdout unavailable".to_owned())
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            TracyProtocolError::Transport("collector stdin unavailable".to_owned())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            TracyProtocolError::Transport("collector stderr unavailable".to_owned())
        })?;
        let stderr_tail = Arc::new(StdMutex::new(VecDeque::new()));
        let stderr_lines = stderr_tail.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            while let Ok(Some((mut bytes, truncated))) =
                bounded_line(&mut reader, MAX_STDERR_LINE_BYTES, true).await
            {
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                }
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                let mut line = String::from_utf8_lossy(&bytes).into_owned();
                let mut boundary = line.len().min(MAX_STDERR_LINE_BYTES);
                while !line.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                let decoded_truncated = boundary < line.len();
                line.truncate(boundary);
                if truncated || decoded_truncated {
                    line.push_str("... [truncated]");
                }
                let mut tail = stderr_lines.lock().unwrap_or_else(|e| e.into_inner());
                if tail.len() == MAX_STDERR_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
        let (stop_schedule, schedule) = watch::channel(None);
        let (status, process_status) = watch::channel(CollectorStatus {
            running: true,
            process_id,
            exit_code: None,
            cleanup_error: None,
        });
        #[cfg(test)]
        let kill_blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let supervisor = tokio::spawn(supervise_child(
            child,
            containment.clone(),
            schedule,
            status,
            #[cfg(test)]
            kill_blocked.clone(),
        ));
        Ok(Self {
            transport: CollectorTransport::new(stdout, stdin, spec.request_timeout),
            stderr_tail,
            stderr_task,
            stop_schedule,
            process_status,
            _supervisor: supervisor,
            _containment: containment,
            #[cfg(test)]
            kill_blocked,
        })
    }

    pub async fn session_start(
        &self,
        host: &str,
        port: u16,
        readiness_timeout_ms: u64,
    ) -> Result<Value, TracyProtocolError> {
        self.transport
            .request(
                "session_start",
                session_start_parameters(host, port, readiness_timeout_ms),
            )
            .await
    }

    pub async fn capture_window(
        &self,
        duration_ms: u64,
        memory_limit_mb: u64,
        output_path: &Path,
        phase: &str,
        phase_iteration: u32,
    ) -> Result<Value, TracyProtocolError> {
        self.transport
            .request(
                "capture_window",
                serde_json::json!({
                    "duration_ms": duration_ms,
                    "memory_limit_mb": memory_limit_mb,
                    "output_path": output_path,
                    "phase": phase,
                    "phase_iteration": phase_iteration,
                }),
            )
            .await
    }

    pub async fn status(&self) -> Result<Value, TracyProtocolError> {
        self.transport
            .request_with_timeout(
                "session_status",
                serde_json::json!({}),
                Duration::from_secs(5),
            )
            .await
    }

    pub async fn cancel(&self) -> Result<Value, TracyProtocolError> {
        self.transport
            .request_with_timeout("cancel", serde_json::json!({}), Duration::from_secs(5))
            .await
    }

    pub async fn stop(&self, timeout: Duration) -> Result<Value, TracyProtocolError> {
        let now = tokio::time::Instant::now();
        let deadline = now + timeout;
        // Reserve half the short budget (at most 250 ms) for observed cleanup.
        let force_at = deadline - (timeout / 2).min(Duration::from_millis(250));
        self.stop_schedule.send_if_modified(|current| {
            if current.is_none_or(|current| current.deadline <= now || force_at < current.force_at)
            {
                *current = Some(StopSchedule { force_at, deadline });
                true
            } else {
                false
            }
        });
        let mut status = self.process_status.clone();
        let protocol = async {
            let result = self
                .transport
                .request_with_timeout(
                    "session_stop",
                    serde_json::json!({}),
                    force_at.saturating_duration_since(tokio::time::Instant::now()),
                )
                .await;
            self.transport.close_writer(force_at).await;
            result
        };
        let cleanup = async {
            loop {
                let observed = status.borrow_and_update().clone();
                if !observed.running && observed.cleanup_error.is_none() {
                    return Ok(());
                }
                if status.changed().await.is_err() {
                    return Err(TracyProtocolError::Transport(
                        "collector supervisor closed before exit confirmation".into(),
                    ));
                }
            }
        };
        let result =
            tokio::time::timeout_at(deadline, async { tokio::join!(protocol, cleanup) }).await;
        self.stderr_task.abort();
        match result {
            Ok((protocol, cleanup)) => {
                cleanup?;
                protocol
            }
            Err(_) => Err(TracyProtocolError::Transport(
                "collector stop deadline expired before cleanup confirmation".into(),
            )),
        }
    }

    pub async fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub async fn is_running(&self) -> bool {
        self.process_status.borrow().running
    }

    pub async fn process_id(&self) -> Option<u32> {
        let status = self.process_status.borrow();
        status.running.then_some(status.process_id)
    }

    pub async fn exit_code(&self) -> Option<i32> {
        self.process_status.borrow().exit_code
    }

    pub fn cleanup_confirmed(&self) -> bool {
        let status = self.process_status.borrow();
        !status.running && status.cleanup_error.is_none()
    }

    #[cfg(test)]
    pub(crate) fn inject_cleanup_fault(&self, operation: u8) {
        self._containment.inject_cleanup_fault(operation);
    }
}

fn session_start_parameters(host: &str, port: u16, readiness_timeout_ms: u64) -> Value {
    serde_json::json!({
        "host": host,
        "port": port,
        "connect_timeout_ms": readiness_timeout_ms,
        "progress_timeout_ms": readiness_timeout_ms,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt};

    pub(crate) async fn owned_fixture() -> (PathBuf, Arc<TracyCollector>) {
        owned_fixture_with_mode("respond").await
    }

    async fn owned_fixture_with_mode(mode: &str) -> (PathBuf, Arc<TracyCollector>) {
        #[cfg(windows)]
        crate::process::initialize_runtime_owner().unwrap();
        #[cfg(unix)]
        crate::process::initialize_runtime_owner_with_executable(
            &std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("meridian-mcp"),
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "meridian-collector-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let helper = root.join(format!("collector{}", std::env::consts::EXE_SUFFIX));
        assert!(std::process::Command::new("rustup")
            .args([
                "run",
                "1.95.0",
                "rustc",
                "--edition=2021",
                "tests/fixtures/tracy/blocked_collector.rs",
                "-o"
            ])
            .arg(&helper)
            .status()
            .unwrap()
            .success());
        let collector = TracyCollector::spawn(TracyCollectorSpec {
            helper,
            working_directory: root.clone(),
            environment: vec![("COLLECTOR_MODE".into(), mode.into())],
            request_timeout: Duration::from_secs(1),
        })
        .await
        .unwrap();
        (root, Arc::new(collector))
    }

    #[tokio::test]
    async fn failed_containment_confirmation_is_retryable() {
        let (root, collector) = owned_fixture().await;
        collector.inject_cleanup_fault(2);
        assert!(collector.stop(Duration::from_millis(100)).await.is_err());
        assert!(!collector.cleanup_confirmed());
        collector.inject_cleanup_fault(0);
        let _ = collector.stop(Duration::from_millis(100)).await;
        assert!(collector.cleanup_confirmed());
        drop(collector);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn expired_failed_kill_can_be_rearmed_with_the_owned_child() {
        let (root, collector) = owned_fixture_with_mode("blocked").await;
        collector.kill_blocked.store(true, Ordering::SeqCst);
        let first = collector.stop(Duration::from_millis(100)).await;
        let still_running = collector.is_running().await;
        collector.kill_blocked.store(false, Ordering::SeqCst);
        let _ = collector.stop(Duration::from_millis(500)).await;
        assert!(first.is_err() && still_running);
        assert!(collector.cleanup_confirmed());
        drop(collector);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn dropping_after_failed_force_retries_before_original_deadline() {
        let (root, collector) = owned_fixture_with_mode("blocked").await;
        collector.kill_blocked.store(true, Ordering::SeqCst);
        let mut status = collector.process_status.clone();
        let supervisor_abort = collector._supervisor.abort_handle();
        let now = tokio::time::Instant::now();
        let original_deadline = now + Duration::from_secs(5);
        collector.stop_schedule.send_replace(Some(StopSchedule {
            force_at: now,
            deadline: original_deadline,
        }));
        tokio::time::timeout(Duration::from_secs(1), async {
            while status.borrow_and_update().cleanup_error.is_none() {
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        collector.kill_blocked.store(false, Ordering::SeqCst);
        assert!(tokio::time::Instant::now() < original_deadline);
        drop(collector);
        let cleanup = tokio::time::timeout(Duration::from_secs(1), async {
            while status.borrow_and_update().running {
                status.changed().await.unwrap();
            }
        })
        .await;
        if cleanup.is_err() {
            supervisor_abort.abort();
        }
        cleanup.expect("Drop must retry the failed force before the original deadline");
        assert!(status.borrow().cleanup_error.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn dropping_collector_signals_owned_child_cleanup() {
        let (root, collector) = owned_fixture_with_mode("blocked").await;
        let mut status = collector.process_status.clone();
        drop(collector);
        tokio::time::timeout(Duration::from_secs(1), async {
            while status.borrow_and_update().running {
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(status.borrow().cleanup_error.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn maximum_response_body_with_crlf_is_accepted() {
        let (transport, peer, mut output) = fixture(8192);
        let request =
            tokio::spawn(async move { transport.request("cancel", serde_json::json!({})).await });
        let mut lines = BufReader::new(peer).lines();
        let incoming: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let mut frame = format!(
            "{{\"schema_version\":2,\"id\":{},\"ok\":true,\"result\":{{}}}}",
            incoming["id"]
        )
        .into_bytes();
        frame.resize(MAX_TRACY_RESPONSE_BYTES, b' ');
        frame.extend_from_slice(b"\r\n");
        output.write_all(&frame).await.unwrap();
        request.await.unwrap().unwrap();
    }

    fn fixture(
        capacity: usize,
    ) -> (
        CollectorTransport<tokio::io::DuplexStream>,
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) {
        let (read, peer_write) = duplex(capacity);
        let (peer_read, write) = duplex(capacity);
        (
            CollectorTransport::new(read, write, Duration::from_secs(2)),
            peer_read,
            peer_write,
        )
    }

    async fn response(peer: &mut tokio::io::DuplexStream, id: u64) {
        peer.write_all(
            format!("{{\"schema_version\":2,\"id\":{id},\"ok\":true,\"result\":{{}}}}\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn writer_queue_timeout_and_cancellation_leave_no_pending_entries() {
        let (transport, peer, mut output) = fixture(1024);
        let held = transport.writer.lock().await;
        assert!(matches!(
            transport
                .request_with_timeout("cancel", serde_json::json!({}), Duration::from_millis(1))
                .await,
            Err(TracyProtocolError::Timeout { .. })
        ));
        let request = tokio::spawn({
            let transport = transport.clone();
            async move { transport.request("cancel", serde_json::json!({})).await }
        });
        tokio::task::yield_now().await;
        request.abort();
        let _ = request.await;
        {
            let registry = transport.owner.0.lock().unwrap();
            assert!(
                registry.pending.is_empty()
                    && registry.abandoned.is_empty()
                    && registry.terminal.is_none()
            );
        }
        drop(held);
        let request = tokio::spawn({
            let transport = transport.clone();
            async move { transport.request("cancel", serde_json::json!({})).await }
        });
        let mut lines = BufReader::new(peer).lines();
        let request_json: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        response(&mut output, request_json["id"].as_u64().unwrap()).await;
        request.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancelled_partial_frame_fails_transport_and_clears_pending() {
        let (transport, mut peer, _output) = fixture(1);
        let request = tokio::spawn({
            let transport = transport.clone();
            async move { transport.request("cancel", serde_json::json!({})).await }
        });
        peer.read_exact(&mut [0u8; 1]).await.unwrap();
        request.abort();
        let _ = request.await;
        {
            let registry = transport.owner.0.lock().unwrap();
            assert!(registry.pending.is_empty());
            assert!(registry.terminal.is_some());
        }
        assert!(matches!(
            transport
                .request("session_status", serde_json::json!({}))
                .await,
            Err(TracyProtocolError::Transport(_))
        ));
    }

    #[tokio::test]
    async fn late_response_is_accepted_once_and_duplicate_is_terminal() {
        for cancel in [false, true] {
            let (transport, peer, mut output) = fixture(4096);
            let mut lines = BufReader::new(peer).lines();
            let first = tokio::spawn({
                let transport = transport.clone();
                async move {
                    transport
                        .request_with_timeout(
                            "cancel",
                            serde_json::json!({}),
                            Duration::from_millis(20),
                        )
                        .await
                }
            });
            let first_json: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let id = first_json["id"].as_u64().unwrap();
            if cancel {
                first.abort();
                let _ = first.await;
            } else {
                assert!(matches!(
                    first.await.unwrap(),
                    Err(TracyProtocolError::Timeout { .. })
                ));
            }
            assert_eq!(transport.owner.0.lock().unwrap().abandoned.len(), 1);
            let second = tokio::spawn({
                let transport = transport.clone();
                async move { transport.request("cancel", serde_json::json!({})).await }
            });
            let second_json: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            response(&mut output, id).await;
            response(&mut output, second_json["id"].as_u64().unwrap()).await;
            second.await.unwrap().unwrap();
            assert!(transport.owner.0.lock().unwrap().abandoned.is_empty());
            let mut failed = transport.owner.0.lock().unwrap().shutdown.subscribe();
            response(&mut output, id).await;
            tokio::time::timeout(Duration::from_secs(1), failed.changed())
                .await
                .unwrap()
                .unwrap();
            assert!(transport.owner.0.lock().unwrap().terminal.is_some());
        }
    }

    #[tokio::test]
    async fn write_failure_removes_pending_and_fails_transport() {
        let (transport, peer, _output) = fixture(1);
        drop(peer);
        assert!(matches!(
            transport.request("cancel", serde_json::json!({})).await,
            Err(TracyProtocolError::Transport(_))
        ));
        let registry = transport.owner.0.lock().unwrap();
        assert!(registry.pending.is_empty() && registry.terminal.is_some());
    }

    #[tokio::test]
    async fn abandoned_request_history_fails_closed_at_capacity() {
        let (transport, peer, _output) = fixture(4096);
        let mut lines = BufReader::new(peer).lines();
        for index in 0..=MAX_ABANDONED_REQUESTS {
            let request = tokio::spawn({
                let transport = transport.clone();
                async move { transport.request("cancel", serde_json::json!({})).await }
            });
            lines.next_line().await.unwrap().unwrap();
            request.abort();
            let _ = request.await;
            let registry = transport.owner.0.lock().unwrap();
            assert!(registry.pending.is_empty());
            assert!(registry.abandoned.len() <= MAX_ABANDONED_REQUESTS);
            assert_eq!(registry.terminal.is_some(), index == MAX_ABANDONED_REQUESTS);
        }
    }

    #[tokio::test]
    async fn bounded_line_drains_oversize_without_expanding_storage() {
        let bytes = [vec![b'x'; 10000], b"\nnext\r\n".to_vec()].concat();
        let mut reader = BufReader::new(bytes.as_slice());
        let (line, truncated) = bounded_line(&mut reader, 64, true).await.unwrap().unwrap();
        assert!(truncated);
        assert_eq!(line.len(), 64);
        assert_eq!(line.capacity(), 64);
        assert_eq!(
            bounded_line(&mut reader, 64, true)
                .await
                .unwrap()
                .unwrap()
                .0,
            b"next\r\n"
        );
    }

    #[test]
    fn session_start_parameters_preserve_the_requested_readiness_timeout() {
        let parameters = session_start_parameters("127.0.0.1", 8086, 60_000);

        assert_eq!(parameters["host"], "127.0.0.1");
        assert_eq!(parameters["port"], 8086);
        assert_eq!(parameters["connect_timeout_ms"], 60_000);
        assert_eq!(parameters["progress_timeout_ms"], 60_000);
    }

    #[test]
    fn helper_error_reports_whether_the_measurement_window_started() {
        let started = TracyProtocolError::Helper {
            code: "client_disconnected".to_owned(),
            message: "disconnected".to_owned(),
            details: serde_json::json!({"window_started": true}),
        };
        let not_started = TracyProtocolError::Helper {
            code: "connect_timeout".to_owned(),
            message: "timeout".to_owned(),
            details: serde_json::json!({"window_started": false}),
        };
        assert!(capture_window_started(&started));
        assert!(!capture_window_started(&not_started));
        assert!(!capture_window_started(&TracyProtocolError::Timeout {
            id: 1
        }));
        assert_eq!(
            capture_failure_code(&not_started),
            crate::result::ToolErrorCode::CaptureNotReady
        );
        assert_eq!(
            capture_failure_code(&started),
            crate::result::ToolErrorCode::HelperFailure
        );
    }
}
