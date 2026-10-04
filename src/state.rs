use anyhow::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tokio::process::Child;
use tokio::sync::{Mutex, MutexGuard, RwLock};
use tokio::task::JoinHandle;

use crate::analysis_snapshot::{AnalysisBuild, AnalysisSnapshot};
use crate::spaceman::debugger::DebuggerSession;
use crate::spaceman::dmi::DmiCache;

pub(crate) const OUTPUT_LOG_CAPACITY: usize = 500;
pub(crate) const OUTPUT_LINE_MAX_BYTES: usize = 16 * 1024;
pub(crate) const OUTPUT_LOG_MAX_BYTES: usize = 1024 * 1024;
pub(crate) const OUTPUT_TRUNCATED_SUFFIX: &str = "... [truncated]";
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RuntimeOutputEntry {
    pub sequence: u64,
    pub monotonic_offset_ms: u64,
    pub text: String,
}

pub struct RuntimeOutputBuffer {
    pub(crate) entries: VecDeque<RuntimeOutputEntry>,
    started_at: Instant,
    next_sequence: u64,
    pub(crate) running: bool,
    pub(crate) drained: bool,
    pub(crate) last_exit_code: Option<i32>,
    pub(crate) changes: tokio::sync::watch::Sender<u64>,
}

impl Default for RuntimeOutputBuffer {
    fn default() -> Self {
        Self {
            entries: VecDeque::with_capacity(OUTPUT_LOG_CAPACITY),
            started_at: Instant::now(),
            next_sequence: 1,
            running: false,
            drained: true,
            last_exit_code: None,
            changes: tokio::sync::watch::channel(0).0,
        }
    }
}

pub type OutputLog = Arc<StdMutex<RuntimeOutputBuffer>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Standard,
    Tracy,
}

pub fn push_output_line(log: &OutputLog, line: String) {
    let offset_ms = log
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .started_at
        .elapsed()
        .as_millis() as u64;
    push_output_line_at(log, offset_ms, line);
}

pub fn push_output_line_at(log: &OutputLog, monotonic_offset_ms: u64, line: String) {
    let line = truncate_output_line(line);
    let mut buffer = log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if buffer.entries.len() >= OUTPUT_LOG_CAPACITY {
        buffer.entries.pop_front();
    }
    let sequence = buffer.next_sequence;
    buffer.next_sequence = buffer.next_sequence.saturating_add(1);
    buffer.entries.push_back(RuntimeOutputEntry {
        sequence,
        monotonic_offset_ms,
        text: line,
    });

    while output_log_byte_len(&buffer.entries) > OUTPUT_LOG_MAX_BYTES {
        buffer.entries.pop_front();
    }
    buffer
        .changes
        .send_modify(|version| *version = version.wrapping_add(1));
}

pub fn nearest_output_before(log: &OutputLog, offset_ms: u64) -> Option<RuntimeOutputEntry> {
    log.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entries
        .iter()
        .rev()
        .find(|entry| entry.monotonic_offset_ms <= offset_ms)
        .cloned()
}

fn truncate_output_line(line: String) -> String {
    if line.len() <= OUTPUT_LINE_MAX_BYTES {
        return line;
    }

    let content_limit = OUTPUT_LINE_MAX_BYTES.saturating_sub(OUTPUT_TRUNCATED_SUFFIX.len());
    let mut split_at = content_limit;
    while !line.is_char_boundary(split_at) {
        split_at -= 1;
    }

    let mut truncated = line[..split_at].to_string();
    truncated.push_str(OUTPUT_TRUNCATED_SUFFIX);
    truncated
}

fn output_log_byte_len(lines: &VecDeque<RuntimeOutputEntry>) -> usize {
    lines.iter().map(|line| line.text.len()).sum()
}

#[derive(Default)]
struct AnalysisState {
    active: Option<Arc<AnalysisSnapshot>>,
    generation: u64,
}

#[derive(Clone, Default)]
pub(crate) struct AnalysisMetadata {
    pub(crate) active_environment: Option<std::path::PathBuf>,
    pub(crate) generation: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("No environment loaded. Call dm_parse_environment first.")]
    ParseRequired,
}

pub struct RuntimeState {
    pub(crate) game_process: Option<Child>,
    pub(crate) containment: Option<Arc<crate::process::ProcessContainment>>,
    pub(crate) game_port: Option<u16>,
    pub(crate) output_log: OutputLog,
    pub(crate) runtime_output_tasks: Vec<JoinHandle<()>>,
    runtime_observer: Option<JoinHandle<()>>,
    pub(crate) last_exit_code: Option<i32>,
    pub(crate) kind: Option<RuntimeKind>,
    pub(crate) profiler_port: Option<u16>,
    pub(crate) launch_provenance: Option<crate::LaunchProvenance>,
    pub(crate) integrity: Option<Arc<Mutex<crate::runtime_integrity::RuntimeIntegritySession>>>,
    pub(crate) integrity_stop: Option<tokio::sync::watch::Sender<bool>>,
    pub(crate) integrity_task: Option<JoinHandle<()>>,
    pub(crate) integrity_summary: Option<crate::runtime_integrity::RuntimeIntegritySummary>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RuntimeStatus {
    pub running: bool,
    pub kind: Option<RuntimeKind>,
    pub game_port: Option<u16>,
    pub profiler_port: Option<u16>,
    pub last_exit_code: Option<i32>,
    pub recent_output_lines: usize,
    pub launch_provenance: Option<crate::LaunchProvenance>,
    pub integrity: Option<crate::runtime_integrity::RuntimeIntegritySummary>,
}

impl RuntimeState {
    fn new() -> Self {
        Self {
            game_process: None,
            containment: None,
            game_port: None,
            output_log: OutputLog::default(),
            runtime_output_tasks: Vec::new(),
            runtime_observer: None,
            last_exit_code: None,
            kind: None,
            profiler_port: None,
            launch_provenance: None,
            integrity: None,
            integrity_stop: None,
            integrity_task: None,
            integrity_summary: None,
        }
    }

    pub(crate) fn set_game_process(
        &mut self,
        process: Child,
        port: u16,
        launch_provenance: crate::LaunchProvenance,
    ) {
        self.game_process = Some(process);
        self.game_port = Some(port);
        self.last_exit_code = None;
        self.kind = Some(RuntimeKind::Standard);
        self.profiler_port = None;
        self.launch_provenance = Some(launch_provenance);
        self.publish_process_status(true);
    }

    pub(crate) fn set_profiled_game_process(
        &mut self,
        process: Child,
        port: u16,
        profiler_port: u16,
        launch_provenance: crate::LaunchProvenance,
    ) {
        self.set_game_process(process, port, launch_provenance);
        self.kind = Some(RuntimeKind::Tracy);
        self.profiler_port = Some(profiler_port);
    }

    pub(crate) fn clear_runtime_diagnostics(&mut self) {
        self.abort_runtime_output_tasks();
        if let Some(observer) = self.runtime_observer.take() {
            observer.abort();
        }
        self.output_log = OutputLog::default();
        self.last_exit_code = None;
    }

    pub(crate) fn add_runtime_output_task(&mut self, task: JoinHandle<()>) {
        self.runtime_output_tasks.push(task);
    }

    fn abort_runtime_output_tasks(&mut self) {
        for task in self.runtime_output_tasks.drain(..) {
            task.abort();
        }
        let mut output = self
            .output_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        output.drained = true;
        output
            .changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub(crate) fn recent_output(&self, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }

        let lines = self
            .output_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = lines.entries.len().saturating_sub(limit);
        lines
            .entries
            .iter()
            .skip(start)
            .map(|entry| entry.text.clone())
            .collect()
    }

    pub(crate) fn recent_output_entries(&self, limit: usize) -> Vec<RuntimeOutputEntry> {
        let lines = self
            .output_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = lines.entries.len().saturating_sub(limit);
        lines.entries.iter().skip(start).cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn matches_output(
        &self,
        pattern: &str,
        use_regex: bool,
    ) -> Result<bool, regex::Error> {
        let output = self.recent_output(OUTPUT_LOG_CAPACITY).join("\n");
        if use_regex {
            Ok(regex::Regex::new(pattern)?.is_match(&output))
        } else {
            Ok(output.contains(pattern))
        }
    }

    pub(crate) fn is_game_running(&mut self) -> bool {
        if let Some(ref mut process) = self.game_process {
            match process.try_wait() {
                Ok(Some(status)) => {
                    self.last_exit_code = status.code();
                    self.game_process = None;
                    if let Some(containment) = &self.containment {
                        let _ = containment.request_termination();
                    }
                    self.game_port = None;
                    self.kind = None;
                    self.profiler_port = None;
                    self.publish_process_status(false);
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    if let Some(containment) = &self.containment {
                        let _ = containment.request_termination();
                    }
                    self.game_process = None;
                    self.game_port = None;
                    self.kind = None;
                    self.profiler_port = None;
                    self.publish_process_status(false);
                    false
                }
            }
        } else {
            false
        }
    }

    fn publish_process_status(&self, running: bool) {
        let mut output = self
            .output_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        output.running = running;
        if running {
            output.drained = false;
        }
        output.last_exit_code = self.last_exit_code;
        output
            .changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub(crate) fn status_summary(&mut self) -> RuntimeStatus {
        let running = self.is_game_running();
        RuntimeStatus {
            running,
            kind: self.kind,
            game_port: self.game_port,
            profiler_port: self.profiler_port,
            last_exit_code: self.last_exit_code,
            recent_output_lines: self
                .output_log
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entries
                .len(),
            launch_provenance: self.launch_provenance.clone(),
            integrity: self.integrity_summary.clone(),
        }
    }

    pub(crate) async fn stop_game_process(&mut self) -> Result<()> {
        if let Some(containment) = &self.containment {
            containment.request_termination()?;
        }
        if let Some(mut process) = self.game_process.take() {
            process.kill().await?;
            let status = process.wait().await?;
            self.last_exit_code = status.code();
        }
        self.finish_runtime_cleanup().await?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.abort_runtime_output_tasks();
        self.game_port = None;
        self.kind = None;
        self.profiler_port = None;
        self.publish_process_status(false);
        Ok(())
    }

    pub(crate) async fn finish_runtime_cleanup(&mut self) -> Result<()> {
        let Some(containment) = &self.containment else {
            return Ok(());
        };
        containment.request_termination()?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !containment.is_terminated()? {
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "runtime containment cleanup timed out"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.containment = None;
        Ok(())
    }
}

impl Drop for RuntimeState {
    fn drop(&mut self) {
        if let Some(containment) = self.containment.take() {
            let _ = containment.terminate(1);
        }
        if let Some(process) = &mut self.game_process {
            let _ = process.start_kill();
        }
        self.abort_runtime_output_tasks();
        self.publish_process_status(false);
        if let Some(observer) = self.runtime_observer.take() {
            observer.abort();
        }
        if let Some(task) = self.integrity_task.take() {
            task.abort();
        }
    }
}

#[derive(Default)]
pub struct TracyCaptureState {
    pub(crate) active: bool,
    pub(crate) output_path: Option<std::path::PathBuf>,
    pub(crate) last_error: Option<String>,
    pub(crate) collector: Option<Arc<crate::tracy_collector::TracyCollector>>,
    pub(crate) phase: Option<crate::tracy_collector::TracySessionPhase>,
    pub(crate) last_status: Option<serde_json::Value>,
    pub(crate) integrity: Option<crate::workspace_integrity::IntegrityBaseline>,
    pub(crate) integrity_journal: Option<crate::workspace_integrity::IntegrityJournal>,
    pub(crate) integrity_owned_paths: Vec<std::path::PathBuf>,
    pub(crate) experiment: Option<crate::tracy_experiment::ExperimentState>,
    pub(crate) used_phases: std::collections::BTreeSet<(String, u32)>,
    pub(crate) memory_series:
        Option<Arc<tokio::sync::Mutex<Vec<crate::process_metrics::RoleMemorySeries>>>>,
    pub(crate) memory_stop: Option<tokio::sync::watch::Sender<bool>>,
    pub(crate) memory_task: Option<JoinHandle<()>>,
    pub(crate) experiment_started_at: Option<tokio::time::Instant>,
    pub(crate) capture_records: Vec<serde_json::Value>,
    pub(crate) diagnostic_records: Vec<serde_json::Value>,
    pub(crate) network_records: Vec<serde_json::Value>,
    pub(crate) wake_client: Option<TracyWakeClient>,
}

pub(crate) struct TracyWakeClient {
    pub(crate) process: Child,
    pub(crate) containment: crate::process::ProcessContainment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TracyCaptureStartError {
    Active,
    PhaseAlreadyUsed,
}

impl TracyCaptureState {
    pub(crate) fn begin_capture(
        &mut self,
        phase: &str,
        phase_iteration: u32,
    ) -> Result<(), TracyCaptureStartError> {
        if self.active {
            return Err(TracyCaptureStartError::Active);
        }
        if self
            .used_phases
            .contains(&(phase.to_owned(), phase_iteration))
        {
            return Err(TracyCaptureStartError::PhaseAlreadyUsed);
        }
        self.active = true;
        Ok(())
    }

    pub(crate) fn finish_capture(
        &mut self,
        phase: &str,
        phase_iteration: u32,
        window_started: bool,
    ) {
        self.active = false;
        if window_started {
            self.used_phases.insert((phase.to_owned(), phase_iteration));
        }
    }

    pub(crate) fn record_diagnostic(
        &mut self,
        record: serde_json::Value,
        owned_paths: impl IntoIterator<Item = std::path::PathBuf>,
    ) {
        self.diagnostic_records.push(record);
        self.integrity_owned_paths.extend(owned_paths);
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ServerState {
    analysis: RwLock<AnalysisState>,
    /// Published metadata has its own short, synchronous lock so deadline
    /// reporting never queues behind analysis readers or writers.
    analysis_metadata: StdMutex<AnalysisMetadata>,
    runtime: Arc<Mutex<RuntimeState>>,
    assets: Arc<Mutex<DmiCache>>,
    asset_limits: crate::limits::ServerLimits,
    asset_jobs: Arc<tokio::sync::Semaphore>,
    debugger: Mutex<Option<DebuggerSession>>,
    lifecycle: Mutex<()>,
    /// Serializes environment parses. Held for the whole build so two callers
    /// never hold two complete object trees at once, and kept separate from
    /// `lifecycle` so a long parse does not block runtime launches.
    parse: Arc<Mutex<()>>,
    #[cfg(test)]
    pub(crate) parse_worker_test: Arc<ParseWorkerTestControl>,
    tracy_capture: Mutex<TracyCaptureState>,
}

impl ServerState {
    pub fn new() -> Self {
        Self::with_limits(crate::limits::ServerLimits::default())
    }

    pub fn with_limits(limits: crate::limits::ServerLimits) -> Self {
        Self {
            analysis: RwLock::new(AnalysisState::default()),
            analysis_metadata: StdMutex::new(AnalysisMetadata::default()),
            runtime: Arc::new(Mutex::new(RuntimeState::new())),
            assets: Arc::new(Mutex::new(DmiCache::default())),
            asset_jobs: Arc::new(tokio::sync::Semaphore::new(limits.max_blocking_jobs.max(1))),
            asset_limits: limits,
            debugger: Mutex::new(None),
            lifecycle: Mutex::new(()),
            parse: Arc::new(Mutex::new(())),
            #[cfg(test)]
            parse_worker_test: Arc::default(),
            tracy_capture: Mutex::new(TracyCaptureState::default()),
        }
    }

    pub async fn snapshot(&self) -> Result<Arc<AnalysisSnapshot>, StateError> {
        self.analysis
            .read()
            .await
            .active
            .clone()
            .ok_or(StateError::ParseRequired)
    }

    /// Build a private snapshot without bypassing this server's parser admission.
    /// The parser moves the shared permit into its worker, retaining it even if
    /// the caller is cancelled before the worker finishes.
    pub(crate) fn isolated_analysis_state(&self) -> Self {
        let mut isolated = Self::with_limits(self.asset_limits.clone());
        isolated.parse = Arc::clone(&self.parse);
        isolated
    }

    pub async fn active_snapshot(&self) -> Option<Arc<AnalysisSnapshot>> {
        self.analysis.read().await.active.clone()
    }

    #[cfg(test)]
    pub(crate) async fn hold_analysis_write(&self) -> AnalysisWriteTestGuard<'_> {
        AnalysisWriteTestGuard {
            _guard: self.analysis.write().await,
        }
    }

    pub async fn install_analysis(&self, build: AnalysisBuild) -> Arc<AnalysisSnapshot> {
        let snapshot = AnalysisSnapshot::from_build(build, 0);
        let mut state = self.analysis.write().await;
        let (snapshot, previous) = self.publish_analysis(&mut state, snapshot);
        drop(state);
        drop(previous);
        snapshot
    }

    pub(crate) async fn install_analysis_before_deadline(
        &self,
        snapshot: AnalysisSnapshot,
        deadline: tokio::time::Instant,
    ) -> Option<Arc<AnalysisSnapshot>> {
        let mut state = tokio::time::timeout_at(deadline, self.analysis.write())
            .await
            .ok()?;
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        let (snapshot, previous) = self.publish_analysis(&mut state, snapshot);
        drop(state);
        drop(previous);
        Some(snapshot)
    }

    fn publish_analysis(
        &self,
        state: &mut AnalysisState,
        mut snapshot: AnalysisSnapshot,
    ) -> (Arc<AnalysisSnapshot>, Option<Arc<AnalysisSnapshot>>) {
        snapshot.generation = state.generation.saturating_add(1);
        let snapshot = Arc::new(snapshot);
        let mut metadata = self
            .analysis_metadata
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = state.active.replace(Arc::clone(&snapshot));
        state.generation = snapshot.generation;
        *metadata = AnalysisMetadata {
            active_environment: Some(snapshot.environment_path.clone()),
            generation: snapshot.generation,
        };
        (snapshot, previous)
    }

    pub(crate) fn analysis_metadata(&self) -> AnalysisMetadata {
        self.analysis_metadata
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub async fn state_generation(&self) -> u64 {
        self.analysis.read().await.generation
    }

    pub async fn clear_analysis(&self) {
        let mut state = self.analysis.write().await;
        let previous = state.active.take();
        self.analysis_metadata
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active_environment = None;
        drop(state);
        drop(previous);
    }

    pub(crate) async fn runtime(&self) -> MutexGuard<'_, RuntimeState> {
        self.runtime.lock().await
    }

    pub(crate) fn observe_runtime(&self, runtime: &mut RuntimeState) {
        if runtime
            .runtime_observer
            .as_ref()
            .is_some_and(|task| !task.is_finished())
        {
            return;
        }
        let owner = Arc::downgrade(&self.runtime);
        let session = Arc::downgrade(&runtime.output_log);
        runtime.runtime_observer = Some(tokio::spawn(async move {
            let mut exited_at = None;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let Some(owner) = owner.upgrade() else {
                    return;
                };
                let Ok(mut runtime) = owner.try_lock() else {
                    continue;
                };
                if !std::sync::Weak::ptr_eq(&session, &Arc::downgrade(&runtime.output_log)) {
                    return;
                }
                if !runtime.is_game_running() {
                    // Give pipe readers and the log tail one bounded drain window.
                    let exited_at = exited_at.get_or_insert_with(Instant::now);
                    if exited_at.elapsed() < std::time::Duration::from_millis(100) {
                        continue;
                    }
                    runtime.abort_runtime_output_tasks();
                    return;
                }
            }
        }));
    }

    pub async fn assets(&self) -> MutexGuard<'_, DmiCache> {
        self.assets.lock().await
    }

    pub(crate) fn asset_cache(&self) -> Arc<Mutex<DmiCache>> {
        self.assets.clone()
    }

    pub fn asset_limits(&self) -> &crate::limits::ServerLimits {
        &self.asset_limits
    }

    pub(crate) async fn run_asset_job<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self.asset_jobs.clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await?
    }

    pub async fn debugger(&self) -> MutexGuard<'_, Option<DebuggerSession>> {
        self.debugger.lock().await
    }

    pub(crate) async fn lifecycle(&self) -> MutexGuard<'_, ()> {
        self.lifecycle.lock().await
    }

    /// Acquire exclusive rights to build an analysis snapshot.
    ///
    /// The guard is owned so a parse that outlives its request — a worker that
    /// blew its timeout — can keep holding it until the worker really exits.
    pub(crate) async fn parse_permit(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.parse).lock_owned().await
    }

    pub(crate) async fn tracy_capture(&self) -> MutexGuard<'_, TracyCaptureState> {
        self.tracy_capture.lock().await
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct ParseWorkerTestControl {
    paused: StdMutex<bool>,
    released: std::sync::Condvar,
    pub(crate) started: std::sync::atomic::AtomicUsize,
    pub(crate) active: std::sync::atomic::AtomicUsize,
    pub(crate) maximum: std::sync::atomic::AtomicUsize,
    pub(crate) changed: tokio::sync::Notify,
}

#[cfg(test)]
pub(crate) struct AnalysisWriteTestGuard<'a> {
    _guard: tokio::sync::RwLockWriteGuard<'a, AnalysisState>,
}

#[cfg(test)]
impl ParseWorkerTestControl {
    pub(crate) fn pause(&self) {
        *self.paused.lock().unwrap() = true;
    }

    pub(crate) fn release(&self) {
        *self.paused.lock().unwrap() = false;
        self.released.notify_all();
    }

    pub(crate) fn enter(self: &Arc<Self>) -> ParseWorkerTestGuard {
        use std::sync::atomic::Ordering::SeqCst;
        let active = self.active.fetch_add(1, SeqCst) + 1;
        self.maximum.fetch_max(active, SeqCst);
        self.started.fetch_add(1, SeqCst);
        self.changed.notify_one();
        let guard = ParseWorkerTestGuard(self.clone());
        let mut paused = self.paused.lock().unwrap();
        while *paused {
            paused = self.released.wait(paused).unwrap();
        }
        guard
    }
}

#[cfg(test)]
pub(crate) struct ParseWorkerTestGuard(Arc<ParseWorkerTestControl>);

#[cfg(test)]
impl Drop for ParseWorkerTestGuard {
    fn drop(&mut self) {
        self.0
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.0.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn asset_admission_stays_with_cancelled_workers() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        struct Gate {
            open: StdMutex<bool>,
            changed: std::sync::Condvar,
        }
        struct Release(Arc<Gate>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0.open.lock().unwrap() = true;
                self.0.changed.notify_all();
            }
        }
        let limits = crate::limits::ServerLimits {
            max_blocking_jobs: 4,
            ..Default::default()
        };
        let state = Arc::new(ServerState::with_limits(limits));
        let gate = Arc::new(Gate {
            open: StdMutex::new(false),
            changed: std::sync::Condvar::new(),
        });
        let release = Release(gate.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let changed = Arc::new(tokio::sync::Notify::new());
        let launch = || {
            let (state, gate, entered, changed) = (
                state.clone(),
                gate.clone(),
                entered.clone(),
                changed.clone(),
            );
            tokio::spawn(async move {
                state
                    .run_asset_job(move || {
                        entered.fetch_add(1, SeqCst);
                        changed.notify_one();
                        let mut open = gate.open.lock().unwrap();
                        while !*open {
                            open = gate.changed.wait(open).unwrap();
                        }
                        Ok(())
                    })
                    .await
            })
        };
        let first: Vec<_> = (0..4).map(|_| launch()).collect();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while entered.load(SeqCst) < 4 {
                changed.notified().await;
            }
        })
        .await
        .unwrap();
        for task in first {
            task.abort();
            let _ = task.await;
        }
        let fifth = launch();
        let admitted_early = tokio::time::timeout(std::time::Duration::from_millis(100), async {
            while entered.load(SeqCst) < 5 {
                changed.notified().await;
            }
        })
        .await
        .is_ok();
        drop(release);
        fifth.await.unwrap().unwrap();
        assert!(
            !admitted_early,
            "cancelled callers released permits while workers were still alive"
        );
        assert_eq!(entered.load(SeqCst), 5);
    }

    #[test]
    fn output_log_evicts_oldest_lines_at_capacity() {
        let log = OutputLog::default();
        for line_number in 0..=OUTPUT_LOG_CAPACITY {
            push_output_line(&log, format!("line {line_number}"));
        }
        let lines: Vec<String> = log
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect();
        assert_eq!(lines.len(), OUTPUT_LOG_CAPACITY);
        assert_eq!(lines.first().map(String::as_str), Some("line 1"));
        assert_eq!(lines.last().map(String::as_str), Some("line 500"));
    }

    #[test]
    fn output_log_truncates_single_lines_to_a_fixed_byte_limit() {
        let log = OutputLog::default();
        push_output_line(&log, "x".repeat(16_384 + 1_000));
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
    fn output_log_evicts_oldest_lines_to_keep_total_bytes_bounded() {
        let log = OutputLog::default();
        for line_number in 0..500 {
            push_output_line(&log, format!("{line_number:03}-{}", "x".repeat(4_096)));
        }
        let lines = log.lock().unwrap();
        let total_bytes: usize = lines.entries.iter().map(|line| line.text.len()).sum();
        assert!(total_bytes <= 1_048_576);
        assert_eq!(
            lines.entries.back().map(|line| &line.text[..3]),
            Some("499")
        );
    }

    #[test]
    fn runtime_output_queries_are_non_destructive() {
        let state = RuntimeState::new();
        for line_number in 0..3 {
            push_output_line(&state.output_log, format!("line {line_number}"));
        }
        assert_eq!(state.recent_output(0), Vec::<String>::new());
        assert_eq!(
            state.recent_output(2),
            vec!["line 1".to_string(), "line 2".to_string()]
        );
        assert!(state.matches_output(r"line \d", true).unwrap());
        assert!(state.matches_output("[invalid", true).is_err());
    }

    #[test]
    fn nearest_output_uses_monotonic_offsets() {
        let log = OutputLog::default();
        push_output_line_at(&log, 100, "phase-start".to_owned());
        push_output_line_at(&log, 250, "phase-complete".to_owned());
        let entry = nearest_output_before(&log, 200).unwrap();
        assert_eq!(entry.sequence, 1);
        assert_eq!(entry.text, "phase-start");
    }

    #[tokio::test]
    async fn empty_state_requires_a_parse() {
        let state = ServerState::new();
        assert!(matches!(
            state.snapshot().await,
            Err(StateError::ParseRequired)
        ));
        assert_eq!(state.state_generation().await, 0);
    }

    #[test]
    fn tracy_phase_is_reusable_only_when_the_measurement_window_never_started() {
        let mut capture = TracyCaptureState::default();
        assert!(capture.begin_capture("steady", 1).is_ok());
        capture.finish_capture("steady", 1, false);
        assert!(capture.begin_capture("steady", 1).is_ok());
        capture.finish_capture("steady", 1, true);
        assert_eq!(
            capture.begin_capture("steady", 1),
            Err(TracyCaptureStartError::PhaseAlreadyUsed)
        );
    }

    #[test]
    fn invalid_capture_records_remain_separate_from_authoritative_captures() {
        let mut capture = TracyCaptureState::default();
        let trace = std::path::PathBuf::from("diagnostics/invalid.tracy");
        let sidecar = std::path::PathBuf::from("diagnostics/invalid.tracy.meridian.json");
        capture.record_diagnostic(
            serde_json::json!({"authoritative": false}),
            [trace.clone(), sidecar.clone()],
        );
        assert!(capture.capture_records.is_empty());
        assert_eq!(capture.diagnostic_records.len(), 1);
        assert!(capture.integrity_owned_paths.contains(&trace));
        assert!(capture.integrity_owned_paths.contains(&sidecar));
    }
}
