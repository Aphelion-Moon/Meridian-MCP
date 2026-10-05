mod analysis;
mod build_response;
mod compile;
mod debugger;
mod diagnostics;
mod dmi;
mod docs;
mod fixture;
mod language;
pub(crate) mod map;
mod memory;
mod native_evidence;
mod parse;
pub mod rift;
pub(crate) mod runtime;
mod search;
mod server_status;
mod tracy;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::mcp::{ToolDefinition, ToolResult};
use crate::result::{structured_error, ToolErrorCode};
use crate::spaceman::debugger::DebuggerInstallation;
use crate::state::ServerState;
use crate::tracy::TracyInstallation;
use crate::{contracts_for_configuration, CapabilityMode, PathPolicy, RiftBuildAccess};

#[derive(Clone)]
pub struct ToolExecutionContext {
    catalog: std::sync::Arc<[&'static crate::ToolContract]>,
    mode: CapabilityMode,
    policy: PathPolicy,
    rift_build: RiftBuildAccess,
    dmdoc_helper: Option<std::path::PathBuf>,
    debugger: Option<DebuggerInstallation>,
    tracy: Option<TracyInstallation>,
    private_state: Option<std::sync::Arc<crate::PrivateStateStore>>,
    build_provenance: Option<std::sync::Arc<crate::BuildProvenanceStore>>,
    integrity_recovery: std::sync::Arc<[crate::runtime_integrity::RuntimeIntegritySummary]>,
    owned_requests:
        std::sync::Arc<std::sync::Mutex<Vec<std::sync::Weak<tokio::sync::watch::Sender<bool>>>>>,
    pub(crate) cancellation: Option<tokio::sync::watch::Receiver<bool>>,
    pub(crate) request_started: Option<tokio::time::Instant>,
    request_owner: Option<std::sync::Arc<()>>,
}

impl ToolExecutionContext {
    pub fn new(mode: CapabilityMode, policy: PathPolicy) -> Self {
        Self::with_rift_build(mode, policy, RiftBuildAccess::Disabled)
    }

    pub fn with_rift_build(
        mode: CapabilityMode,
        policy: PathPolicy,
        rift_build: RiftBuildAccess,
    ) -> Self {
        Self {
            catalog: std::sync::Arc::from(active_contracts(mode, rift_build, false, false, false)),
            mode,
            policy,
            rift_build,
            dmdoc_helper: None,
            debugger: None,
            tracy: None,
            private_state: None,
            build_provenance: None,
            integrity_recovery: std::sync::Arc::from([]),
            owned_requests: Default::default(),
            cancellation: None,
            request_started: None,
            request_owner: None,
        }
    }

    pub fn with_features(
        mode: CapabilityMode,
        policy: PathPolicy,
        rift_build: RiftBuildAccess,
        dmdoc_helper: Option<std::path::PathBuf>,
        debugger: Option<DebuggerInstallation>,
        tracy: Option<TracyInstallation>,
    ) -> Self {
        Self::with_features_and_state(
            mode,
            policy,
            rift_build,
            dmdoc_helper,
            debugger,
            tracy,
            None,
        )
    }

    pub fn with_features_and_state(
        mode: CapabilityMode,
        policy: PathPolicy,
        rift_build: RiftBuildAccess,
        dmdoc_helper: Option<std::path::PathBuf>,
        debugger: Option<DebuggerInstallation>,
        tracy: Option<TracyInstallation>,
        private_state: Option<std::sync::Arc<crate::PrivateStateStore>>,
    ) -> Self {
        let build_provenance = private_state.as_ref().map(|state| {
            std::sync::Arc::new(crate::BuildProvenanceStore::new(
                std::sync::Arc::clone(state),
                policy.clone(),
            ))
        });
        let integrity_recovery = private_state
            .as_ref()
            .and_then(|state| {
                crate::runtime_integrity::recover_unfinished(state, policy.effective_roots()).ok()
            })
            .unwrap_or_default();
        Self {
            catalog: std::sync::Arc::from(active_contracts(
                mode,
                rift_build,
                dmdoc_helper.is_some(),
                debugger.is_some(),
                tracy.is_some(),
            )),
            mode,
            policy,
            rift_build,
            dmdoc_helper,
            debugger,
            tracy,
            private_state,
            build_provenance,
            integrity_recovery: std::sync::Arc::from(integrity_recovery),
            owned_requests: Default::default(),
            cancellation: None,
            request_started: None,
            request_owner: None,
        }
    }

    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        self.catalog
            .iter()
            .map(|contract| definition(contract))
            .collect()
    }

    pub fn mode(&self) -> CapabilityMode {
        self.mode
    }

    pub(crate) fn cancel_owned_requests(&self) {
        let mut requests = self
            .owned_requests
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        requests.retain(|request| {
            if let Some(request) = request.upgrade() {
                let _ = request.send(true);
                true
            } else {
                false
            }
        });
    }

    pub(crate) fn cancellation(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.cancellation.clone()
    }
    pub(crate) fn cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|receiver| *receiver.borrow())
    }
    pub(crate) fn finalization_reason(
        &self,
        deadline: tokio::time::Instant,
    ) -> Option<&'static str> {
        if self.cancelled() {
            Some("request_cancelled")
        } else if tokio::time::Instant::now() >= deadline {
            Some("request_timed_out")
        } else {
            None
        }
    }

    pub(crate) fn deadline(&self, timeout_ms: u64) -> tokio::time::Instant {
        self.request_started
            .unwrap_or_else(tokio::time::Instant::now)
            + std::time::Duration::from_millis(timeout_ms)
    }

    pub(crate) async fn execution_lease(
        &self,
        artifact: &std::path::Path,
        working_directory: &std::path::Path,
        kind: &'static str,
        deadline: tokio::time::Instant,
    ) -> Result<Result<crate::execution_lease::ExecutionLease, ToolResult>> {
        let Some(store) = self.private_state_arc() else {
            return Ok(Err(ToolResult::structured_error(
                "state_not_configured",
                "writer execution requires a shared private state directory",
                "Configure MERIDIAN_MCP_STATE_DIR before running writer tools.",
            )));
        };
        if self.cancelled() || tokio::time::Instant::now() >= deadline {
            return Ok(Err(ToolResult::structured_error(
                "timed_out",
                "execution admission expired or was cancelled",
                "Retry with a new request after cleanup.",
            )));
        }
        let policy = self.policy.clone();
        let artifact = artifact.to_owned();
        let working = working_directory.to_owned();
        let request_owner = self.request_owner.clone();
        let task = tokio::task::spawn_blocking(move || {
            if tokio::time::Instant::now() >= deadline {
                return Err(crate::execution_lease::AdmissionError::Other(anyhow!(
                    "execution admission expired"
                )));
            }
            let mut lease = crate::execution_lease::ExecutionLease::acquire(
                store, &policy, &artifact, &working, kind,
            )?;
            lease.request_owner = request_owner;
            if tokio::time::Instant::now() >= deadline {
                return Err(crate::execution_lease::AdmissionError::Other(anyhow!(
                    "execution admission expired"
                )));
            }
            Ok(lease)
        });
        match tokio::time::timeout_at(deadline, task).await {
            Ok(result) => Ok(result?.map_err(|error| error.result())),
            Err(_) => Ok(Err(ToolResult::structured_error(
                "timed_out",
                "execution admission exceeded the total deadline",
                "Retry with a new request after cleanup.",
            ))),
        }
    }

    pub fn rift_build_access(&self) -> RiftBuildAccess {
        self.rift_build
    }

    pub(crate) fn policy(&self) -> &PathPolicy {
        &self.policy
    }
    pub(crate) fn dmdoc_helper(&self) -> Option<&std::path::Path> {
        self.dmdoc_helper.as_deref()
    }
    pub(crate) fn debugger(&self) -> Option<&DebuggerInstallation> {
        self.debugger.as_ref()
    }
    pub(crate) fn tracy(&self) -> Option<&TracyInstallation> {
        self.tracy.as_ref()
    }
    pub(crate) fn private_state(&self) -> Option<&crate::PrivateStateStore> {
        self.private_state.as_deref()
    }
    pub(crate) fn private_state_arc(&self) -> Option<std::sync::Arc<crate::PrivateStateStore>> {
        self.private_state.as_ref().map(std::sync::Arc::clone)
    }
    pub(crate) fn build_provenance(&self) -> Option<&crate::BuildProvenanceStore> {
        self.build_provenance.as_deref()
    }
    pub(crate) fn build_provenance_arc(
        &self,
    ) -> Option<std::sync::Arc<crate::BuildProvenanceStore>> {
        self.build_provenance.as_ref().map(std::sync::Arc::clone)
    }
    pub(crate) fn integrity_recovery(
        &self,
    ) -> &[crate::runtime_integrity::RuntimeIntegritySummary] {
        &self.integrity_recovery
    }
}

fn definition(contract: &crate::ToolContract) -> ToolDefinition {
    ToolDefinition {
        name: contract.name.into(),
        description: contract.summary.into(),
        input_schema: (contract.schema)(),
        output_schema: (contract.output_schema)(),
    }
}
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    static DEFINITIONS: std::sync::OnceLock<Vec<ToolDefinition>> = std::sync::OnceLock::new();
    DEFINITIONS
        .get_or_init(|| crate::all_contracts().iter().map(definition).collect())
        .clone()
}
fn active_contracts(
    mode: CapabilityMode,
    rift: RiftBuildAccess,
    docs: bool,
    debugger: bool,
    tracy: bool,
) -> Vec<&'static crate::ToolContract> {
    contracts_for_configuration(mode, rift)
        .into_iter()
        .filter(|contract| match contract.gate {
            crate::contracts::StartupGate::Docs => docs,
            crate::contracts::StartupGate::Debugger => debugger,
            crate::contracts::StartupGate::Tracy => tracy,
            crate::contracts::StartupGate::Base | crate::contracts::StartupGate::Rift => true,
        })
        .collect()
}
pub fn get_tool_definitions_for(
    mode: CapabilityMode,
    rift: RiftBuildAccess,
) -> Vec<ToolDefinition> {
    get_tool_definitions_for_runtime(mode, rift, false, false, false)
}
pub fn get_tool_definitions_for_active(
    mode: CapabilityMode,
    rift: RiftBuildAccess,
    docs: bool,
) -> Vec<ToolDefinition> {
    get_tool_definitions_for_runtime(mode, rift, docs, false, false)
}
pub fn get_tool_definitions_for_runtime(
    mode: CapabilityMode,
    rift: RiftBuildAccess,
    docs: bool,
    debugger: bool,
    tracy: bool,
) -> Vec<ToolDefinition> {
    active_contracts(mode, rift, docs, debugger, tracy)
        .into_iter()
        .map(definition)
        .collect()
}
/// Call a tool by name with the given arguments
pub async fn call_tool(
    context: &ToolExecutionContext,
    state: &ServerState,
    name: &str,
    args: Value,
) -> Result<ToolResult> {
    if name == "rift_compile" && !cfg!(windows) {
        return Ok(policy_error(
            "unsupported_platform",
            "rift_compile is supported only on Windows".to_string(),
            None,
            "Run rift_compile from an approved Windows Meridian-MCP installation.",
            json!({ "tool": name, "platform": std::env::consts::OS }),
        ));
    }
    let Some(contract) = context
        .catalog
        .iter()
        .find(|contract| contract.name == name)
    else {
        return Ok(policy_error(
            "tool_not_available",
            "tool is not available in this startup configuration".into(),
            None,
            "Use a tool advertised by tools/list.",
            json!({"tool":name,"mode":match context.mode { CapabilityMode::Analysis => "analysis", CapabilityMode::Development => "development" }}),
        ));
    };
    let mut args = match (contract.decode)(args) {
        Ok(args) => args,
        Err(error) => {
            return Ok(if matches!(name, "rift_compile" | "dm_generate_docs") {
                error.result_with_code("invalid_arguments")
            } else {
                error.result()
            })
        }
    };
    if let Err(error) = contain_arguments(&context.policy, &mut args) {
        let mut details = json!({
            "path": error.path().display().to_string(),
            "policy_code": error.code(),
            "containment_mode": error.context().containment_mode,
            "policy_source": error.context().policy_source,
            "effective_roots": error.context().effective_roots,
        });
        let retained_analysis =
            if let crate::parameters::ToolRequest::ParseEnvironment(request) = &args {
                let metadata = state.analysis_metadata();
                details["state_preserved"] = json!(true);
                details["requested_environment"] = json!(request.dme_path);
                details["state_generation"] = json!(metadata.generation);
                details["active_environment"] = json!(metadata.active_environment);
                details["analysis"] = json!(metadata.identity);
                metadata.identity
            } else {
                None
            };
        let result = policy_error(
            error.code(),
            error.to_string(),
            Some(error.path()),
            "Use a contained path and only startup-allowlisted executables.",
            details,
        );
        return Ok(match retained_analysis {
            Some(analysis) => result.with_analysis(analysis),
            None => result,
        });
    }
    if let Err(error) = args.validate_canonical_paths() {
        return Ok(error.result());
    }
    // Freeze admission only after strict decoding and path authorization. All
    // downstream snapshot reads on this request view clone the same Arc.
    let snapshot = if let Some(expected) = args.snapshot_expectation() {
        let snapshot = match state.snapshot().await {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return Ok(ToolResult::structured_error(
                    "parse_required",
                    "No environment loaded.",
                    "Call dm_parse_environment first.",
                ))
            }
        };
        if let Some(expected) = expected {
            if expected != snapshot.snapshot_id {
                return Ok(crate::identity::StaleIdentity {
                    field: "expected_snapshot",
                    expected: expected.to_owned(),
                    current: Some(snapshot.snapshot_id.clone()),
                }
                .result()
                .with_analysis(snapshot.identity()));
            }
        }
        Some(snapshot)
    } else {
        None
    };
    let runtime = if let Some((debugger, expected)) = args.runtime_expectation() {
        let current = if debugger {
            state
                .debugger()
                .await
                .as_ref()
                .map(|session| session.runtime_id.clone())
        } else {
            state.runtime().await.runtime_id.clone()
        };
        if let Some(expected) = expected {
            if Some(expected) != current.as_deref() {
                let result = crate::identity::StaleIdentity {
                    field: "expected_runtime",
                    expected: expected.to_owned(),
                    current,
                }
                .result();
                return Ok(match snapshot.as_ref() {
                    Some(snapshot) => result.with_analysis(snapshot.identity()),
                    None => result,
                });
            }
        }
        Some(current)
    } else {
        None
    };
    let mut admitted = state.for_request(snapshot, runtime);
    if args.uses_optional_analysis() && admitted.admitted_analysis().is_none() {
        admitted.freeze_optional_analysis(state.active_snapshot().await);
    }
    let state = &admitted;
    if matches!(
        name,
        "dm_compile" | "rift_compile" | "dm_run" | "dm_debug_launch" | "dm_tracy_launch"
    ) {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let sender = std::sync::Arc::new(sender);
        {
            let mut requests = context
                .owned_requests
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            requests.retain(|request| request.strong_count() != 0);
            requests.push(std::sync::Arc::downgrade(&sender));
        }
        let (acknowledge, acknowledged) = tokio::sync::oneshot::channel();
        let mut owner = RequestOwnership {
            cancellation: Some(sender),
            acknowledge: Some(acknowledge),
        };
        let mut execution = context.clone();
        execution.cancellation = Some(receiver.clone());
        execution
            .request_started
            .get_or_insert_with(tokio::time::Instant::now);
        execution.request_owner = Some(std::sync::Arc::new(()));
        let state = state.clone();
        let name = name.to_owned();
        let (response, result) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let launch = !matches!(name.as_str(), "dm_compile" | "rift_compile");
            let mut cancellation = receiver;
            let outcome = if launch {
                tokio::select! {
                    biased;
                    _ = cancellation.wait_for(|cancelled| *cancelled) => Ok(ToolResult::structured_error("cancelled", "launch was cancelled", "Retry as a new operation after cleanup.")),
                    result = dispatch_tool(&execution, &state, &name, args) => result,
                }
            } else {
                dispatch_tool(&execution, &state, &name, args).await
            };
            let cancelled = *cancellation.borrow();
            if cancelled && launch {
                if let Err(error) = cleanup_owned_launch(&execution, &state, &name).await {
                    let _ = response.send(Err(error));
                    return;
                }
            }
            let delivered = response.send(outcome).is_ok();
            // Retain the owner until the caller acknowledges delivery. A caller
            // dropped after launch completed still requires owned cleanup.
            if launch && !cancelled && (!delivered || acknowledged.await.is_err()) {
                let _ = cleanup_owned_launch(&execution, &state, &name).await;
            }
        });
        let result = result.await?;
        owner.cancellation = None;
        if let Some(acknowledge) = owner.acknowledge.take() {
            let _ = acknowledge.send(());
        }
        return result;
    }
    let result = match dispatch_tool(context, state, name, args).await {
        Err(error) if error.is::<crate::identity::StaleIdentity>() => Ok(error
            .downcast_ref::<crate::identity::StaleIdentity>()
            .expect("checked error type")
            .result()),
        result => result,
    };
    result
        .map_err(|error| match state.admitted_analysis() {
            Some(snapshot) => crate::result::SemanticCallError {
                error,
                analysis: snapshot.identity(),
            }
            .into(),
            None => error,
        })
        .map(|result| match state.admitted_analysis() {
            Some(snapshot) => result.with_analysis(snapshot.identity()),
            None => result,
        })
}

struct RequestOwnership {
    cancellation: Option<std::sync::Arc<tokio::sync::watch::Sender<bool>>>,
    acknowledge: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for RequestOwnership {
    fn drop(&mut self) {
        if let Some(sender) = self.cancellation.take() {
            let _ = sender.send(true);
        }
    }
}

async fn cleanup_owned_launch(
    context: &ToolExecutionContext,
    state: &ServerState,
    name: &str,
) -> Result<()> {
    let _lifecycle = state.lifecycle().await;
    let owner = context
        .request_owner
        .as_ref()
        .expect("owned request identity");
    if name == "dm_debug_launch" {
        let owns = state
            .debugger()
            .await
            .as_ref()
            .and_then(|session| session.execution_lease.as_ref())
            .is_some_and(|lease| lease.belongs_to(owner));
        if owns {
            debugger::stop_with_lifecycle(state).await?;
        }
    } else {
        let owns = state
            .runtime()
            .await
            .execution_lease
            .as_ref()
            .is_some_and(|lease| lease.belongs_to(owner));
        if owns {
            if name == "dm_tracy_launch" {
                tracy::stop_with_lifecycle(context, state).await?;
            } else {
                runtime::stop_with_lifecycle(state).await?;
            }
        }
    }
    Ok(())
}

async fn dispatch_tool(
    context: &ToolExecutionContext,
    state: &ServerState,
    _name: &str,
    args: crate::parameters::ToolRequest,
) -> Result<ToolResult> {
    use crate::parameters::ToolRequest;
    match args {
        // Parsing tools
        ToolRequest::ServerStatus(_args) => server_status::status(context, state).await,
        ToolRequest::ParseEnvironment(args) => {
            parse::parse_environment_with_policy(state, args, context.policy()).await
        }
        ToolRequest::CheckFixtureSync(args) => fixture::check_sync(context, state, args).await,
        ToolRequest::MemorySummary(args) => {
            memory::run(context, state, memory::MemoryInput::Summary(args)).await
        }
        ToolRequest::MemoryCompare(args) => {
            memory::run(context, state, memory::MemoryInput::Compare(args)).await
        }
        ToolRequest::NativeEvidenceSummary(args) => {
            native_evidence::summary(context, state, args).await
        }
        ToolRequest::NativeEvidenceCompare(args) => {
            native_evidence::compare(context, state, args).await
        }
        ToolRequest::GetType(args) => parse::get_type(state, args).await,
        ToolRequest::GetProc(args) => parse::get_proc(state, args).await,
        ToolRequest::GetVar(args) => parse::get_var(state, args).await,
        ToolRequest::ListTypes(args) => parse::list_types(state, args).await,
        ToolRequest::SearchSymbols(args) => parse::search_symbols(state, args).await,
        ToolRequest::SearchContext(args) => search::search_context(state, args).await,

        // Analysis tools
        ToolRequest::CheckErrors(args) => diagnostics::check_errors(state, args).await,
        ToolRequest::GetDefinition(args) => analysis::get_definition(state, args).await,
        ToolRequest::GenerateDocs(args) => docs::generate(context, state, args).await,
        ToolRequest::DocumentSymbols(args) => language::document_symbols(state, args).await,
        ToolRequest::FindReferences(args) => language::find_references(state, args).await,
        ToolRequest::FindImplementations(args) => language::find_implementations(state, args).await,
        ToolRequest::DmiInfo(args) => dmi::info(context, state, args).await,
        ToolRequest::CompareDmiStates(args) => dmi::compare(context, state, args).await,
        ToolRequest::FindDmiDuplicates(args) => dmi::find_duplicates(context, state, args).await,
        ToolRequest::AuditIcons(args) => dmi::audit_icons(context, state, args).await,
        ToolRequest::ExtractDmi(args) => dmi::extract(context, state, args).await,

        // Compile tool
        ToolRequest::Compile(args) => compile::compile(context, state, args).await,
        ToolRequest::RiftCompile(args) => rift::compile(context, state, args).await,

        // Map tools
        ToolRequest::RenderMap(args) => map::render_map(context, state, args).await,
        ToolRequest::MapInfo(args) => map::map_info(args).await,
        ToolRequest::FindOnMap(args) => map::find_on_map(args).await,
        ToolRequest::DiffMaps(args) => map::diff_maps(args).await,
        ToolRequest::ListRenderPasses(_args) => map::list_render_passes().await,
        ToolRequest::RenderMaps(args) => map::render_maps(context, state, args).await,

        ToolRequest::DebugLaunch(args) => debugger::launch(context, state, args).await,
        ToolRequest::DebugStop(_args) => debugger::stop(state).await,
        ToolRequest::DebugSetBreakpoints(args) => debugger::set_breakpoints(state, args).await,
        ToolRequest::DebugSetFunctionBreakpoints(args) => {
            debugger::set_function_breakpoints(state, args).await
        }
        ToolRequest::DebugSetExceptionBreakpoints(args) => {
            debugger::set_exception_breakpoints(state, args).await
        }
        ToolRequest::DebugControl(args) => debugger::control(state, args).await,
        ToolRequest::DebugThreads(_args) => debugger::threads(state).await,
        ToolRequest::DebugStackTrace(args) => debugger::stack_trace(state, args).await,
        ToolRequest::DebugScopes(args) => debugger::scopes(state, args).await,
        ToolRequest::DebugVariables(args) => debugger::variables(state, args).await,
        ToolRequest::DebugEvaluate(args) => debugger::evaluate(state, args).await,
        ToolRequest::DebugMemory(args) => debugger::memory(state, args.into_control()).await,
        ToolRequest::DebugExceptionInfo(_args) => debugger::exception_info(state).await,
        ToolRequest::DebugSource(args) => debugger::source(state, args).await,
        ToolRequest::DebugWaitForEvent(args) => debugger::wait_for_event(state, args).await,

        // Runtime tools
        ToolRequest::Run(args) => runtime::run(context, state, args).await,
        ToolRequest::WaitForOutput(args) => runtime::wait_for_output(state, args).await,
        ToolRequest::Stop(args) => runtime::stop(state, args).await,
        ToolRequest::Status(args) => runtime::status(state, args).await,
        ToolRequest::Topic(args) => runtime::topic(state, args).await,
        ToolRequest::TracyPrepare(args) => tracy::prepare(context, args).await,
        ToolRequest::TracyLaunch(args) => tracy::launch(context, state, args).await,
        ToolRequest::TracyCapture(args) => tracy::capture(context, state, args).await,
        ToolRequest::TracyStatus(_args) => tracy::status(state).await,
        ToolRequest::TracyStop(_args) => tracy::stop(context, state).await,
        ToolRequest::TracyHotspots(args) => tracy::hotspots(context, state, args).await,
        ToolRequest::TracyZone(args) => tracy::zone(context, state, args).await,
        ToolRequest::TracyFrameStats(args) => tracy::frame_stats(context, state, args).await,
        ToolRequest::TracyCompare(args) => tracy::compare(context, state, args).await,
        ToolRequest::TracyControlStats(args) => tracy::control_stats(context, state, args).await,
    }
}

fn contain_arguments(
    policy: &PathPolicy,
    args: &mut crate::parameters::ToolRequest,
) -> std::result::Result<(), crate::PolicyError> {
    use crate::parameters::ToolRequest::*;
    fn read(policy: &PathPolicy, path: &mut String) -> std::result::Result<(), crate::PolicyError> {
        *path = policy.wire_path(policy.read_path(&*path)?)?;
        Ok(())
    }
    fn runtime(
        policy: &PathPolicy,
        path: &mut String,
    ) -> std::result::Result<(), crate::PolicyError> {
        *path = policy.wire_path(policy.runtime_dmb(&*path)?)?;
        Ok(())
    }
    fn optional(
        policy: &PathPolicy,
        path: &mut Option<String>,
    ) -> std::result::Result<(), crate::PolicyError> {
        if let Some(path) = path {
            read(policy, path)?;
        }
        Ok(())
    }
    fn working(
        policy: &PathPolicy,
        path: &mut String,
        directory: &mut Option<String>,
        is_runtime: bool,
    ) -> std::result::Result<(), crate::PolicyError> {
        if let Some(directory) = directory {
            let canonical = policy.read_directory(&*directory)?;
            if std::path::Path::new(path).is_relative() {
                *path = policy.wire_path(canonical.join(&*path))?;
            }
            *directory = policy.wire_path(canonical)?;
        }
        if is_runtime {
            runtime(policy, path)
        } else {
            read(policy, path)
        }
    }
    fn evidence(
        policy: &PathPolicy,
        request: &mut crate::native_evidence::model::NativeEvidenceRequest,
    ) -> std::result::Result<(), crate::PolicyError> {
        for artifact in &mut request.artifacts {
            artifact.path = policy.read_path(&artifact.path)?;
        }
        if let Some(path) = &mut request.dmb_path {
            *path = policy.read_path(&*path)?;
        }
        Ok(())
    }
    match args {
        ParseEnvironment(a) => read(policy, &mut a.dme_path)?,
        CheckFixtureSync(a) => read(policy, &mut a.fixture_manifest_path)?,
        MemorySummary(a) => a.evidence_path = policy.read_path(&a.evidence_path)?,
        MemoryCompare(a) => {
            a.baseline.evidence_path = policy.read_path(&a.baseline.evidence_path)?;
            a.current.evidence_path = policy.read_path(&a.current.evidence_path)?;
        }
        NativeEvidenceSummary(a) => evidence(policy, a)?,
        NativeEvidenceCompare(a) => {
            for run in &mut a.runs {
                evidence(policy, run)?;
            }
        }
        DocumentSymbols(a) => read(policy, &mut a.file_path)?,
        DmiInfo(a) => read(policy, &mut a.dmi_path)?,
        CompareDmiStates(a) => {
            read(policy, &mut a.left_dmi_path)?;
            read(policy, &mut a.right_dmi_path)?;
        }
        FindDmiDuplicates(a) => optional(policy, &mut a.scope_path)?,
        AuditIcons(a) => optional(policy, &mut a.scope_path)?,
        ExtractDmi(a) => {
            read(policy, &mut a.dmi_path)?;
            a.output_path = policy
                .wire_path(policy.output_path(&a.output_path, a.overwrite.unwrap_or(false))?)?;
        }
        GenerateDocs(a) => {
            a.output_directory = policy.wire_path(
                policy.directory_output_path(&a.output_directory, a.overwrite.unwrap_or(false))?,
            )?;
        }
        RenderMap(a) => {
            read(policy, &mut a.dmm_path)?;
            let output = a.output_path.clone().unwrap_or_else(|| {
                std::path::Path::new(&a.dmm_path)
                    .with_extension("png")
                    .display()
                    .to_string()
            });
            a.output_path =
                Some(policy.wire_path(policy.output_path(output, a.overwrite.unwrap_or(false))?)?);
        }
        MapInfo(a) => read(policy, &mut a.dmm_path)?,
        FindOnMap(a) => read(policy, &mut a.dmm_path)?,
        DiffMaps(a) => {
            read(policy, &mut a.left_dmm_path)?;
            read(policy, &mut a.right_dmm_path)?;
        }
        Compile(a) => {
            working(policy, &mut a.dme_path, &mut a.working_directory, false)?;
            optional(policy, &mut a.fixture_manifest_path)?;
            if let Some(path) = &mut a.compiler_path {
                *path = policy.wire_path(policy.executable(&*path)?)?;
            }
        }
        RiftCompile(a) => optional(policy, &mut a.fixture_manifest_path)?,
        Run(a) => working(policy, &mut a.dmb_path, &mut a.working_directory, true)?,
        DebugLaunch(a) => runtime(policy, &mut a.dmb_path)?,
        DebugSetBreakpoints(a) => read(policy, &mut a.source_path)?,
        TracyPrepare(a) => runtime(policy, &mut a.dmb_path)?,
        TracyLaunch(a) => runtime(policy, &mut a.dmb_path)?,
        TracyHotspots(a) => read(policy, &mut a.trace_path)?,
        TracyZone(a) => read(policy, &mut a.trace_path)?,
        TracyFrameStats(a) => read(policy, &mut a.trace_path)?,
        TracyCompare(a) => {
            read(policy, &mut a.baseline_path)?;
            read(policy, &mut a.current_path)?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn require_launchable_artifact(
    context: &ToolExecutionContext,
    dmb_path: &std::path::Path,
    require_verified: bool,
) -> std::result::Result<crate::LaunchProvenance, ToolResult> {
    let dmb = match crate::FileIdentity::capture(dmb_path) {
        Ok(dmb) => dmb,
        Err(error) => {
            return Err(structured_error(
                ToolErrorCode::InvalidInput,
                "could not identify the DMB immediately before launch",
                Some("Use an existing contained regular DMB file.".to_owned()),
                json!({"dmb_path":dmb_path,"error":error.to_string()}),
            ));
        }
    };
    let decision = match context.build_provenance() {
        Some(store) => match store.evaluate_launch(dmb_path, require_verified) {
            Ok(decision) => decision,
            Err(error) => {
                return Err(structured_error(
                    ToolErrorCode::WorkspaceIntegrityViolation,
                    "managed build provenance could not be validated",
                    Some(
                        "Inspect the private state store and compile the artifact again."
                            .to_owned(),
                    ),
                    json!({"dmb_path":dmb_path,"error":error.to_string()}),
                ));
            }
        },
        None => crate::LaunchDecision {
            status: crate::ProvenanceStatus::Unverified,
            allowed: !require_verified,
            record_id: None,
            reasons: vec![crate::ProvenanceReason {
                code: "no_build_record".to_owned(),
                message: "no managed successful build record exists for this artifact".to_owned(),
                role: None,
                path: Some(dmb.path.clone()),
            }],
        },
    };
    let launch = crate::LaunchProvenance {
        status: decision.status,
        build_record_id: decision.record_id,
        dmb_sha256: dmb.sha256,
        warnings: decision.reasons,
    };
    if decision.allowed {
        Ok(launch)
    } else {
        let code = if launch.status == crate::ProvenanceStatus::Stale {
            "stale_build_artifact"
        } else {
            "build_provenance_unavailable"
        };
        Err(structured_error(
            ToolErrorCode::WorkspaceIntegrityViolation,
            code,
            Some("Compile the DMB through Meridian-MCP and retry the launch.".to_owned()),
            json!({"provenance":launch}),
        ))
    }
}

fn policy_error(
    code: &str,
    message: String,
    path: Option<&std::path::Path>,
    recovery: &str,
    details: Value,
) -> ToolResult {
    ToolResult::error(
        json!({
            "code": code,
            "message": message,
            "recovery": recovery,
            "details": details,
            "path": path.map(|path| path.display().to_string())
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::get_tool_definitions;
    use serde_json::json;

    #[tokio::test]
    async fn admitted_snapshot_answers_and_metadata_survive_concurrent_reparse() {
        struct OwnedRoot(std::path::PathBuf);
        impl Drop for OwnedRoot {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!(
            "meridian-admission-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        let root = OwnedRoot(path);
        let first = root.0.join("first.dme");
        let second = root.0.join("second.dme");
        std::fs::write(&first, "/datum/admitted_first\n").unwrap();
        std::fs::write(&second, "/datum/admitted_second\n").unwrap();
        let context = super::ToolExecutionContext::new(
            crate::CapabilityMode::Analysis,
            crate::PathPolicy::new(vec![root.0.clone()], vec![]).unwrap(),
        );
        let state = crate::state::ServerState::new();
        super::call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":first}),
        )
        .await
        .unwrap();
        let captured = state.snapshot().await.unwrap();
        let identity = captured.identity();
        let admitted = state.for_request(Some(captured), None);
        let (reached, ready) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let read_context = context.clone();
        let expected = identity.snapshot_id.clone();
        let reader = tokio::spawn(async move {
            reached.send(()).unwrap();
            released.await.unwrap();
            super::call_tool(
                &read_context,
                &admitted,
                "dm_get_type",
                json!({"type_path":"/datum/admitted_first", "expected_snapshot":expected}),
            )
            .await
            .unwrap()
        });
        ready.await.unwrap();
        super::call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":second}),
        )
        .await
        .unwrap();
        release.send(()).unwrap();
        let result = reader.await.unwrap();
        assert_eq!(result.is_error, None);
        assert_eq!(result.meta.as_ref().unwrap().analysis, identity);
        let crate::result::ToolContent::Text { text } = &result.content[0];
        let body: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(body["analysis"]["snapshot_id"], identity.snapshot_id);
        assert_eq!(body["path"], "/datum/admitted_first");
        assert_ne!(
            state.snapshot().await.unwrap().snapshot_id,
            identity.snapshot_id
        );
    }

    #[test]
    fn tool_definitions_use_supported_name_prefixes() {
        let definitions = get_tool_definitions();

        assert!(!definitions.is_empty());
        assert!(definitions
            .iter()
            .all(|tool| tool.name.starts_with("dm_") || tool.name == "rift_compile"));
    }

    #[test]
    fn debugger_launch_schema_exposes_interactive_and_headless_hosts() {
        let definitions = get_tool_definitions();
        let launch = definitions
            .iter()
            .find(|tool| tool.name == "dm_debug_launch")
            .expect("dm_debug_launch should be defined");

        assert_eq!(
            launch.input_schema["properties"]["host_mode"]["enum"],
            json!(["interactive", "headless"])
        );
        assert_eq!(
            launch.input_schema["properties"]["host_mode"]["default"],
            "interactive"
        );
    }

    #[test]
    fn context_search_schema_requires_a_query_and_exposes_filters() {
        let definitions = get_tool_definitions();
        let search = definitions
            .iter()
            .find(|tool| tool.name == "dm_search_context")
            .expect("context search tool should be registered");

        assert_eq!(
            search.input_schema["required"],
            serde_json::json!(["query"])
        );
        assert_eq!(
            search.input_schema["properties"]["kind"]["enum"],
            serde_json::json!(["all", "type", "proc", "var"])
        );
        assert_eq!(search.input_schema["properties"]["limit"]["maximum"], 50);
        assert_eq!(
            search.input_schema["properties"]["max_source_lines"]["maximum"],
            200
        );
    }

    #[test]
    fn diagnostic_schema_exposes_bounded_snapshot_filters() {
        let definitions = get_tool_definitions();
        let diagnostics = definitions
            .iter()
            .find(|tool| tool.name == "dm_check_errors")
            .expect("dm_check_errors should be registered");

        assert!(diagnostics.description.contains("cached"));
        assert_eq!(
            diagnostics.input_schema["properties"]["limit"]["default"],
            50
        );
        assert_eq!(
            diagnostics.input_schema["properties"]["limit"]["maximum"],
            100
        );
        assert_eq!(
            diagnostics.input_schema["properties"]["severity"]["enum"],
            json!(["error", "warning", "info", "hint"])
        );
        assert_eq!(
            diagnostics.input_schema["properties"]["component"]["enum"],
            json!(["parser", "dreamchecker"])
        );
    }

    #[test]
    fn enumeration_schemas_expose_hard_result_bounds() {
        let definitions = get_tool_definitions();
        let types = definitions
            .iter()
            .find(|tool| tool.name == "dm_list_types")
            .expect("type listing tool should be registered");
        let symbols = definitions
            .iter()
            .find(|tool| tool.name == "dm_search_symbols")
            .expect("symbol search tool should be registered");

        assert_eq!(types.input_schema["properties"]["limit"]["maximum"], 500);
        assert_eq!(
            types.input_schema["properties"]["cursor"]["pattern"],
            "^[0-9]+$"
        );
        assert_eq!(symbols.input_schema["properties"]["limit"]["maximum"], 200);
    }
}
