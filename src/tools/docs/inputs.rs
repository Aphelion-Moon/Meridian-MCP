use crate::{limits::ServerLimits, PathPolicy};
use anyhow::{anyhow, Result};
use dreammaker::{Context, Preprocessor};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

type Inventory = BTreeMap<PathBuf, (PathBuf, u64, SystemTime)>;

// dmdoc reads live source and Markdown, not just the active analysis snapshot.
// Re-discover before installation so changed configuration or new inputs cannot
// turn an authorized output replacement into source deletion.
pub(super) fn capture(policy: &PathPolicy, environment: &Path, output: &Path) -> Result<Inventory> {
    let base = environment
        .parent()
        .ok_or_else(|| anyhow!("environment has no parent"))?;
    let mut context = Context::default();
    context.set_read_policy(Arc::new(policy.clone()));
    context.autodetect_config(environment);
    let preprocessor = Preprocessor::new(&context, environment.to_owned())?;
    for _ in preprocessor {}
    anyhow::ensure!(
        !context.read_denied(),
        "documentation input is outside workspace roots"
    );

    let mut inputs = Inventory::new();
    let mut roots = BTreeSet::new();
    add(&mut inputs, policy, environment, output)?;
    let config = base.join("SpacemanDMM.toml");
    match std::fs::symlink_metadata(&config) {
        Ok(_) => add(&mut inputs, policy, &config, output)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut source_paths = Vec::new();
    context
        .file_list()
        .for_each(|path| source_paths.push(path.to_owned()));
    for path in source_paths {
        if path.as_os_str().is_empty() {
            continue;
        }
        add(&mut inputs, policy, &base.join(&path), output)?;
        if let Some(Component::Normal(first)) = path.components().next() {
            roots.insert(base.join(first));
        }
    }
    if !context.config().dmdoc.module_directories.is_empty() {
        roots = context
            .config()
            .dmdoc
            .module_directories
            .iter()
            .map(|path| base.join(path))
            .collect();
    }
    if let Some(index) = &context.config().dmdoc.index_file {
        add(&mut inputs, policy, &base.join(index), output)?;
    }
    let limit = ServerLimits::default().max_docs_files;
    let mut visited = 0_usize;
    let mut stack: Vec<_> = roots.into_iter().map(|path| (path, true)).collect();
    while let Some((path, root)) = stack.pop() {
        visited += 1;
        anyhow::ensure!(
            visited <= limit,
            "documentation input scan exceeds {limit} entries"
        );
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        // WalkDir follows a root link, but does not descend through other links.
        if metadata.is_dir() || (root && metadata.file_type().is_symlink() && path.is_dir()) {
            let checked = policy.read_directory(&path)?;
            for entry in std::fs::read_dir(checked)? {
                anyhow::ensure!(
                    visited + stack.len() < limit,
                    "documentation input scan exceeds {limit} entries"
                );
                stack.push((path.join(entry?.file_name()), false));
            }
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("md" | "txt")
        ) {
            add(&mut inputs, policy, &path, output)?;
        }
    }
    Ok(inputs)
}

fn add(inputs: &mut Inventory, policy: &PathPolicy, path: &Path, output: &Path) -> Result<()> {
    let resolved = policy.read_path(path)?;
    anyhow::ensure!(
        !path.starts_with(output) && !resolved.starts_with(output),
        "documentation output must not contain documentation inputs"
    );
    let metadata = std::fs::metadata(&resolved)?;
    anyhow::ensure!(
        metadata.is_file(),
        "documentation input is not a regular file"
    );
    inputs.insert(
        path.to_owned(),
        (resolved, metadata.len(), metadata.modified()?),
    );
    Ok(())
}
