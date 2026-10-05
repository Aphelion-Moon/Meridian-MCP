use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RootSource {
    ExplicitRoot,
    LinkedGitWorktree,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RepositoryIdentity {
    pub kind: &'static str,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct EffectiveRoot {
    pub path: PathBuf,
    pub source: RootSource,
    pub repository_identity: Option<RepositoryIdentity>,
    pub head_revision: Option<String>,
    pub dirty: Option<bool>,
}

pub fn expand_effective_roots(
    explicit_roots: &[PathBuf],
    repositories: &[PathBuf],
) -> Result<Vec<EffectiveRoot>> {
    if explicit_roots.is_empty() && repositories.is_empty() {
        return Err(anyhow!("at least one workspace root is required"));
    }

    let mut roots = BTreeMap::new();
    for root in explicit_roots {
        let canonical = root
            .canonicalize()
            .with_context(|| format!("cannot canonicalize workspace root: {}", root.display()))?;
        roots.insert(
            canonical.clone(),
            effective_root(canonical, RootSource::ExplicitRoot, None),
        );
    }

    for repository in repositories {
        let repository = repository.canonicalize().with_context(|| {
            format!(
                "cannot canonicalize authorized repository: {}",
                repository.display()
            )
        })?;
        let common_directory = git_common_directory(&repository)?;
        let identity = repository_identity(&common_directory);
        for worktree in linked_worktrees(&repository)? {
            let worktree = worktree.canonicalize().with_context(|| {
                format!(
                    "cannot canonicalize linked worktree: {}",
                    worktree.display()
                )
            })?;
            let candidate_common_directory = git_common_directory(&worktree)?;
            if candidate_common_directory != common_directory {
                return Err(anyhow!(
                    "linked worktree has a different Git common directory: {}",
                    worktree.display()
                ));
            }
            roots.entry(worktree.clone()).or_insert_with(|| {
                effective_root(
                    worktree,
                    RootSource::LinkedGitWorktree,
                    Some(identity.clone()),
                )
            });
        }
    }

    if roots.is_empty() {
        return Err(anyhow!("no effective workspace roots were discovered"));
    }
    Ok(roots.into_values().collect())
}

fn effective_root(
    path: PathBuf,
    source: RootSource,
    known_identity: Option<RepositoryIdentity>,
) -> EffectiveRoot {
    let common_directory = git_common_directory(&path).ok();
    let repository_identity =
        known_identity.or_else(|| common_directory.as_deref().map(repository_identity));
    let (head_revision, dirty) = git_observation(&path);

    EffectiveRoot {
        path,
        source,
        repository_identity,
        head_revision,
        dirty,
    }
}

pub(crate) fn git_observation(path: &Path) -> (Option<String>, Option<bool>) {
    let head_revision = run_git(path, &["rev-parse", "HEAD"])
        .ok()
        .and_then(successful_stdout);
    let dirty = run_git(
        path,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| !output.stdout.is_empty());

    (head_revision, dirty)
}

fn git_common_directory(repository: &Path) -> Result<PathBuf> {
    let output = run_git(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_directory = successful_stdout(output).ok_or_else(|| {
        anyhow!(
            "authorized repository is not a valid Git worktree: {}",
            repository.display()
        )
    })?;
    PathBuf::from(common_directory)
        .canonicalize()
        .with_context(|| {
            format!(
                "cannot canonicalize Git common directory for {}",
                repository.display()
            )
        })
}

fn linked_worktrees(repository: &Path) -> Result<Vec<PathBuf>> {
    let output = run_git(repository, &["worktree", "list", "--porcelain", "-z"])?;
    if !output.status.success() {
        return Err(anyhow!(
            "cannot enumerate linked worktrees for {}: {}",
            repository.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut worktrees = Vec::new();
    for field in output.stdout.split(|byte| *byte == 0) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            let path =
                std::str::from_utf8(path).context("Git returned a non-UTF-8 worktree path")?;
            worktrees.push(PathBuf::from(path));
        }
    }
    if worktrees.is_empty() {
        return Err(anyhow!(
            "Git returned no worktrees for authorized repository: {}",
            repository.display()
        ));
    }
    Ok(worktrees)
}

fn repository_identity(common_directory: &Path) -> RepositoryIdentity {
    RepositoryIdentity {
        kind: "local_git_common_dir_sha256",
        digest: format!(
            "{:x}",
            Sha256::digest(common_directory.to_string_lossy().as_bytes())
        ),
    }
}

fn run_git(directory: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .with_context(|| format!("cannot run Git in {}", directory.display()))
}

fn successful_stdout(output: Output) -> Option<String> {
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}
