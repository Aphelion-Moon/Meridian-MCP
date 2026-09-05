pub mod analysis_snapshot;
pub mod artifact;
pub mod atomic_output;
pub mod build_identity;
pub mod build_provenance;
pub mod capabilities;
pub mod config;
pub mod contracts;
pub mod fixture_manifest;
pub mod helper_manifest;
pub mod index;
pub mod limits;
pub mod mcp;
pub mod memory_evidence;
pub mod native_evidence;
pub mod native_memory;
pub mod network_audit;
pub mod parameters;
pub mod path_policy;
pub mod private_state;
pub mod proc_resolution;
pub mod process;
pub mod process_environment;
pub mod process_metrics;
pub mod project;
pub mod repository_roots;
pub mod result;
pub mod runtime_integrity;
mod search;
pub mod semantic;
pub mod server;
mod source;
pub mod source_fingerprint;
pub mod spaceman;
pub mod state;
pub mod tools;
pub mod tracy;
pub mod tracy_artifact;
pub mod tracy_collector;
pub mod tracy_experiment;
pub mod tracy_protocol;
pub mod tracy_runtime_config;
pub mod tracy_statistics;
pub mod workspace_integrity;

pub use artifact::FileIdentity;
pub use build_provenance::{
    BuildAttempt, BuildAttemptOutcome, BuildInputIdentity, BuildProvenanceStore, BuildRecord,
    LaunchDecision, LaunchProvenance, ProjectBuildIdentity, ProvenanceReason, ProvenanceStatus,
};
pub use config::{CapabilityMode, DebuggerAccess, RiftBuildAccess, ServerConfig, TracyAccess};
pub use contracts::{
    all_contracts, contracts_for, contracts_for_configuration, render_tool_reference, SupportLevel,
    ToolContract, ToolEffects,
};
pub use fixture_manifest::{
    FixtureInputDocument, FixtureInputRole, FixtureManifest, FixtureManifestDocument,
    FixtureManifestError, RequiredProcDocument, VerifiedFixtureInput, VerifiedFixtureManifest,
};
pub use parameters::RiftNetworkMode;
pub use path_policy::{PathPolicy, PathPolicyStatus, PolicyContext, PolicyError};
pub use private_state::PrivateStateStore;
pub use proc_resolution::{
    ProcResolution, ProcResolutionError, ProcResolutionKind, ProcResolver,
    ResolvedProcImplementation, SourceLocation,
};
pub use project::ProjectProfile;
pub use repository_roots::{expand_effective_roots, EffectiveRoot, RepositoryIdentity, RootSource};
pub use server::MeridianServer;
pub use source_fingerprint::SourceFingerprint;
pub use tools::rift::BuildEvidence;

pub async fn run(config: ServerConfig) -> anyhow::Result<()> {
    mcp::run_server(config).await
}
