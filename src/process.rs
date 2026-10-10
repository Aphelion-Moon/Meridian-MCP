pub(crate) mod output;
use crate::network_audit::{NetworkAuditCollector, NetworkAuditReport};
use anyhow::{Context, Result};
use serde::Serialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncRead;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};

pub const MAX_PROCESS_OUTPUT_BYTES: usize = 512 * 1024;
pub const MAX_PROCESS_INPUT_BYTES: usize = 1024 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(unix)]
mod unix_owner;

/// Configure the absolute Meridian executable used by Unix runtime guardians.
#[cfg(unix)]
pub fn initialize_runtime_owner_with_executable(path: &std::path::Path) -> Result<()> {
    unix_owner::initialize(path)
}

/// Dispatch the private guardian entry point before creating threads or services.
#[cfg(unix)]
pub fn dispatch_runtime_guardian() -> bool {
    unix_owner::dispatch()
}

#[cfg(windows)]
static RUNTIME_OWNER_JOB: std::sync::Mutex<Option<ProcessContainment>> =
    std::sync::Mutex::new(None);

/// Opt this process and its future children into cleanup when this process exits.
///
/// Call once at the entry point of a dedicated runtime owner, before spawning any
/// runtime. The Windows job handle is intentionally retained for the process
/// lifetime. Library hosts must explicitly opt in: subsequent children inherit
/// this job, including children launched outside Meridian-MCP.
pub fn initialize_runtime_owner() -> Result<()> {
    #[cfg(windows)]
    {
        let mut owner = RUNTIME_OWNER_JOB
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if owner.is_some() {
            return Ok(());
        }
        let job = ProcessContainment::new()?;
        // The current-process pseudo handle cannot fail to close after successful
        // assignment: there must be no fallible operation before retaining job.
        let assigned = unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                job.handle,
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
            )
        };
        if assigned == 0 {
            return Err(std::io::Error::last_os_error())
                .context("cannot establish runtime owner job");
        }
        *owner = Some(job);
        Ok(())
    }
    #[cfg(unix)]
    {
        let executable = std::env::current_exe()?;
        anyhow::ensure!(executable.file_name().is_some_and(|name| name == "meridian-mcp"),
            "library hosts must configure initialize_runtime_owner_with_executable with the Meridian executable");
        initialize_runtime_owner_with_executable(&executable)
    }
    #[cfg(not(any(windows, unix)))]
    anyhow::bail!("runtime tree ownership requires a native owner-loss supervisor; this platform is not yet supported")
}

/// Spawn without allowing user code to execute before per-runtime containment.
pub(crate) fn spawn_runtime_process(
    command: &mut Command,
) -> Result<(Child, std::sync::Arc<ProcessContainment>)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        if RUNTIME_OWNER_JOB
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_none()
        {
            anyhow::bail!("runtime owner is not initialized; call initialize_runtime_owner in a dedicated host before launching DreamDaemon");
        }
        let mut runtime_containment = ProcessContainment::new()?;
        *runtime_containment.completion_handles.get_mut().unwrap() = Some(Vec::new());
        let containment = std::sync::Arc::new(runtime_containment);
        command
            .kill_on_drop(true)
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .context("spawned runtime has no process identity")?;
        if let Err(error) = containment
            .assign(pid)
            .and_then(|()| resume_suspended_process(pid))
        {
            let _ = containment.terminate(1);
            let _ = child.start_kill();
            return Err(error).context("refusing to execute runtime outside containment");
        }
        Ok((child, containment))
    }
    #[cfg(unix)]
    {
        unix_owner::spawn(command)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = command;
        anyhow::bail!("runtime tree ownership requires a native owner-loss supervisor; this platform is not yet supported")
    }
}

#[cfg(windows)]
fn resume_suspended_process(pid: u32) -> Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let snapshot = OwnedHandle::from_raw_handle(snapshot);
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut available = Thread32First(snapshot.as_raw_handle(), &mut entry);
        while available != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let thread = OwnedHandle::from_raw_handle(thread);
                let previous_count = ResumeThread(thread.as_raw_handle());
                if previous_count == u32::MAX {
                    return Err(std::io::Error::last_os_error().into());
                }
                anyhow::ensure!(
                    previous_count == 1,
                    "runtime primary thread had an unexpected suspension count"
                );
                return Ok(());
            }
            available = Thread32Next(snapshot.as_raw_handle(), &mut entry);
        }
    }
    anyhow::bail!("suspended runtime primary thread was not found")
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoundedOutput {
    pub text: String,
    pub captured_bytes: usize,
    pub truncated_bytes: u64,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Exited,
    WallTimeout,
    IdleTimeout,
    Cancelled,
    SpawnFailed,
}

#[derive(Debug)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    pub idle_timeout: Duration,
    pub capture_network: bool,
    pub cancellation: Option<watch::Receiver<bool>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcessOutcome {
    pub process_started: bool,
    pub exit_code: Option<i32>,
    pub termination: TerminationReason,
    pub duration_ms: u128,
    pub stdout: BoundedOutput,
    pub stderr: BoundedOutput,
    /// Both pipes reached EOF and every received chunk was delivered to capture.
    pub output_complete: bool,
    pub network_audit: NetworkAuditReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

struct TailBuffer {
    bytes: Vec<u8>,
    truncated_bytes: u64,
}

impl TailBuffer {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            truncated_bytes: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= MAX_PROCESS_OUTPUT_BYTES {
            self.truncated_bytes = self
                .truncated_bytes
                .saturating_add(self.bytes.len() as u64)
                .saturating_add((chunk.len() - MAX_PROCESS_OUTPUT_BYTES) as u64);
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&chunk[chunk.len() - MAX_PROCESS_OUTPUT_BYTES..]);
            return;
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(MAX_PROCESS_OUTPUT_BYTES);
        if overflow != 0 {
            self.bytes.drain(..overflow);
            self.truncated_bytes = self.truncated_bytes.saturating_add(overflow as u64);
        }
        self.bytes.extend_from_slice(chunk);
    }

    fn finish(self) -> BoundedOutput {
        BoundedOutput {
            captured_bytes: self.bytes.len(),
            text: String::from_utf8_lossy(&self.bytes).into_owned(),
            truncated_bytes: self.truncated_bytes,
        }
    }
}

async fn capture_stream<R>(
    mut reader: R,
    stream: OutputStream,
    sender: mpsc::Sender<(OutputStream, Vec<u8>)>,
) -> bool
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = match tokio::io::AsyncReadExt::read(&mut reader, &mut buffer).await {
            Ok(0) => return true,
            Err(_) => return false,
            Ok(count) => count,
        };
        if sender
            .send((stream, buffer[..count].to_vec()))
            .await
            .is_err()
        {
            return false;
        }
    }
}

pub async fn run_contained_process(spec: ProcessSpec) -> Result<ProcessOutcome> {
    run_contained_process_observed(spec, |_, _| {}).await
}

/// Observe each chunk before tail eviction, including the final output drain.
/// Observers must do bounded synchronous work; the runner retains no extra log.
pub async fn run_contained_process_observed(
    spec: ProcessSpec,
    observe: impl FnMut(OutputStream, &[u8]) + Send,
) -> Result<ProcessOutcome> {
    run_process_observed(spec, observe, false).await
}

pub(crate) async fn run_owned_process_observed(
    spec: ProcessSpec,
    observe: impl FnMut(OutputStream, &[u8]) + Send,
) -> Result<ProcessOutcome> {
    run_process_observed(spec, observe, true).await
}

async fn run_process_observed(
    spec: ProcessSpec,
    mut observe: impl FnMut(OutputStream, &[u8]) + Send,
    owned: bool,
) -> Result<ProcessOutcome> {
    if spec
        .stdin
        .as_ref()
        .is_some_and(|input| input.len() > MAX_PROCESS_INPUT_BYTES)
    {
        anyhow::bail!("process stdin exceeds the {MAX_PROCESS_INPUT_BYTES}-byte limit");
    }
    let started_at = tokio::time::Instant::now();
    let mut audit = NetworkAuditCollector::new(spec.capture_network);
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.arguments)
        .current_dir(&spec.working_directory)
        .env_clear()
        .envs(spec.environment.iter().cloned())
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if spec
        .cancellation
        .as_ref()
        .is_some_and(|receiver| *receiver.borrow())
    {
        return Ok(ProcessOutcome {
            process_started: false,
            exit_code: None,
            termination: TerminationReason::Cancelled,
            duration_ms: started_at.elapsed().as_millis(),
            stdout: TailBuffer::new().finish(),
            stderr: TailBuffer::new().finish(),
            network_audit: audit.finish(),
            output_complete: true,
        });
    }
    let spawned = if owned {
        spawn_runtime_process(&mut command)
    } else {
        let containment = std::sync::Arc::new(
            ProcessContainment::new().context("cannot create process containment")?,
        );
        match command.spawn() {
            Ok(mut child) => {
                if let Err(error) = containment.assign(child.id().unwrap_or_default()) {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(error).context("refusing to run a process outside containment");
                }
                Ok((child, containment))
            }
            Err(error) => Err(error.into()),
        }
    };
    let (mut child, containment) = match spawned {
        Ok(spawned) => spawned,
        Err(error) => {
            if owned {
                // Owned spawn can fail after creating a child. Without its
                // completion proof the execution marker must stay active.
                return Err(error).context("owned spawn or setup did not establish cleanup");
            }
            let message = format!("failed to spawn {}: {error}", spec.program.display());
            return Ok(ProcessOutcome {
                process_started: false,
                exit_code: None,
                termination: TerminationReason::SpawnFailed,
                duration_ms: started_at.elapsed().as_millis(),
                stdout: TailBuffer::new().finish(),
                stderr: BoundedOutput {
                    captured_bytes: message.len(),
                    text: message,
                    truncated_bytes: 0,
                },
                network_audit: audit.finish(),
                output_complete: false,
            });
        }
    };
    let process_id = child.id().unwrap_or_default();

    let stdin_task = match (spec.stdin, child.stdin.take()) {
        (Some(input), Some(mut stdin)) => Some(tokio::spawn(async move {
            stdin.write_all(&input).await?;
            stdin.shutdown().await
        })),
        _ => None,
    };

    let (sender, mut receiver) = mpsc::channel(64);
    let mut reader_tasks = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        reader_tasks.push(tokio::spawn(capture_stream(
            stdout,
            OutputStream::Stdout,
            sender.clone(),
        )));
    }
    if let Some(stderr) = child.stderr.take() {
        reader_tasks.push(tokio::spawn(capture_stream(
            stderr,
            OutputStream::Stderr,
            sender.clone(),
        )));
    }

    let mut stdout = TailBuffer::new();
    let mut stderr = TailBuffer::new();
    let mut last_progress = started_at;
    let mut previous_cpu_time = containment.cpu_time_100ns();
    let mut cancellation = spec.cancellation;
    let (termination, exit_code) = loop {
        let now = tokio::time::Instant::now();
        if cancellation
            .as_ref()
            .is_some_and(|receiver| *receiver.borrow())
        {
            terminate_child(&containment, &mut child).await?;
            break (
                TerminationReason::Cancelled,
                child.wait().await.ok().and_then(|status| status.code()),
            );
        }
        if now.duration_since(started_at) >= spec.timeout {
            terminate_child(&containment, &mut child).await?;
            break (
                TerminationReason::WallTimeout,
                child.wait().await.ok().and_then(|s| s.code()),
            );
        }
        if now.duration_since(last_progress) >= spec.idle_timeout {
            terminate_child(&containment, &mut child).await?;
            break (
                TerminationReason::IdleTimeout,
                child.wait().await.ok().and_then(|s| s.code()),
            );
        }

        tokio::select! {
            status = child.wait() => {
                let status = status.context("failed waiting for contained process")?;
                break (TerminationReason::Exited, status.code());
            }
            output = receiver.recv() => {
                if let Some((stream, bytes)) = output {
                    last_progress = tokio::time::Instant::now();
                    observe(stream, &bytes);
                    append_output(&mut stdout, &mut stderr, stream, &bytes);
                }
            }
            changed = async {
                match cancellation.as_mut() {
                    Some(receiver) => receiver.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() || cancellation.as_ref().is_some_and(|receiver| *receiver.borrow()) {
                    terminate_child(&containment, &mut child).await?;
                    break (
                        TerminationReason::Cancelled,
                        child.wait().await.ok().and_then(|status| status.code()),
                    );
                }
            }
            _ = tokio::time::sleep(PROCESS_POLL_INTERVAL) => {}
        }

        if let Some(cpu_time) = containment.cpu_time_100ns() {
            if previous_cpu_time.is_none_or(|previous| cpu_time > previous) {
                last_progress = tokio::time::Instant::now();
            }
            previous_cpu_time = Some(cpu_time);
        }
        let process_ids = containment.process_ids(process_id);
        audit.sample(&process_ids, started_at.elapsed().as_millis());
    };

    let process_ids = containment.process_ids(process_id);
    audit.sample(&process_ids, started_at.elapsed().as_millis());
    if owned {
        wait_for_cleanup(&containment).await?;
    } else {
        let _ = containment.terminate(1);
    }
    drop(sender);
    let output_complete = drain_output(
        &mut receiver,
        &mut stdout,
        &mut stderr,
        &mut reader_tasks,
        &mut observe,
    )
    .await;
    if let Some(task) = stdin_task {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if termination == TerminationReason::Exited => {
                return Err(error).context("failed writing bounded process stdin");
            }
            Err(error) if termination == TerminationReason::Exited => {
                return Err(error).context("bounded process stdin task failed");
            }
            Ok(Err(_)) | Err(_) => {}
        }
    }

    Ok(ProcessOutcome {
        process_started: true,
        exit_code,
        termination,
        duration_ms: started_at.elapsed().as_millis(),
        stdout: stdout.finish(),
        stderr: stderr.finish(),
        output_complete,
        network_audit: audit.finish(),
    })
}

pub(crate) async fn wait_for_cleanup(containment: &ProcessContainment) -> Result<()> {
    containment.request_termination()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !containment.is_terminated()? {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "owned process cleanup remains unconfirmed"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
}

fn append_output(
    stdout: &mut TailBuffer,
    stderr: &mut TailBuffer,
    stream: OutputStream,
    bytes: &[u8],
) {
    match stream {
        OutputStream::Stdout => stdout.push(bytes),
        OutputStream::Stderr => stderr.push(bytes),
    }
}

async fn terminate_child(containment: &ProcessContainment, _child: &mut Child) -> Result<()> {
    containment.terminate(1)?;
    #[cfg(not(windows))]
    {
        let _ = _child.kill().await;
    }
    Ok(())
}

async fn drain_output(
    receiver: &mut mpsc::Receiver<(OutputStream, Vec<u8>)>,
    stdout: &mut TailBuffer,
    stderr: &mut TailBuffer,
    reader_tasks: &mut Vec<tokio::task::JoinHandle<bool>>,
    observe: &mut (impl FnMut(OutputStream, &[u8]) + Send),
) -> bool {
    let deadline = tokio::time::Instant::now() + OUTPUT_DRAIN_TIMEOUT;
    let mut complete = loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break false;
        }
        match tokio::time::timeout(deadline - now, receiver.recv()).await {
            Ok(Some((stream, bytes))) => {
                observe(stream, &bytes);
                append_output(stdout, stderr, stream, &bytes);
            }
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    let drained = complete;
    for task in reader_tasks.drain(..) {
        if !drained && !task.is_finished() {
            task.abort();
        }
        complete &= matches!(task.await, Ok(true));
    }
    // Preserve chunks already received before a drain deadline or reader abort.
    while let Ok((stream, bytes)) = receiver.try_recv() {
        observe(stream, &bytes);
        append_output(stdout, stderr, stream, &bytes);
    }
    complete
}

#[cfg(test)]
mod output_capture_tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context as TaskContext, Poll};
    use tokio::io::ReadBuf;

    struct FailedRead;
    impl AsyncRead for FailedRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut TaskContext<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::Error::other("owned read failure")))
        }
    }

    #[tokio::test]
    async fn output_drain_deadline_preserves_received_bytes_but_marks_incomplete() {
        let (mut writer, reader) = tokio::io::duplex(32);
        writer.write_all(b"before stalled EOF").await.unwrap();
        let (sender, mut receiver) = mpsc::channel(2);
        let mut tasks = vec![tokio::spawn(capture_stream(
            reader,
            OutputStream::Stdout,
            sender,
        ))];
        let mut stdout = TailBuffer::new();
        let mut stderr = TailBuffer::new();
        let mut observed = Vec::new();
        let complete = drain_output(
            &mut receiver,
            &mut stdout,
            &mut stderr,
            &mut tasks,
            &mut |_, bytes| observed.extend_from_slice(bytes),
        )
        .await;
        assert!(!complete);
        assert_eq!(observed, b"before stalled EOF");
        assert_eq!(stdout.finish().text, "before stalled EOF");
        assert!(tasks.is_empty());
        drop(writer);
    }

    #[tokio::test]
    async fn output_read_errors_are_distinguished_from_clean_eof() {
        for failed in [false, true] {
            let (sender, mut receiver) = mpsc::channel(2);
            let task = if failed {
                tokio::spawn(capture_stream(FailedRead, OutputStream::Stdout, sender))
            } else {
                tokio::spawn(capture_stream(
                    &b"final bytes"[..],
                    OutputStream::Stdout,
                    sender,
                ))
            };
            let mut stdout = TailBuffer::new();
            let mut stderr = TailBuffer::new();
            let mut observed = Vec::new();
            let complete = drain_output(
                &mut receiver,
                &mut stdout,
                &mut stderr,
                &mut vec![task],
                &mut |stream, bytes| {
                    assert_eq!(stream, OutputStream::Stdout);
                    observed.extend_from_slice(bytes);
                },
            )
            .await;
            assert_eq!(complete, !failed);
            assert_eq!(
                observed,
                if failed {
                    &b""[..]
                } else {
                    &b"final bytes"[..]
                }
            );
        }
    }
}

#[cfg(not(windows))]
pub(crate) struct ProcessContainment {
    #[cfg(test)]
    cleanup_fault: std::sync::atomic::AtomicU8,
    #[cfg(unix)]
    runtime: Option<unix_owner::RuntimeOwner>,
}

#[cfg(not(windows))]
impl ProcessContainment {
    pub(crate) fn request_termination(&self) -> Result<()> {
        #[cfg(test)]
        self.check_cleanup_fault(1)?;
        #[cfg(unix)]
        if let Some(runtime) = &self.runtime {
            return runtime.request_termination();
        }
        self.terminate(1)
    }

    pub(crate) fn is_terminated(&self) -> Result<bool> {
        #[cfg(test)]
        self.check_cleanup_fault(2)?;
        #[cfg(test)]
        if self.cleanup_fault.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            return Ok(false);
        }
        #[cfg(unix)]
        if let Some(runtime) = &self.runtime {
            return runtime.is_terminated();
        }
        Ok(true)
    }

    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            #[cfg(test)]
            cleanup_fault: std::sync::atomic::AtomicU8::new(0),
            #[cfg(unix)]
            runtime: None,
        })
    }
    pub(crate) fn assign(&self, _process_id: u32) -> Result<()> {
        Ok(())
    }
    pub(crate) fn terminate(&self, _exit_code: u32) -> Result<()> {
        #[cfg(unix)]
        if let Some(runtime) = &self.runtime {
            return runtime.terminate();
        }
        Ok(())
    }
    fn cpu_time_100ns(&self) -> Option<u64> {
        None
    }
    fn process_ids(&self, root_process_id: u32) -> Vec<u32> {
        vec![root_process_id]
    }
}

#[cfg(windows)]
pub(crate) struct ProcessContainment {
    #[cfg(test)]
    cleanup_fault: std::sync::atomic::AtomicU8,
    handle: windows_sys::Win32::Foundation::HANDLE,
    completion_handles: std::sync::Mutex<Option<Vec<std::os::windows::io::OwnedHandle>>>,
    termination_requested: std::sync::atomic::AtomicBool,
}

#[cfg(windows)]
unsafe impl Send for ProcessContainment {}
#[cfg(windows)]
unsafe impl Sync for ProcessContainment {}

#[cfg(windows)]
impl ProcessContainment {
    pub(crate) fn request_termination(&self) -> Result<()> {
        #[cfg(test)]
        self.check_cleanup_fault(1)?;
        self.terminate(1)
    }

    pub(crate) fn is_terminated(&self) -> Result<bool> {
        #[cfg(test)]
        self.check_cleanup_fault(2)?;
        #[cfg(test)]
        if self.cleanup_fault.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            return Ok(false);
        }
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        let mut captured = self
            .completion_handles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(handles) = captured.as_mut() {
            let mut wait_error = None;
            handles.retain(|handle| {
                match unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(
                        handle.as_raw_handle(),
                        0,
                    )
                } {
                    WAIT_OBJECT_0 => false,
                    WAIT_TIMEOUT => true,
                    _ => {
                        wait_error = Some(std::io::Error::last_os_error());
                        true
                    }
                }
            });
            if let Some(error) = wait_error {
                return Err(error.into());
            }
            if !handles.is_empty() {
                return Ok(false);
            }
        }
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let result = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&accounting) as u32,
                std::ptr::null_mut(),
            )
        };
        anyhow::ensure!(
            result != 0,
            "cannot query runtime job cleanup: {}",
            std::io::Error::last_os_error()
        );
        Ok(accounting.ActiveProcesses == 0)
    }

    pub(crate) fn new() -> Result<Self> {
        use std::mem::size_of;
        use std::ptr;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if configured == 0 {
                let error = std::io::Error::last_os_error();
                windows_sys::Win32::Foundation::CloseHandle(handle);
                return Err(error.into());
            }
            Ok(Self {
                handle,
                completion_handles: std::sync::Mutex::new(None),
                termination_requested: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                cleanup_fault: std::sync::atomic::AtomicU8::new(0),
            })
        }
    }

    pub(crate) fn assign(&self, process_id: u32) -> Result<()> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        unsafe {
            let process = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_TERMINATE,
                0,
                process_id,
            );
            if process.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let assigned = AssignProcessToJobObject(self.handle, process);
            let assign_error = (assigned == 0).then(std::io::Error::last_os_error);
            let closed = CloseHandle(process);
            if let Some(error) = assign_error {
                return Err(error.into());
            }
            if closed == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }
    }

    pub(crate) fn terminate(&self, exit_code: u32) -> Result<()> {
        let mut captured = self
            .completion_handles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(handles) = captured.as_mut() {
            if !self
                .termination_requested
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                *handles = self.capture_job_process_handles()?;
            }
        }
        let terminated = unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, exit_code)
        };
        if terminated == 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            self.termination_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn capture_job_process_handles(&self) -> Result<Vec<std::os::windows::io::OwnedHandle>> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_MORE_DATA};
        use windows_sys::Win32::System::JobObjects::{
            IsProcessInJob, JobObjectBasicProcessIdList, QueryInformationJobObject,
            JOBOBJECT_BASIC_PROCESS_ID_LIST,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };
        let mut storage = vec![0_usize; 65];
        loop {
            let result = unsafe {
                QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicProcessIdList,
                    storage.as_mut_ptr().cast(),
                    std::mem::size_of_val(storage.as_slice()) as u32,
                    std::ptr::null_mut(),
                )
            };
            if result != 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                return Err(error.into());
            }
            anyhow::ensure!(
                storage.len() < 65537,
                "runtime job exceeds bounded cleanup enumeration"
            );
            storage.resize((storage.len() * 2).min(65537), 0);
        }
        let list = unsafe { &*storage.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
        let count = list.NumberOfProcessIdsInList as usize;
        anyhow::ensure!(
            count < storage.len(),
            "invalid runtime job membership count"
        );
        let ids = unsafe { std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), count) };
        let mut handles = Vec::with_capacity(count);
        for &pid in ids {
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid as u32,
                )
            };
            if handle.is_null() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                    continue;
                }
                return Err(error.into());
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
            let mut member = 0;
            anyhow::ensure!(
                unsafe { IsProcessInJob(handle.as_raw_handle(), self.handle, &mut member) } != 0,
                "cannot validate runtime cleanup process: {}",
                std::io::Error::last_os_error()
            );
            if member != 0 {
                handles.push(handle);
            }
        }
        Ok(handles)
    }

    fn cpu_time_100ns(&self) -> Option<u64> {
        use std::mem::size_of;
        use std::ptr;
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAndIoAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION,
        };

        unsafe {
            let mut accounting = JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION::default();
            let queried = QueryInformationJobObject(
                self.handle,
                JobObjectBasicAndIoAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION>() as u32,
                ptr::null_mut(),
            );
            (queried != 0).then(|| {
                accounting
                    .BasicInfo
                    .TotalUserTime
                    .saturating_add(accounting.BasicInfo.TotalKernelTime)
                    .max(0) as u64
            })
        }
    }

    fn process_ids(&self, root_process_id: u32) -> Vec<u32> {
        use std::mem::size_of_val;
        use std::ptr;
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
        };

        let mut storage = vec![0_usize; 1024];
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicProcessIdList,
                storage.as_mut_ptr().cast(),
                size_of_val(storage.as_slice()) as u32,
                ptr::null_mut(),
            )
        };
        if queried == 0 {
            return vec![root_process_id];
        }
        let list = unsafe { &*(storage.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()) };
        let count = (list.NumberOfProcessIdsInList as usize).min(storage.len().saturating_sub(1));
        let ids = unsafe { std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), count) };
        let mut output = ids.iter().map(|id| *id as u32).collect::<Vec<_>>();
        if output.is_empty() {
            output.push(root_process_id);
        }
        output
    }
}

#[cfg(windows)]
impl Drop for ProcessContainment {
    fn drop(&mut self) {
        let closed = unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
        debug_assert_ne!(closed, 0, "failed to close Windows Job Object handle");
    }
}

#[cfg(test)]
impl ProcessContainment {
    pub(crate) fn inject_cleanup_fault(&self, operation: u8) {
        self.cleanup_fault
            .store(operation, std::sync::atomic::Ordering::SeqCst);
    }

    fn check_cleanup_fault(&self, operation: u8) -> Result<()> {
        anyhow::ensure!(
            self.cleanup_fault.load(std::sync::atomic::Ordering::SeqCst) != operation,
            "injected containment cleanup failure"
        );
        Ok(())
    }
}

#[cfg(test)]
mod runtime_ownership_tests {
    #[cfg(unix)]
    #[tokio::test]
    async fn unix_runtime_preserves_real_child_exit() {
        let executable = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("meridian-mcp");
        super::initialize_runtime_owner_with_executable(&executable).unwrap();
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "exit 37"]);
        let (mut child, containment) = super::spawn_runtime_process(&mut command).unwrap();
        assert!(child.id().is_some());
        assert_eq!(child.wait().await.unwrap().code(), Some(37));
        containment.terminate(1).unwrap();
    }
    #[cfg(not(any(windows, unix)))]
    #[test]
    fn unsupported_owner_loss_supervision_refuses_before_spawn() {
        let marker = std::env::temp_dir().join(format!(
            "meridian-unsupported-runtime-{}",
            std::process::id()
        ));
        let mut command = tokio::process::Command::new("touch");
        command.arg(&marker);
        assert!(super::initialize_runtime_owner().is_err());
        assert!(super::spawn_runtime_process(&mut command).is_err());
        assert!(!marker.exists());
    }

    #[cfg(windows)]
    #[test]
    fn library_host_must_explicitly_opt_into_runtime_ownership() {
        if std::env::var_os("MERIDIAN_UNINITIALIZED_OWNER_FIXTURE").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "process::runtime_ownership_tests::library_host_must_explicitly_opt_into_runtime_ownership"])
                .env("MERIDIAN_UNINITIALIZED_OWNER_FIXTURE", "1")
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let mut command = tokio::process::Command::new("not-a-real-executable");
        let error = super::spawn_runtime_process(&mut command).err().unwrap();
        assert!(error.to_string().contains("initialize_runtime_owner"));
    }
}
