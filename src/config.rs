use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

use crate::repository_roots::{expand_effective_roots, EffectiveRoot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityMode {
    Analysis,
    Development,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    #[default]
    All,
    Code,
    Assets,
    Runtime,
}

impl std::str::FromStr for ToolProfile {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "all" => Ok(Self::All),
            "code" => Ok(Self::Code),
            "assets" => Ok(Self::Assets),
            "runtime" => Ok(Self::Runtime),
            _ => Err(anyhow!("unknown MERIDIAN_MCP_TOOL_PROFILE value: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RiftBuildAccess {
    Disabled,
    Offline,
    Network,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebuggerAccess {
    Disabled,
    Auxtools,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TracyAccess {
    Disabled,
    Byond,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    mode: CapabilityMode,
    tool_profile: ToolProfile,
    workspace_roots: Vec<PathBuf>,
    effective_roots: Vec<EffectiveRoot>,
    compiler_allowlist: Vec<PathBuf>,
    rift_build_access: RiftBuildAccess,
    helper_manifest: Option<PathBuf>,
    state_directory: Option<PathBuf>,
    debugger_access: DebuggerAccess,
    tracy_access: TracyAccess,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self> {
        let tool_profile = match std::env::var("MERIDIAN_MCP_TOOL_PROFILE") {
            Ok(value) => value.parse()?,
            Err(std::env::VarError::NotPresent) => ToolProfile::All,
            Err(error) => return Err(error).context("MERIDIAN_MCP_TOOL_PROFILE must be Unicode"),
        };
        let mode = std::env::var("MERIDIAN_MCP_MODE").ok();
        let roots = std::env::var_os("MERIDIAN_MCP_ROOTS").ok_or_else(|| {
            anyhow!("MERIDIAN_MCP_ROOTS must contain at least one workspace root")
        })?;
        let compilers = std::env::var_os("MERIDIAN_MCP_COMPILERS");
        let repositories = std::env::var_os("MERIDIAN_MCP_REPOSITORIES")
            .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .unwrap_or_default();
        let rift_build = std::env::var("MERIDIAN_MCP_RIFT_BUILD").ok();
        let helper_manifest = std::env::var_os("MERIDIAN_MCP_HELPER_MANIFEST").map(PathBuf::from);
        let tracy = std::env::var("MERIDIAN_MCP_TRACY").ok();
        let state_directory = std::env::var_os("MERIDIAN_MCP_STATE_DIR").map(PathBuf::from);
        let mut config = Self::from_values_with_all_features(
            mode.as_deref(),
            std::env::split_paths(&roots).collect(),
            repositories,
            compilers
                .as_deref()
                .map(std::env::split_paths)
                .map(Iterator::collect)
                .unwrap_or_default(),
            rift_build.as_deref(),
            tracy.as_deref(),
            helper_manifest,
            state_directory,
        )?;
        config.debugger_access = match std::env::var("MERIDIAN_MCP_DEBUGGER").ok().as_deref() {
            None | Some("disabled") => DebuggerAccess::Disabled,
            Some("auxtools") if config.mode == CapabilityMode::Development => {
                DebuggerAccess::Auxtools
            }
            Some("auxtools") => {
                return Err(anyhow!(
                    "MERIDIAN_MCP_DEBUGGER=auxtools requires development mode"
                ))
            }
            Some(value) => return Err(anyhow!("unknown MERIDIAN_MCP_DEBUGGER value: {value}")),
        };
        Ok(config.with_tool_profile(tool_profile))
    }

    pub fn from_values(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
    ) -> Result<Self> {
        Self::from_values_with_features(mode, workspace_roots, compiler_allowlist, None, None, None)
    }

    pub fn from_values_with_state(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
        state_directory: Option<PathBuf>,
    ) -> Result<Self> {
        Self::from_values_with_features_and_state(
            mode,
            workspace_roots,
            compiler_allowlist,
            None,
            None,
            None,
            state_directory,
        )
    }

    pub fn from_values_with_rift_build(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
        rift_build: Option<&str>,
    ) -> Result<Self> {
        Self::from_values_with_features(
            mode,
            workspace_roots,
            compiler_allowlist,
            rift_build,
            None,
            None,
        )
    }

    pub fn from_values_with_features(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
        rift_build: Option<&str>,
        tracy: Option<&str>,
        helper_manifest: Option<PathBuf>,
    ) -> Result<Self> {
        Self::from_values_with_all_features(
            mode,
            workspace_roots,
            Vec::new(),
            compiler_allowlist,
            rift_build,
            tracy,
            helper_manifest,
            None,
        )
    }

    pub fn from_values_with_features_and_state(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
        rift_build: Option<&str>,
        tracy: Option<&str>,
        helper_manifest: Option<PathBuf>,
        state_directory: Option<PathBuf>,
    ) -> Result<Self> {
        Self::from_values_with_all_features(
            mode,
            workspace_roots,
            Vec::new(),
            compiler_allowlist,
            rift_build,
            tracy,
            helper_manifest,
            state_directory,
        )
    }

    pub fn from_values_with_rift_build_and_state(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
        rift_build: Option<&str>,
        state_directory: Option<PathBuf>,
    ) -> Result<Self> {
        Self::from_values_with_features_and_state(
            mode,
            workspace_roots,
            compiler_allowlist,
            rift_build,
            None,
            None,
            state_directory,
        )
    }

    pub fn from_values_with_repositories(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        repositories: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
    ) -> Result<Self> {
        Self::from_values_with_all_features(
            mode,
            workspace_roots,
            repositories,
            compiler_allowlist,
            None,
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_values_with_all_features(
        mode: Option<&str>,
        workspace_roots: Vec<PathBuf>,
        repositories: Vec<PathBuf>,
        compiler_allowlist: Vec<PathBuf>,
        rift_build: Option<&str>,
        tracy: Option<&str>,
        helper_manifest: Option<PathBuf>,
        state_directory: Option<PathBuf>,
    ) -> Result<Self> {
        let mode = match mode.unwrap_or("analysis") {
            "analysis" => CapabilityMode::Analysis,
            "development" => CapabilityMode::Development,
            other => return Err(anyhow!("unknown MERIDIAN_MCP_MODE value: {other}")),
        };
        let rift_build_access = match rift_build.unwrap_or("disabled") {
            "disabled" => RiftBuildAccess::Disabled,
            "offline" => RiftBuildAccess::Offline,
            "network" => RiftBuildAccess::Network,
            other => {
                return Err(anyhow!("unknown MERIDIAN_MCP_RIFT_BUILD value: {other}"));
            }
        };
        if workspace_roots.is_empty() {
            return Err(anyhow!("at least one workspace root is required"));
        }
        let tracy_access = match tracy.unwrap_or("disabled") {
            "disabled" => TracyAccess::Disabled,
            "byond" if mode == CapabilityMode::Development && helper_manifest.is_some() => {
                TracyAccess::Byond
            }
            "byond" if mode != CapabilityMode::Development => {
                return Err(anyhow!(
                    "MERIDIAN_MCP_TRACY=byond requires development mode"
                ));
            }
            "byond" => {
                return Err(anyhow!(
                    "MERIDIAN_MCP_TRACY=byond requires MERIDIAN_MCP_HELPER_MANIFEST"
                ));
            }
            other => return Err(anyhow!("unknown MERIDIAN_MCP_TRACY value: {other}")),
        };
        let effective_roots = expand_effective_roots(&workspace_roots, &repositories)?;
        let state_directory = match (mode, state_directory) {
            (CapabilityMode::Analysis, None) => None,
            (CapabilityMode::Analysis, Some(_)) => {
                return Err(anyhow!(
                    "MERIDIAN_MCP_STATE_DIR is not accepted in analysis mode"
                ));
            }
            (CapabilityMode::Development, None) => {
                return Err(anyhow!(
                    "MERIDIAN_MCP_STATE_DIR is required in development mode"
                ));
            }
            (CapabilityMode::Development, Some(path)) => {
                let metadata = std::fs::symlink_metadata(&path).with_context(|| {
                    format!("cannot inspect private state directory: {}", path.display())
                })?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(anyhow!(
                        "MERIDIAN_MCP_STATE_DIR must be an existing non-symlink directory"
                    ));
                }
                let path = path.canonicalize()?;
                if effective_roots
                    .iter()
                    .any(|root| path.starts_with(&root.path) || root.path.starts_with(&path))
                {
                    return Err(anyhow!(
                        "MERIDIAN_MCP_STATE_DIR must be outside every workspace root"
                    ));
                }
                Some(path)
            }
        };
        let workspace_roots = effective_roots
            .iter()
            .map(|root| root.path.clone())
            .collect();
        Ok(Self {
            mode,
            tool_profile: ToolProfile::All,
            workspace_roots,
            effective_roots,
            compiler_allowlist: canonicalize_all(compiler_allowlist, "compiler")?,
            rift_build_access,
            helper_manifest: helper_manifest
                .map(|path| {
                    path.canonicalize().with_context(|| {
                        format!("cannot canonicalize helper manifest: {}", path.display())
                    })
                })
                .transpose()?,
            state_directory,
            debugger_access: DebuggerAccess::Disabled,
            tracy_access,
        })
    }

    pub fn mode(&self) -> CapabilityMode {
        self.mode
    }
    pub fn tool_profile(&self) -> ToolProfile {
        self.tool_profile
    }
    pub fn with_tool_profile(mut self, profile: ToolProfile) -> Self {
        self.tool_profile = profile;
        self
    }
    pub fn workspace_roots(&self) -> &[PathBuf] {
        &self.workspace_roots
    }
    pub fn effective_roots(&self) -> &[EffectiveRoot] {
        &self.effective_roots
    }
    pub fn compiler_allowlist(&self) -> &[PathBuf] {
        &self.compiler_allowlist
    }
    pub fn rift_build_access(&self) -> RiftBuildAccess {
        self.rift_build_access
    }
    pub fn helper_manifest(&self) -> Option<&std::path::Path> {
        self.helper_manifest.as_deref()
    }
    pub fn state_directory(&self) -> Option<&std::path::Path> {
        self.state_directory.as_deref()
    }
    pub fn debugger_access(&self) -> DebuggerAccess {
        self.debugger_access
    }
    pub fn tracy_access(&self) -> TracyAccess {
        self.tracy_access
    }
}

fn canonicalize_all(paths: Vec<PathBuf>, label: &str) -> Result<Vec<PathBuf>> {
    paths
        .into_iter()
        .map(|path| {
            path.canonicalize()
                .with_context(|| format!("cannot canonicalize {label}: {}", path.display()))
        })
        .collect()
}
