use std::fmt;
use std::path::{Path, PathBuf};

use crate::{EffectiveRoot, RootSource};

#[derive(Clone, Debug, serde::Serialize)]
pub struct PolicyContext {
    pub containment_mode: &'static str,
    pub policy_source: &'static str,
    pub effective_roots: Vec<EffectiveRoot>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PathPolicyStatus {
    #[serde(rename = "mode")]
    pub containment_mode: &'static str,
    pub policy_source: &'static str,
    pub effective_roots: Vec<EffectiveRoot>,
    pub compiler_allowlist: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct PolicyError {
    code: &'static str,
    path: PathBuf,
    message: String,
    context: Box<PolicyContext>,
}

impl PolicyError {
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn context(&self) -> &PolicyContext {
        &self.context
    }
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for PolicyError {}

#[derive(Clone, Debug)]
pub struct PathPolicy {
    roots: Vec<PathBuf>,
    effective_roots: Vec<EffectiveRoot>,
    executables: Vec<PathBuf>,
}

impl PathPolicy {
    pub fn new(roots: Vec<PathBuf>, executables: Vec<PathBuf>) -> Result<Self, PolicyError> {
        if roots.is_empty() {
            return Err(error(
                "no_workspace_roots",
                PathBuf::new(),
                "no workspace roots configured",
                empty_context(),
            ));
        }
        let roots = canonicalize(roots, "invalid_workspace_root")?;
        let effective_roots = roots
            .iter()
            .cloned()
            .map(|path| EffectiveRoot {
                path,
                source: RootSource::ExplicitRoot,
                repository_identity: None,
                head_revision: None,
                dirty: None,
            })
            .collect();
        Ok(Self {
            roots,
            effective_roots,
            executables: canonicalize(executables, "invalid_executable")?,
        })
    }

    pub fn from_effective_roots(
        effective_roots: Vec<EffectiveRoot>,
        executables: Vec<PathBuf>,
    ) -> Result<Self, PolicyError> {
        if effective_roots.is_empty() {
            return Err(error(
                "no_workspace_roots",
                PathBuf::new(),
                "no workspace roots configured",
                empty_context(),
            ));
        }
        let mut normalized = Vec::with_capacity(effective_roots.len());
        for mut root in effective_roots {
            root.path = root.path.canonicalize().map_err(|source| {
                error(
                    "invalid_workspace_root",
                    root.path.clone(),
                    format!("cannot resolve {}: {source}", root.path.display()),
                    empty_context(),
                )
            })?;
            normalized.push(root);
        }
        normalized.sort_by(|left, right| left.path.cmp(&right.path));
        normalized.dedup_by(|left, right| left.path == right.path);
        let roots = normalized.iter().map(|root| root.path.clone()).collect();
        Ok(Self {
            roots,
            effective_roots: normalized,
            executables: canonicalize(executables, "invalid_executable")?,
        })
    }

    pub fn read_path(&self, path: impl AsRef<Path>) -> Result<PathBuf, PolicyError> {
        let input = path.as_ref();
        let canonical = input.canonicalize().map_err(|source| {
            self.error(
                "path_not_found",
                input.to_owned(),
                format!("cannot resolve {}: {source}", input.display()),
            )
        })?;
        self.require_contained(canonical)
    }

    pub fn read_directory(&self, path: impl AsRef<Path>) -> Result<PathBuf, PolicyError> {
        let canonical = self.read_path(path)?;
        if !canonical.is_dir() {
            return Err(self.error(
                "invalid_input",
                canonical,
                "working_directory must be a directory",
            ));
        }
        Ok(canonical)
    }

    pub fn output_path(
        &self,
        path: impl AsRef<Path>,
        overwrite: bool,
    ) -> Result<PathBuf, PolicyError> {
        let input = path.as_ref();
        if input.exists() {
            let canonical = self.read_path(input)?;
            if !overwrite {
                return Err(self.error(
                    "output_exists",
                    canonical,
                    "output already exists; set overwrite=true",
                ));
            }
            return Ok(canonical);
        }
        let parent = input.parent().ok_or_else(|| {
            self.error(
                "invalid_output_path",
                input.to_owned(),
                "output path has no parent",
            )
        })?;
        let parent = parent.canonicalize().map_err(|source| {
            self.error(
                "output_parent_not_found",
                parent.to_owned(),
                format!("cannot resolve output parent: {source}"),
            )
        })?;
        let candidate = parent.join(input.file_name().ok_or_else(|| {
            self.error(
                "invalid_output_path",
                input.to_owned(),
                "output path has no file name",
            )
        })?);
        self.require_contained(candidate)
    }

    /// Validate the requested directory before canonicalization can erase its
    /// identity as a symbolic link or Windows junction.
    pub fn directory_output_path(
        &self,
        path: impl AsRef<Path>,
        overwrite: bool,
    ) -> Result<PathBuf, PolicyError> {
        // Strip trailing separators and `.` so `link/` and `link/.` cannot make
        // symlink_metadata follow the final link instead of inspecting it.
        let input: PathBuf = path.as_ref().components().collect();
        match std::fs::symlink_metadata(&input) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(self.error(
                    "invalid_output_path",
                    input,
                    "output must be a directory, not a file or link",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(self.error(
                    "invalid_output_path",
                    input,
                    format!("cannot inspect output directory: {error}"),
                ));
            }
        }
        self.output_path(input, overwrite)
    }

    pub fn executable(&self, path: impl AsRef<Path>) -> Result<PathBuf, PolicyError> {
        let input = path.as_ref();
        let canonical = input.canonicalize().map_err(|_| {
            self.error(
                "executable_not_allowed",
                input.to_owned(),
                "executable is not allowlisted",
            )
        })?;
        if self.executables.contains(&canonical) {
            Ok(canonical)
        } else {
            Err(self.error(
                "executable_not_allowed",
                canonical,
                "executable is not allowlisted",
            ))
        }
    }

    pub fn runtime_dmb(&self, path: impl AsRef<Path>) -> Result<PathBuf, PolicyError> {
        self.read_path(path)
    }

    pub fn compiler_allowlist(&self) -> &[PathBuf] {
        &self.executables
    }

    pub fn workspace_roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn effective_roots(&self) -> &[EffectiveRoot] {
        &self.effective_roots
    }

    pub fn status(&self) -> PathPolicyStatus {
        PathPolicyStatus {
            containment_mode: "immutable_startup_roots",
            policy_source: "server_startup_configuration",
            effective_roots: self.effective_roots.clone(),
            compiler_allowlist: self.executables.clone(),
        }
    }

    fn require_contained(&self, path: PathBuf) -> Result<PathBuf, PolicyError> {
        if self.roots.iter().any(|root| path.starts_with(root)) {
            Ok(path)
        } else {
            Err(self.error(
                "path_outside_workspace",
                path,
                "path is outside configured workspace roots",
            ))
        }
    }

    fn context(&self) -> PolicyContext {
        PolicyContext {
            containment_mode: "immutable_startup_roots",
            policy_source: "server_startup_configuration",
            effective_roots: self.effective_roots.clone(),
        }
    }

    fn error(&self, code: &'static str, path: PathBuf, message: impl Into<String>) -> PolicyError {
        error(code, path, message, self.context())
    }
}

fn canonicalize(paths: Vec<PathBuf>, code: &'static str) -> Result<Vec<PathBuf>, PolicyError> {
    paths
        .into_iter()
        .map(|path| {
            path.canonicalize().map_err(|source| {
                error(
                    code,
                    path.clone(),
                    format!("cannot resolve {}: {source}", path.display()),
                    empty_context(),
                )
            })
        })
        .collect()
}

fn empty_context() -> PolicyContext {
    PolicyContext {
        containment_mode: "immutable_startup_roots",
        policy_source: "server_startup_configuration",
        effective_roots: Vec::new(),
    }
}

fn error(
    code: &'static str,
    path: PathBuf,
    message: impl Into<String>,
    context: PolicyContext,
) -> PolicyError {
    PolicyError {
        code,
        path,
        message: message.into(),
        context: Box::new(context),
    }
}

impl dreammaker::ReadPolicy for PathPolicy {
    fn resolve(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.read_path(path)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))
    }
}
