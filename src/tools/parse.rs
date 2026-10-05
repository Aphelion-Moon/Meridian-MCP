use anyhow::{anyhow, Result};
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tracing::info;

use dreammaker::{Context, Parser, Preprocessor, Severity};

use crate::analysis_snapshot::{
    collect_diagnostics, configured_diagnostic_rules, AnalysisBuild, AnalysisContext,
    AnalysisSnapshot,
};
use crate::mcp::ToolResult;
use crate::outputs::budget::{Budget, MEMBER_WORK};
use crate::outputs::*;
use crate::result::{json_success, structured_error, ToolErrorCode, ToolMetadata};
use crate::search::SearchDocuments;
use crate::semantic::SEMANTIC_CHUNK_SCHEMA_VERSION;
use crate::source_fingerprint::SourceFingerprint;
use crate::state::ServerState;
use crate::{PathPolicy, ProjectProfile};

/// Diagnostics that mean the environment was not fully read or its inheritance
/// graph cannot safely be analyzed.
///
/// SpacemanDMM has no structured discriminant for these, so they are matched on
/// description text. The strings below follow the pinned revision and local
/// delta (`preprocessor.rs`, `lexer.rs`, `objtree.rs`); if an update reworded them
/// the failure is silent and severe — a truncated tree installed as a success —
/// so `blocking_error_descriptions_match_upstream_wording` guards the list.
const BLOCKING_ERROR_PREFIXES: &[&str] = &[
    "failed to find #include",
    "failed to open file: #include",
    "cyclic parent_type",
];
const BLOCKING_ERROR_MESSAGES: &[&str] = &["i/o error opening file", "i/o error reading file"];

/// Default ceiling on a single parse. Generous enough for a station-sized
/// environment on a cold cache; present so a pathological input cannot wedge a
/// client forever with no reply.
const DEFAULT_PARSE_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_TYPE_LIMIT: u64 = 100;
const DEFAULT_SYMBOL_LIMIT: u64 = 50;

/// Render a canonicalized path the way a caller wrote it.
///
/// Containment canonicalizes arguments, which on Windows yields the `\\?\`
/// verbatim form. Echoing that back is technically correct and unreadable, so
/// reporting strips it the same way spawn paths do elsewhere in the tools.
fn display_path(path: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let path_text = path.to_string_lossy();
        if let Some(unc_path) = path_text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc_path}");
        }
        if let Some(dos_path) = path_text.strip_prefix(r"\\?\") {
            return dos_path.to_owned();
        }
    }

    path.display().to_string()
}

fn is_blocking_error(description: &str) -> bool {
    BLOCKING_ERROR_PREFIXES
        .iter()
        .any(|prefix| description.starts_with(prefix))
        || BLOCKING_ERROR_MESSAGES.contains(&description)
}

/// Parse a DreamMaker environment
#[cfg(test)]
pub async fn parse_environment(state: &ServerState, args: Value) -> Result<ToolResult> {
    let args: crate::parameters::ParseEnvironmentParams = crate::parameters::decode(args)?;
    let path = args.dme_path.as_str();
    let root = std::path::Path::new(path)
        .parent()
        .ok_or_else(|| anyhow!("Environment has no parent"))?;
    let policy = PathPolicy::new(vec![root.to_owned()], Vec::new())?;
    parse_environment_with_policy(state, args, &policy).await
}

struct ParsedEnvironment {
    snapshot: AnalysisSnapshot,
    preprocess_parse: u64,
    dreamchecker: u64,
    search_documents_ms: u64,
    build_timings: crate::analysis_snapshot::AnalysisBuildTimings,
}

enum ParseOutcome {
    Reused(Arc<AnalysisSnapshot>),
    Built(Box<ParsedEnvironment>),
}

#[cfg(test)]
pub(crate) async fn parse_environment_with_policy(
    state: &ServerState,
    args: crate::parameters::ParseEnvironmentParams,
    policy: &PathPolicy,
) -> Result<ToolResult> {
    let context = super::ToolExecutionContext::new(crate::CapabilityMode::Analysis, policy.clone());
    parse_environment_controlled(&context, state, args).await
}

pub(crate) async fn parse_environment_controlled(
    control: &super::ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::ParseEnvironmentParams,
) -> Result<ToolResult> {
    let request_started = control
        .request_started
        .unwrap_or_else(tokio::time::Instant::now)
        .into_std();
    let dme_path = args.dme_path.as_str();
    let scoped_state = state.for_parse(PathBuf::from(dme_path));
    let state = &scoped_state;
    let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(DEFAULT_PARSE_TIMEOUT_MS));
    let deadline = control.deadline(timeout.as_millis() as u64);
    let policy = control.policy();
    if control.finalization_reason(deadline).is_some() {
        return parse_timeout(state, timeout).await;
    }
    control.progress(crate::request::Stage::Admission);
    let path = PathBuf::from(dme_path);
    let force = args.force.unwrap_or(false);

    let queue_started = Instant::now();
    let permit = match tokio::time::timeout_at(deadline, state.parse_permit()).await {
        Ok(permit) => permit,
        Err(_) => return parse_timeout(state, timeout).await,
    };
    let queue_wait = queue_started.elapsed().as_millis() as u64;
    let candidate = match tokio::time::timeout_at(deadline, state.active_snapshot()).await {
        Ok(candidate) => candidate,
        Err(_) => return parse_timeout(state, timeout).await,
    };
    if control.finalization_reason(deadline).is_some() {
        return parse_timeout(state, timeout).await;
    }
    let policy = policy.clone();
    // Reuse validation includes blocking-pool scheduling; full build stage
    // timings measure work inside the worker and exclude that scheduling delay.
    let reuse_started = Instant::now();
    #[cfg(test)]
    let worker_test = state.parse_worker_test.clone();
    // Capture admission before spawning. Dropping any caller future or join
    // handle cannot release it while this non-abortable job is queued/running.
    // Returning it with the result also covers the snapshot-installation await.
    let worker_control = control.clone();
    let work_state = state.clone();
    let handle = async move {
        work_state
            .run_blocking_job(move || {
                let outcome = (|| -> Result<ParseOutcome> {
                    #[cfg(test)]
                    let _worker_test = worker_test.enter();
                    anyhow::ensure!(
                        worker_control.finalization_reason(deadline).is_none(),
                        "parse deadline expired"
                    );
                    if !path.is_file() {
                        let reason = if path.is_dir() {
                            format!(
                                "Not a file (expected a .dme environment): {}",
                                path.display()
                            )
                        } else {
                            format!("File not found: {}", path.display())
                        };
                        return Err(anyhow!(reason));
                    }
                    if !force {
                        if let Some(reused) = reusable_snapshot(candidate, &path) {
                            anyhow::ensure!(
                                worker_control.finalization_reason(deadline).is_none(),
                                "parse request ended"
                            );
                            return Ok(ParseOutcome::Reused(reused));
                        }
                    } else {
                        drop(candidate);
                    }
                    anyhow::ensure!(
                        worker_control.finalization_reason(deadline).is_none(),
                        "parse deadline expired"
                    );
                    info!("Parsing environment: {}", path.display());
                    build_environment_checked(path, policy, &worker_control, deadline)
                        .map(|build| ParseOutcome::Built(Box::new(build)))
                })();
                Ok((outcome, permit))
            })
            .await
    };
    let (outcome, _permit) = match tokio::time::timeout_at(deadline, handle).await {
        Ok(Ok(result)) if control.finalization_reason(deadline).is_none() => result,
        Ok(Err(error)) => {
            return parse_failure(
                state,
                ToolErrorCode::Internal,
                format!("parser worker failed: {error}"),
                Some("Retry the parse; report the worker failure if it recurs.".to_owned()),
            )
            .await
        }
        _ => return parse_timeout(state, timeout).await,
    };
    let (snapshot, reused, mut timings) = match outcome {
        Ok(ParseOutcome::Reused(snapshot)) => (
            snapshot,
            true,
            std::collections::BTreeMap::from([
                ("queue_wait".into(), queue_wait),
                (
                    "reuse_validation".into(),
                    reuse_started.elapsed().as_millis() as u64,
                ),
            ]),
        ),
        Ok(ParseOutcome::Built(parsed)) => {
            let ParsedEnvironment {
                snapshot,
                preprocess_parse,
                dreamchecker,
                search_documents_ms,
                build_timings,
            } = *parsed;
            let Some(snapshot) = state
                .install_analysis_checked(snapshot, deadline, || {
                    control.finalization_reason(deadline).is_none()
                })
                .await?
            else {
                return parse_timeout(state, timeout).await;
            };
            (
                snapshot,
                false,
                std::collections::BTreeMap::from([
                    ("queue_wait".into(), queue_wait),
                    ("preprocess_parse".into(), preprocess_parse),
                    ("dreamchecker".into(), dreamchecker),
                    ("search_documents".into(), search_documents_ms),
                    ("analysis_indexes".into(), build_timings.analysis_indexes),
                    ("fingerprint".into(), build_timings.fingerprint),
                ]),
            )
        }
        Err(error) => {
            return parse_failure(
                state,
                ToolErrorCode::InvalidInput,
                error.to_string(),
                Some(
                    "Correct the DreamMaker parse errors and run dm_parse_environment again."
                        .to_owned(),
                ),
            )
            .await
        }
    };
    let total = request_started.elapsed().as_millis() as u64;
    if control.finalization_reason(deadline).is_some() {
        return parse_timeout(state, timeout).await;
    }
    timings.insert("total".into(), total);
    let (errors, warnings) = snapshot.diagnostic_counts();
    crate::result::analysis_text(
        &snapshot,
        ParseEnvironmentData {
            success: true,
            reused,
            environment: display_path(&snapshot.environment_path),
            total_types: snapshot.total_types,
            indexed_symbols: snapshot.indexed_symbol_count(),
            error_count: errors,
            warning_count: warnings,
            state_generation: snapshot.generation,
            spacemandmm_revision: snapshot.spacemandmm_revision,
            spacemandmm_local_patch: crate::capabilities::SPACEMANDMM_LOCAL_PATCH,
            spacemandmm_local_patch_sha256: crate::capabilities::SPACEMANDMM_LOCAL_PATCH_SHA256,
            retrieval: RetrievalCapabilities {
                lexical: LexicalCapability {
                    status: "ready",
                    algorithm: "bm25",
                    documents: snapshot.indexed_symbol_count(),
                },
                dense: DenseCapability {
                    status: "not_configured",
                },
                semantic_chunk_schema_version: SEMANTIC_CHUNK_SCHEMA_VERSION,
            },
            timings_ms: timings,
            duration_ms: (!reused).then_some(total),
        },
    )
}

async fn parse_timeout(state: &ServerState, timeout: Duration) -> Result<ToolResult> {
    parse_failure(state, ToolErrorCode::TimedOut,
        format!("parse request exceeded its total {} ms budget; any unfinished worker retains admission until it exits", timeout.as_millis()),
        Some("Wait for the active parser worker to finish, then retry.".to_owned())).await
}

#[cfg(test)]
fn build_environment(parse_path: PathBuf, policy: PathPolicy) -> Result<ParsedEnvironment> {
    let control = super::ToolExecutionContext::new(crate::CapabilityMode::Analysis, policy.clone());
    build_environment_checked(
        parse_path,
        policy,
        &control,
        control.deadline(DEFAULT_PARSE_TIMEOUT_MS),
    )
}

fn build_environment_checked(
    parse_path: PathBuf,
    policy: PathPolicy,
    control: &super::ToolExecutionContext,
    deadline: tokio::time::Instant,
) -> Result<ParsedEnvironment> {
    anyhow::ensure!(
        control.finalization_reason(deadline).is_none(),
        "parse request ended"
    );
    control.progress(crate::request::Stage::Capture);
    let parse_started_at = SystemTime::now();
    let preprocess_started = Instant::now();
    let mut context = Context::default();
    context.set_read_policy(Arc::new(policy.clone()));
    context.autodetect_config(&parse_path);
    let mut preprocessor = Preprocessor::new(&context, parse_path.clone())?;
    let (fatal, objtree) = {
        let mut parser = Parser::new(&context, &mut preprocessor);
        parser.enable_procs();
        parser.parse_object_tree_2()
    };
    let defines = preprocessor.finalize();
    let blocking_errors = context
        .errors()
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity() == Severity::Error && is_blocking_error(diagnostic.description())
        })
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if context.read_denied() {
        return Err(anyhow!(
            "path_outside_workspace: derived read denied by startup policy"
        ));
    }
    if objtree.parent_cycle_detected() {
        return Err(anyhow!(
            "DreamMaker parser reported cyclic parent_type inheritance"
        ));
    }
    if fatal || !blocking_errors.is_empty() {
        let diagnostics = blocking_errors.join("\n");
        return Err(anyhow!("DreamMaker parser reported errors:\n{diagnostics}"));
    }
    let preprocess_parse = preprocess_started.elapsed().as_millis() as u64;
    anyhow::ensure!(
        control.finalization_reason(deadline).is_none(),
        "parse request ended"
    );
    control.progress(crate::request::Stage::Execution);

    // Always run DreamChecker so semantic diagnostics describe this parsed
    // snapshot. Its cost is reported separately in the parse response.
    let dreamchecker_started = Instant::now();
    dreamchecker::run(&context, &objtree);
    let configured_rules = configured_diagnostic_rules(&parse_path, &context);
    let diagnostics = collect_diagnostics(&context, &configured_rules);
    let dreamchecker = dreamchecker_started.elapsed().as_millis() as u64;
    anyhow::ensure!(
        control.finalization_reason(deadline).is_none(),
        "parse request ended"
    );
    control.progress(crate::request::Stage::Evidence);
    let search_documents_started = Instant::now();
    let search_documents = SearchDocuments::from_object_tree(&objtree, &context, &parse_path);
    let search_documents_ms = search_documents_started.elapsed().as_millis() as u64;
    if context.read_denied() {
        return Err(anyhow!(
            "path_outside_workspace: derived read denied by startup policy"
        ));
    }
    let profile = ProjectProfile::discover(&policy, &parse_path).ok();
    let (build, build_timings) = AnalysisBuild::from_parse(
        parse_path,
        &context,
        objtree,
        defines,
        search_documents,
        diagnostics,
        profile,
        parse_started_at,
        policy,
    );
    anyhow::ensure!(
        control.finalization_reason(deadline).is_none(),
        "parse request ended"
    );
    control.progress(crate::request::Stage::Publication);
    Ok(ParsedEnvironment {
        snapshot: AnalysisSnapshot::from_build(build, 0),
        preprocess_parse,
        dreamchecker,
        search_documents_ms,
        build_timings,
    })
}

/// The active snapshot, when it already describes exactly this environment.
///
/// Returns `None` whenever reuse cannot be proven safe: a different environment,
/// or source files whose on-disk state does not match the snapshot's fingerprint.
pub(super) fn reusable_snapshot(
    snapshot: Option<Arc<AnalysisSnapshot>>,
    path: &std::path::Path,
) -> Option<Arc<AnalysisSnapshot>> {
    let snapshot = snapshot?;
    if snapshot.environment_path.as_path() != path {
        return None;
    }
    let current = SourceFingerprint::capture_with_discovery(
        snapshot.source_inputs(),
        snapshot.source_fingerprint.discovery_paths(),
        SystemTime::now(),
    );
    snapshot
        .source_fingerprint
        .matches(&current)
        .then_some(snapshot)
}

async fn parse_failure(
    state: &ServerState,
    code: ToolErrorCode,
    error: String,
    recovery: Option<String>,
) -> Result<ToolResult> {
    let metadata = state.analysis_metadata();
    let analysis = metadata.identity.clone();
    let result = structured_error(
        code,
        error,
        recovery,
        json!({
        "state_preserved": true,
        "active_environment": metadata.active_environment.as_deref().map(display_path),
        "state_generation": metadata.generation
        ,"analysis": metadata.identity,
        "requested_environment": state.requested_environment().map(display_path)
        }),
    );
    Ok(match analysis {
        Some(analysis) => result.with_analysis(analysis),
        None => result,
    })
}

/// Helper to get file path string from a location
fn get_file_path(context: &AnalysisContext, file_id: dreammaker::FileId) -> String {
    context.file_path(file_id).display().to_string()
}

/// Get type information
pub async fn get_type(
    state: &ServerState,
    args: crate::parameters::GetTypeParams,
) -> Result<ToolResult> {
    use crate::parameters::GetTypeSection as Section;
    let snapshot = state.snapshot().await?;
    let Some(ty) = snapshot.objtree.find(&args.type_path) else {
        return Ok(ToolResult::error(format!(
            "Type not found: {}",
            args.type_path
        )));
    };
    let selected = args.sections.clone().unwrap_or_else(|| {
        vec![
            Section::Documentation,
            Section::Vars,
            Section::Procs,
            Section::Children,
        ]
    });
    let mut normalized = selected.clone();
    normalized.sort();
    let compact = args.detail == Some(crate::parameters::DocumentSymbolsDetail::Compact);
    let codec = crate::cursor::Cursor::new(
        &snapshot,
        if compact { "compact" } else { "full" },
        json!(["get_type", "members_order_v1", args.type_path, normalized]),
    );
    let offset = codec.decode(args.cursor.as_deref())?;
    let mut budget = Budget::default();
    let mut omissions = Omissions::default();
    let documentation = selected.contains(&Section::Documentation).then(|| {
        budget.document(
            snapshot
                .search_index
                .type_document(&ty.path)
                .map(|v| v.docs.as_str()),
            "documentation",
            &mut omissions,
        )
    });
    let mut vars = selected.contains(&Section::Vars).then(Vec::new);
    let mut procs = selected.contains(&Section::Procs).then(Vec::new);
    let mut children = selected.contains(&Section::Children).then(Vec::new);
    let total = if vars.is_some() { ty.vars.len() } else { 0 }
        + if procs.is_some() { ty.procs.len() } else { 0 }
        + if children.is_some() {
            ty.len_children()
        } else {
            0
        };
    if offset > total {
        anyhow::bail!("invalid_input: cursor offset exceeds this type's selected members")
    }
    let maximum = args.limit.unwrap_or(MEMBER_WORK as u64) as usize;
    let mut position = offset.min(if vars.is_some() { ty.vars.len() } else { 0 });
    let mut count = 0;
    // Combined section order is stable, independent of the caller's section order.
    // A retained identifying row always advances, even if its large field is omitted.
    if let Some(rows) = vars.as_mut() {
        for (name, var) in ty.vars.iter().skip(offset.min(ty.vars.len())) {
            if position >= offset && count < maximum && budget.bytes > 256 {
                let mut fields = Omissions::default();
                budget.bytes = budget.bytes.saturating_sub(192);
                let name = budget.text(name, 4096, "name", &mut fields);
                let constant = if compact {
                    None
                } else {
                    budget.constant(var.value.constant.as_ref(), "constant", &mut fields)
                };
                rows.push(TypeVariable {
                    name,
                    has_value: var.value.expression.is_some(),
                    constant,
                    declared_here: var.declaration.is_some(),
                    field_omissions: fields,
                });
                count += 1;
            } else if position >= offset {
                break;
            }
            position += 1;
        }
    }
    // Skip complete preceding sections without visiting each skipped row.
    let vars_total = if vars.is_some() { ty.vars.len() } else { 0 };
    if position == vars_total || offset >= vars_total {
        position = vars_total
            + offset.saturating_sub(vars_total).min(if procs.is_some() {
                ty.procs.len()
            } else {
                0
            });
        if let Some(rows) = procs.as_mut() {
            for (name, proc) in ty
                .procs
                .iter()
                .skip(offset.saturating_sub(vars_total).min(ty.procs.len()))
            {
                if position >= offset && count < maximum && budget.bytes > 256 {
                    budget.bytes = budget.bytes.saturating_sub(160);
                    rows.push(TypeProcedure {
                        name: budget.text(name, 4096, "procs.name", &mut omissions),
                        parameter_count: proc.value.first().map_or(0, |v| v.parameters.len()),
                        override_count: proc.value.len(),
                        declared_here: proc.declaration.is_some(),
                    });
                    count += 1;
                } else if position >= offset {
                    break;
                }
                position += 1;
            }
        }
        let procs_end = vars_total + if procs.is_some() { ty.procs.len() } else { 0 };
        if position == procs_end || offset >= procs_end {
            position = procs_end
                + offset.saturating_sub(procs_end).min(if children.is_some() {
                    ty.len_children()
                } else {
                    0
                });
            if let Some(rows) = children.as_mut() {
                for child in ty
                    .children()
                    .skip(offset.saturating_sub(procs_end).min(ty.len_children()))
                {
                    if position >= offset && count < maximum && budget.bytes > 256 {
                        budget.bytes = budget.bytes.saturating_sub(16);
                        rows.push(budget.text(&child.path, 4096, "children", &mut omissions));
                        count += 1;
                    } else if position >= offset {
                        break;
                    }
                    position += 1;
                }
            }
        }
    }
    let next = offset + count;
    let pagination =
        (args.limit.is_some() || args.cursor.is_some() || args.sections.is_some() || next < total)
            .then(|| TypePagination {
                offset,
                count,
                total_count: total,
                next_cursor: (next < total).then(|| codec.encode(next)),
                evaluation_complete: next == total,
            });
    crate::result::analysis_text(
        &snapshot,
        GetTypeData {
            path: ty.path.to_string(),
            parent: ty.parent_type().map(|v| v.path.to_string()),
            children,
            documentation,
            vars,
            procs,
            location: format!(
                "{}:{}:{}",
                get_file_path(&snapshot.context, ty.location.file),
                ty.location.line,
                ty.location.column
            ),
            pagination,
            field_omissions: omissions,
        },
    )
}

pub async fn get_proc(
    state: &ServerState,
    args: crate::parameters::GetProcParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let include_source = args.include_source.unwrap_or(true);
    let maximum = args
        .max_source_lines
        .unwrap_or(snapshot.search_index.source_line_limit() as u64) as usize;
    let resolution = match snapshot
        .proc_resolver()
        .view(&args.type_path, &args.proc_name)
    {
        Ok(view) => view,
        Err(error) => {
            return Ok(structured_error(
                if matches!(
                    error,
                    crate::proc_resolution::ProcResolutionError::HierarchyLimit
                ) {
                    ToolErrorCode::LimitExceeded
                } else {
                    ToolErrorCode::NotFound
                },
                error.to_string(),
                Some("Check the exact type and proc names in the active analysis snapshot.".into()),
                serde_json::to_value(error)?,
            ))
        }
    };
    let mut budget = Budget::default();
    let mut omissions = Omissions::default();
    let mut overrides = Vec::new();
    for implementation in resolution.implementations.take(MEMBER_WORK + 1) {
        if overrides.len() == MEMBER_WORK || budget.bytes < 256 {
            omissions
                .fields
                .insert("overrides".into(), "aggregate response/work limit".into());
            break;
        }
        let owner = snapshot
            .objtree
            .find(&implementation.owner)
            .expect("resolved owner remains in snapshot");
        let value = owner
            .procs
            .get(args.proc_name.as_str())
            .and_then(|v| v.value.get(implementation.override_index))
            .expect("resolved override remains in snapshot");
        let mut fields = Omissions::default();
        let location = format!(
            "{}:{}:{}",
            implementation.location.file,
            implementation.location.line,
            implementation.location.column
        );
        let identifying_bytes =
            crate::result::encoded_bytes(&(&implementation.owner, &location), usize::MAX)
                .unwrap_or(usize::MAX)
                .saturating_add(256);
        if !overrides.is_empty() && identifying_bytes.saturating_add(256) > budget.bytes {
            omissions
                .fields
                .insert("overrides".into(), "aggregate response byte limit".into());
            break;
        }
        budget.bytes = budget.bytes.saturating_sub(256);
        let implementation_owner =
            budget.text(&implementation.owner, 64 * 1024, "owner", &mut fields);
        let location = budget.text(&location, 64 * 1024, "location", &mut fields);
        let mut parameters = Vec::new();
        for parameter in value.parameters.iter().take(MEMBER_WORK) {
            if budget.bytes < 128 {
                fields
                    .fields
                    .insert("parameters".into(), "aggregate response/work limit".into());
                break;
            }
            budget.bytes = budget.bytes.saturating_sub(64);
            parameters.push(Parameter {
                name: budget.text(&parameter.name, 4096, "parameters", &mut fields),
                has_default: parameter.default.is_some(),
            });
        }
        if parameters.len() < value.parameters.len() {
            fields
                .fields
                .insert("parameters".into(), "aggregate response/work limit".into());
        }
        let document = snapshot.search_index.proc_document(
            &implementation.owner,
            &args.proc_name,
            implementation.override_index,
        );
        let documentation = budget.document(
            document.map(|v| v.docs.as_str()),
            "documentation",
            &mut fields,
        );
        let mut source = if include_source {
            budget.source(document.and_then(|v| v.source.as_ref()), maximum)
        } else {
            SourceFields::default()
        };
        if include_source {
            source.source_origin = Some("analysis_snapshot");
            source.source_line_limit = Some(maximum);
        }
        overrides.push(ProcImplementation {
            owner: implementation_owner,
            override_index: implementation.override_index,
            parameters,
            documentation,
            location,
            has_body: implementation.has_body,
            source,
            field_omissions: fields,
        });
    }
    let diagnostics = if resolution.implementation_owner == args.type_path {
        Vec::new()
    } else {
        vec![format!(
            "requested type inherits the implementation from {}",
            resolution.implementation_owner
        )]
    };
    crate::result::analysis_text(
        &snapshot,
        GetProcData {
            name: args.proc_name.clone(),
            type_path: args.type_path.clone(),
            requested_type_path: args.type_path.clone(),
            implementation_owner: resolution.implementation_owner.into(),
            declaration_owner: resolution.declaration_owner.into(),
            resolution_kind: resolution.resolution_kind,
            declared: resolution.implementation_owner == args.type_path,
            overrides,
            resolution_diagnostics: diagnostics,
            state_generation: snapshot.generation,
            field_omissions: omissions,
        },
    )
}

pub async fn get_var(
    state: &ServerState,
    args: crate::parameters::GetVarParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let Some(ty) = snapshot.objtree.find(&args.type_path) else {
        return Ok(ToolResult::error(format!(
            "Type not found: {}",
            args.type_path
        )));
    };
    let Some(value) = ty.get_value(&args.var_name) else {
        return Ok(ToolResult::error(format!(
            "Variable not found: {}/{}",
            args.type_path, args.var_name
        )));
    };
    let value_owner = ty
        .iter_parent_types()
        .find(|o| o.vars.contains_key(args.var_name.as_str()))
        .expect("resolved variable owner");
    let declaration = ty.get_var_declaration(&args.var_name);
    let declaration_owner = ty.iter_parent_types().find(|o| {
        o.vars
            .get(args.var_name.as_str())
            .is_some_and(|v| v.declaration.is_some())
    });
    let docs_owner = if value.docs.is_empty() {
        declaration_owner
    } else {
        Some(value_owner)
    };
    let document =
        docs_owner.and_then(|o| snapshot.search_index.var_document(&o.path, &args.var_name));
    let mut budget = Budget::default();
    let mut omissions = Omissions::default();
    let documentation = budget.document(
        document.map(|v| v.docs.as_str()),
        "documentation",
        &mut omissions,
    );
    let constant = budget.constant(value.constant.as_ref(), "constant", &mut omissions);
    crate::result::analysis_text(
        &snapshot,
        GetVarData {
            name: args.var_name.clone(),
            type_path: args.type_path.clone(),
            declared: ty
                .vars
                .get(args.var_name.as_str())
                .is_some_and(|v| v.declaration.is_some()),
            declared_type: declaration.map(|v| v.var_type.to_string()),
            value_owner: value_owner.path.to_string(),
            declaration_owner: declaration_owner.map(|v| v.path.to_string()),
            inherited: value_owner.path != ty.path,
            documentation,
            constant,
            has_expression: value.expression.is_some(),
            location: format!(
                "{}:{}:{}",
                get_file_path(&snapshot.context, value.location.file),
                value.location.line,
                value.location.column
            ),
            declaration_location: declaration.map(|v| {
                format!(
                    "{}:{}:{}",
                    get_file_path(&snapshot.context, v.location.file),
                    v.location.line,
                    v.location.column
                )
            }),
            state_generation: snapshot.generation,
            field_omissions: omissions,
        },
    )
}

/// List types in the object tree
pub async fn list_types(
    state: &ServerState,
    args: crate::parameters::ListTypesParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;

    let prefix = args.prefix.as_deref().unwrap_or("");

    let max_depth = args.max_depth.map(|d| d as usize);
    let limit = args.limit.unwrap_or(DEFAULT_TYPE_LIMIT) as usize;
    let codec = crate::cursor::Cursor::new(
        &snapshot,
        "full",
        json!(["list_types", "object_tree_order_v1", prefix, max_depth]),
    );
    let cursor = codec.decode(args.cursor.as_deref())?;

    let matching_types: Vec<_> = objtree
        .iter_types()
        .filter(|ty| ty.path.starts_with(prefix))
        .filter(|ty| {
            if let Some(max) = max_depth {
                ty.path.matches('/').count() <= max
            } else {
                true
            }
        })
        .collect();
    let total_count = matching_types.len();
    anyhow::ensure!(
        cursor <= total_count,
        "Cursor offset exceeds the result count"
    );
    let mut budget = Budget::default();
    let mut types = Vec::new();
    let mut byte_limited = false;
    for ty in matching_types.into_iter().skip(cursor).take(limit) {
        let size = crate::result::encoded_bytes(&ty.path, usize::MAX)
            .unwrap_or(usize::MAX)
            .saturating_add(80);
        if !types.is_empty() && size > budget.bytes {
            byte_limited = true;
            break;
        }
        budget.bytes = budget.bytes.saturating_sub(80);
        let mut omissions = Omissions::default();
        let path = budget.text(
            &ty.path,
            crate::outputs::budget::DETAIL_BYTES - 80,
            "path",
            &mut omissions,
        );
        byte_limited |= !omissions.is_empty();
        types.push(ListTypeRow {
            path,
            var_count: ty.vars.len(),
            proc_count: ty.procs.len(),
            field_omissions: omissions,
        });
    }
    let next_offset = cursor.saturating_add(types.len());
    let has_more = next_offset < total_count;
    let next_cursor = has_more.then(|| codec.encode(next_offset));

    let result = ListTypesData {
        count: types.len(),
        total_count,
        types,
        pagination: LegacyPagination {
            cursor: cursor.to_string(),
            limit,
            next_cursor,
            has_more,
        },
    };
    let mut metadata = ToolMetadata::for_snapshot(&snapshot);
    metadata.truncated = has_more || byte_limited;
    if byte_limited {
        metadata.truncation_reasons.push("type_page_bytes".into());
    }
    if has_more {
        metadata
            .truncation_reasons
            .push("result_page_limit".to_owned());
    }

    Ok(json_success(metadata, result))
}

/// Search for symbols
pub async fn search_symbols(
    state: &ServerState,
    args: crate::parameters::SearchSymbolsParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;

    let query = args.query.as_str().to_lowercase();

    let kind = args
        .kind
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("all");
    if !matches!(kind, "type" | "proc" | "var" | "macro" | "all") {
        return Err(anyhow!("kind must be one of: type, proc, var, macro, all"));
    }

    let limit = args.limit.unwrap_or(DEFAULT_SYMBOL_LIMIT) as usize;

    let mut results: Vec<SymbolSearchRow> = Vec::new();

    if kind == "all" || kind == "macro" {
        for symbol in snapshot.language_index.macros() {
            if results.len() >= limit {
                break;
            }
            if symbol.name.to_lowercase().contains(&query) {
                results.push(SymbolSearchRow::Macro {
                    name: symbol.name.to_string(),
                    location: format!("{}:{}", symbol.file, symbol.line),
                    file: symbol.file.to_string(),
                    line: symbol.line,
                    column: symbol.column,
                });
            }
        }
    }

    if kind == "all" || kind == "proc" {
        for resolution in snapshot.proc_resolver().resolutions().filter(|resolution| {
            resolution.requested_type_path == resolution.implementation_owner
                && resolution.proc_name.to_lowercase().contains(&query)
        }) {
            if results.len() >= limit {
                break;
            }
            let first = resolution
                .implementations
                .first()
                .expect("a resolved procedure has an implementation");
            results.push(SymbolSearchRow::Proc {
                name: resolution.proc_name.clone(),
                type_path: resolution.implementation_owner.clone(),
                implementation_owner: resolution.implementation_owner.clone(),
                declaration_owner: resolution.declaration_owner.clone(),
                resolution_kind: resolution.resolution_kind,
                location: format!("{}:{}", first.location.file, first.location.line),
            });
        }
    }

    for ty in objtree.iter_types() {
        if results.len() >= limit {
            break;
        }

        // Search types
        if (kind == "all" || kind == "type") && ty.path.to_lowercase().contains(&query) {
            let file_path = get_file_path(context, ty.location.file);
            results.push(SymbolSearchRow::Type {
                path: ty.path.to_string(),
                location: format!("{}:{}", file_path, ty.location.line),
            });
        }

        // Search vars
        if kind == "all" || kind == "var" {
            for (name, var) in ty.vars.iter() {
                if results.len() >= limit {
                    break;
                }
                if name.to_lowercase().contains(&query) {
                    let file_path = get_file_path(context, var.value.location.file);
                    results.push(SymbolSearchRow::Var {
                        name: name.to_string(),
                        type_path: ty.path.to_string(),
                        location: format!("{}:{}", file_path, var.value.location.line),
                    });
                }
            }
        }
    }

    let result = SearchSymbolsData {
        count: results.len(),
        results,
    };
    crate::result::analysis_text(&snapshot, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{ToolContent, ToolResult};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct ReleaseWorker(Arc<crate::state::ParseWorkerTestControl>);

    impl Drop for ReleaseWorker {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    async fn wait_for_worker_count(state: &ServerState, started: bool, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let changed = state.parse_worker_test.changed.notified();
                let observed = if started {
                    &state.parse_worker_test.started
                } else {
                    &state.parse_worker_test.active
                };
                if observed.load(Ordering::SeqCst) == count {
                    return;
                }
                changed.await;
            }
        })
        .await
        .expect("worker count should settle");
    }

    #[tokio::test]
    async fn queued_parse_deadline_includes_admission() {
        let (directory, dme_path) = write_environment_fixture();
        let state = Arc::new(ServerState::new());
        state.parse_worker_test.pause();
        let release = ReleaseWorker(state.parse_worker_test.clone());
        let first = tokio::spawn({
            let state = state.clone();
            let dme_path = dme_path.clone();
            async move { parse_environment(&state, json!({"dme_path":dme_path})).await }
        });
        wait_for_worker_count(&state, true, 1).await;
        let queued = tokio::time::timeout(
            Duration::from_millis(250),
            parse_environment(&state, json!({"dme_path":dme_path, "timeout_ms":1})),
        )
        .await;
        let started = state.parse_worker_test.started.load(Ordering::SeqCst);
        drop(release);
        first.await.unwrap().unwrap();
        assert_eq!(started, 1);
        assert_eq!(
            result_json(
                &queued
                    .expect("queue must obey the request deadline")
                    .unwrap()
            )["code"],
            "timed_out"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn dropped_parse_caller_keeps_worker_admission_until_exit() {
        let (directory, dme_path) = write_environment_fixture();
        let state = Arc::new(ServerState::new());
        state.parse_worker_test.pause();
        let release = ReleaseWorker(state.parse_worker_test.clone());
        let first = tokio::spawn({
            let state = state.clone();
            let dme_path = dme_path.clone();
            async move { parse_environment(&state, json!({"dme_path":dme_path})).await }
        });
        wait_for_worker_count(&state, true, 1).await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let next = tokio::time::timeout(
            Duration::from_millis(250),
            parse_environment(&state, json!({"dme_path":dme_path, "timeout_ms":1})),
        )
        .await;
        let maximum = state.parse_worker_test.maximum.load(Ordering::SeqCst);
        let started = state.parse_worker_test.started.load(Ordering::SeqCst);
        drop(release);
        wait_for_worker_count(&state, false, 0).await;
        assert_eq!(state.state_generation().await, 0);
        assert_eq!(maximum, 1, "orphaned worker must retain admission");
        assert_eq!(started, 1);
        assert_eq!(
            result_json(&next.expect("queued request must time out").unwrap())["code"],
            "timed_out"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn reuse_validation_worker_obeys_deadline_and_preserves_snapshot() {
        let (directory, dme_path) = write_environment_fixture();
        settle(&directory);
        let state = Arc::new(ServerState::new());
        parse_environment(&state, json!({"dme_path":dme_path}))
            .await
            .unwrap();
        let original = state.snapshot().await.unwrap();
        state.parse_worker_test.pause();
        let release = ReleaseWorker(state.parse_worker_test.clone());
        let request = tokio::spawn({
            let state = state.clone();
            let dme_path = dme_path.clone();
            async move {
                parse_environment(&state, json!({"dme_path":dme_path, "timeout_ms":100})).await
            }
        });
        wait_for_worker_count(&state, true, 2).await;
        let result = tokio::time::timeout(Duration::from_millis(250), request)
            .await
            .expect("blocking reuse validation must not stall the async timer")
            .unwrap()
            .unwrap();
        assert_eq!(result_json(&result)["code"], "timed_out");
        assert!(Arc::ptr_eq(&original, &state.snapshot().await.unwrap()));
        assert!(
            tokio::time::timeout(Duration::from_millis(1), state.parse_permit())
                .await
                .is_err()
        );
        drop(release);
        wait_for_worker_count(&state, false, 0).await;
        let next = parse_environment(&state, json!({"dme_path":dme_path}))
            .await
            .unwrap();
        assert_eq!(result_json(&next)["reused"], true);
        assert_eq!(result_json(&next)["state_generation"], original.generation);
        assert_eq!(state.parse_worker_test.maximum.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn analysis_writer_cannot_block_initial_read_or_timeout_metadata() {
        let (directory, dme_path) = write_environment_fixture();
        let state = ServerState::new();
        parse_environment(&state, json!({"dme_path":dme_path}))
            .await
            .unwrap();
        let writer = state.hold_analysis_write().await;
        let result = tokio::time::timeout(
            Duration::from_millis(250),
            parse_environment(&state, json!({"dme_path":dme_path, "timeout_ms":1})),
        )
        .await;
        drop(writer);
        let body = result_json(
            &result
                .expect("held writer must not extend the deadline")
                .unwrap(),
        );
        assert_eq!(body["code"], "timed_out");
        assert_eq!(body["details"]["state_generation"], 1);
        assert_eq!(
            body["details"]["active_environment"],
            display_path(&dme_path)
        );
        assert_eq!(state.parse_worker_test.started.load(Ordering::SeqCst), 1);
        state.clear_analysis().await;
        let writer = state.hold_analysis_write().await;
        let result = tokio::time::timeout(
            Duration::from_millis(250),
            parse_environment(&state, json!({"dme_path":dme_path, "timeout_ms":1})),
        )
        .await;
        drop(writer);
        let body = result_json(
            &result
                .expect("cleared metadata must also remain available")
                .unwrap(),
        );
        assert_eq!(body["code"], "timed_out");
        assert_eq!(body["details"]["state_generation"], 1);
        assert!(body["details"]["active_environment"].is_null());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn prepared_worker_result_has_a_one_ms_installation_deadline() {
        let (directory, dme_path) = write_environment_fixture();
        let state = ServerState::new();
        parse_environment(&state, json!({"dme_path":dme_path}))
            .await
            .unwrap();
        let policy = PathPolicy::new(vec![directory.clone()], vec![]).unwrap();
        let parsed = tokio::task::spawn_blocking({
            let dme_path = dme_path.clone();
            move || build_environment(dme_path, policy)
        })
        .await
        .unwrap()
        .unwrap();
        let writer = state.hold_analysis_write().await;
        let result = tokio::time::timeout(Duration::from_millis(250), async {
            let timeout = Duration::from_millis(1);
            let deadline = tokio::time::Instant::now() + timeout;
            assert!(state
                .install_analysis_before_deadline(parsed.snapshot, deadline)
                .await
                .unwrap()
                .is_none());
            parse_timeout(&state, timeout).await
        })
        .await;
        drop(writer);
        let body = result_json(
            &result
                .expect("installation expiry must return while writer stays held")
                .unwrap(),
        );
        assert_eq!(body["code"], "timed_out");
        assert_eq!(body["details"]["state_generation"], 1);
        assert_eq!(
            body["details"]["active_environment"],
            display_path(&dme_path)
        );
        assert_eq!(state.state_generation().await, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn analysis_writer_cannot_block_installation_timeout_response() {
        let (directory, dme_path) = write_environment_fixture();
        let state = Arc::new(ServerState::new());
        parse_environment(&state, json!({"dme_path":dme_path}))
            .await
            .unwrap();
        state.parse_worker_test.pause();
        let release = ReleaseWorker(state.parse_worker_test.clone());
        let request = tokio::spawn({
            let state = state.clone();
            let dme_path = dme_path.clone();
            async move {
                parse_environment(
                    &state,
                    json!({"dme_path":dme_path, "force":true, "timeout_ms":100}),
                )
                .await
            }
        });
        wait_for_worker_count(&state, true, 2).await;
        let writer = state.hold_analysis_write().await;
        drop(release);
        wait_for_worker_count(&state, false, 0).await;
        let mut request = request;
        let result = tokio::time::timeout(Duration::from_millis(250), &mut request).await;
        if result.is_err() {
            request.abort();
        }
        drop(writer);
        let body = result_json(
            &result
                .expect("held writer must not block timeout reporting")
                .unwrap()
                .unwrap(),
        );
        assert_eq!(body["code"], "timed_out");
        assert_eq!(body["details"]["state_generation"], 1);
        assert_eq!(
            body["details"]["active_environment"],
            display_path(&dme_path)
        );
        assert_eq!(state.state_generation().await, 1);
        let permit = tokio::time::timeout(Duration::from_millis(250), state.parse_permit())
            .await
            .unwrap();
        drop(permit);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn result_json(result: &ToolResult) -> Value {
        let ToolContent::Text { text } = &result.content[0];
        serde_json::from_str(text).expect("tool result should be JSON")
    }

    fn write_environment_fixture() -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "meridian-mcp-inspection-{}-{unique}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let dme_path = directory.join("fixture.dme");
        std::fs::write(&dme_path, "#include \"fixture.dm\"\n").unwrap();
        std::fs::write(
            directory.join("fixture.dm"),
            r#"/** Fixture parent documentation. */
/datum/meridian_fixture
	/// Stored fixture values.
	var/list/items = list()

/** Return the supplied value.
 * Arguments:
 * * value - value to return
 */
/datum/meridian_fixture/proc/do_work(value)
	return value

/datum/meridian_fixture/child
"#,
        )
        .unwrap();
        (directory, dme_path)
    }

    /// Push every fixture file's mtime far enough into the past that the
    /// fingerprint's settle window accepts it. Without this, files written
    /// moments ago are deliberately treated as unreusable.
    fn settle(directory: &std::path::Path) {
        let settled = SystemTime::now() - Duration::from_secs(60);
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(settled)
                    .unwrap();
            }
        }
    }

    async fn parsed_fixture() -> (PathBuf, ServerState) {
        let (directory, dme_path) = write_environment_fixture();
        let state = ServerState::new();
        let result = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        assert_eq!(result.is_error, None, "parse result: {result:?}");
        (directory, state)
    }

    #[test]
    fn blocking_error_descriptions_match_upstream_wording() {
        // Verified against the pinned SpacemanDMM revision: the first two are
        // formatted in preprocessor.rs, the last two raised in lexer.rs.
        assert!(is_blocking_error(
            r#"failed to find #include "code/absent.dm""#
        ));
        assert!(is_blocking_error(
            r#"failed to open file: #include "code/absent.dm""#
        ));
        assert!(is_blocking_error("i/o error opening file"));
        assert!(is_blocking_error("i/o error reading file"));
        assert!(is_blocking_error(
            "cyclic parent_type involving /datum/cycle"
        ));

        assert!(!is_blocking_error("expected expression, found ')'"));
        assert!(!is_blocking_error("undefined proc: do_work"));
    }

    #[cfg(windows)]
    #[test]
    fn reported_paths_drop_the_windows_verbatim_prefix() {
        assert_eq!(
            display_path(std::path::Path::new(r"\\?\C:\workspace\tgstation.dme")),
            r"C:\workspace\tgstation.dme"
        );
        assert_eq!(
            display_path(std::path::Path::new(r"\\?\UNC\server\share\tgstation.dme")),
            r"\\server\share\tgstation.dme"
        );
        assert_eq!(
            display_path(std::path::Path::new(r"C:\workspace\tgstation.dme")),
            r"C:\workspace\tgstation.dme"
        );
    }

    #[tokio::test]
    async fn an_unchanged_environment_is_reused_without_reparsing() {
        let (directory, dme_path) = write_environment_fixture();
        settle(&directory);
        let state = ServerState::new();

        let first = parse_environment(&state, json!({"dme_path": dme_path.clone()}))
            .await
            .unwrap();
        assert_eq!(first.is_error, None, "first parse: {first:?}");
        assert_eq!(result_json(&first)["reused"], false);
        let generation = state.state_generation().await;

        let second = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        let payload = result_json(&second);

        assert_eq!(second.is_error, None, "second parse: {second:?}");
        assert_eq!(payload["reused"], true);
        assert_eq!(
            payload["timings_ms"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["queue_wait", "reuse_validation", "total"]
                .into_iter()
                .collect()
        );
        assert_eq!(payload["state_generation"], generation);
        assert_eq!(payload["total_types"], result_json(&first)["total_types"]);
        assert_eq!(state.state_generation().await, generation);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn proc_source_reports_snapshot_generation_and_excerpt_limit() {
        let (directory, dme_path) = write_environment_fixture();
        let source = directory.join("fixture.dm");
        std::fs::write(
            &source,
            format!("/proc/long_excerpt()\n{}", "\treturn 1\n".repeat(100)),
        )
        .unwrap();
        let state = ServerState::new();
        parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        std::fs::remove_file(&source).unwrap();
        let result = get_proc(
            &state,
            crate::parameters::decode(json!({"type_path":"", "proc_name":"long_excerpt"}))
                .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let body = result_json(&result);
        assert_eq!(body["state_generation"], 1);
        let implementation = &body["overrides"][0];
        assert_eq!(implementation["source_origin"], "analysis_snapshot");
        assert_eq!(implementation["source_line_limit"], 80);
        assert_eq!(
            implementation["source"].as_str().unwrap().lines().count(),
            80
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn parse_failure_preserves_the_requested_error_code() {
        let result = parse_failure(
            &ServerState::new(),
            ToolErrorCode::TimedOut,
            "parse exceeded 1 ms".to_owned(),
            Some("Wait for the active parser worker to finish, then retry.".to_owned()),
        )
        .await
        .unwrap();

        assert_eq!(result_json(&result)["code"], "timed_out");
    }

    #[test]
    fn parse_timeout_rejects_values_outside_the_contract() {
        for timeout in [0, 1_800_001] {
            assert!(
                crate::parameters::decode::<crate::parameters::ParseEnvironmentParams>(
                    json!({"dme_path":"fixture.dme","timeout_ms":timeout})
                )
                .is_err()
            );
        }
        let request = crate::parameters::decode::<crate::parameters::ParseEnvironmentParams>(
            json!({"dme_path":"fixture.dme","timeout_ms":1_800_000}),
        )
        .unwrap();
        assert_eq!(request.timeout_ms, Some(1_800_000));
    }

    #[tokio::test]
    async fn an_edited_source_file_forces_a_reparse() {
        let (directory, dme_path) = write_environment_fixture();
        settle(&directory);
        let state = ServerState::new();
        parse_environment(&state, json!({"dme_path": dme_path.clone()}))
            .await
            .unwrap();
        let generation = state.state_generation().await;

        std::fs::write(
            directory.join("fixture.dm"),
            "/datum/meridian_fixture\n\n/datum/meridian_fixture/successor\n",
        )
        .unwrap();
        settle(&directory);

        let reparsed = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        let payload = result_json(&reparsed);

        assert_eq!(reparsed.is_error, None, "reparse: {reparsed:?}");
        assert_eq!(payload["reused"], false);
        assert_eq!(payload["state_generation"], generation + 1);
        assert!(state
            .snapshot()
            .await
            .unwrap()
            .objtree
            .find("/datum/meridian_fixture/successor")
            .is_some());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn editing_a_comment_only_include_forces_a_reparse() {
        let (directory, dme_path) = write_environment_fixture();
        let comment_only = directory.join("comment_only.dm");
        std::fs::write(
            &dme_path,
            "#include \"fixture.dm\"\n#include \"comment_only.dm\"\n",
        )
        .unwrap();
        std::fs::write(&comment_only, "// first revision\n").unwrap();
        settle(&directory);
        let state = ServerState::new();

        parse_environment(&state, json!({"dme_path": dme_path.clone()}))
            .await
            .unwrap();
        let generation = state.state_generation().await;
        assert!(state
            .snapshot()
            .await
            .unwrap()
            .source_inputs()
            .contains(&comment_only.canonicalize().unwrap()));

        std::fs::write(&comment_only, "// second revision\n").unwrap();
        settle(&directory);
        let reparsed = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        let body = result_json(&reparsed);

        assert_eq!(body["reused"], false);
        assert_eq!(body["state_generation"], generation + 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn force_reparses_an_unchanged_environment() {
        let (directory, dme_path) = write_environment_fixture();
        settle(&directory);
        let state = ServerState::new();
        parse_environment(&state, json!({"dme_path": dme_path.clone()}))
            .await
            .unwrap();
        let generation = state.state_generation().await;

        let forced = parse_environment(&state, json!({"dme_path": dme_path, "force": true}))
            .await
            .unwrap();
        let payload = result_json(&forced);

        assert_eq!(payload["reused"], false);
        assert_eq!(payload["state_generation"], generation + 1);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_successful_parse_reports_diagnostic_counts_and_duration() {
        let (directory, dme_path) = write_environment_fixture();
        settle(&directory);
        let state = ServerState::new();

        let result = parse_environment(&state, json!({"dme_path": dme_path.clone()}))
            .await
            .unwrap();
        let payload = result_json(&result);

        assert!(payload["error_count"].is_u64());
        assert!(payload["warning_count"].is_u64());
        assert!(payload["duration_ms"].is_u64());
        assert_eq!(payload["retrieval"]["lexical"]["status"], "ready");
        assert_eq!(payload["retrieval"]["lexical"]["algorithm"], "bm25");
        assert_eq!(payload["retrieval"]["dense"]["status"], "not_configured");
        assert_eq!(payload["retrieval"]["semantic_chunk_schema_version"], 1);
        for stage in [
            "queue_wait",
            "preprocess_parse",
            "dreamchecker",
            "search_documents",
            "analysis_indexes",
            "fingerprint",
            "total",
        ] {
            assert!(payload["timings_ms"][stage].is_u64(), "missing {stage}");
        }
        assert!(!payload["spacemandmm_revision"].as_str().unwrap().is_empty());
        assert_eq!(
            payload["environment"].as_str().unwrap(),
            dme_path.display().to_string()
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_directory_is_rejected_before_parsing() {
        let (directory, _) = write_environment_fixture();
        let result = parse_environment(
            &ServerState::new(),
            json!({"dme_path": directory.display().to_string()}),
        )
        .await
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        let payload = result_json(&result);
        assert!(
            payload["message"]
                .as_str()
                .is_some_and(|message| message.contains("Not a file")),
            "payload: {payload}"
        );
        assert_eq!(payload["details"]["state_preserved"], true);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn type_inspection_returns_docs_and_direct_children() {
        let (directory, state) = parsed_fixture().await;
        let result = get_type(
            &state,
            crate::parameters::decode(json!({"type_path": "/datum/meridian_fixture"}))
                .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let payload = result_json(&result);

        assert!(payload["documentation"]
            .as_str()
            .unwrap()
            .contains("Fixture parent documentation"));
        assert_eq!(
            payload["children"],
            json!(["/datum/meridian_fixture/child"])
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn proc_inspection_reports_docs_and_a_parsed_body() {
        let (directory, state) = parsed_fixture().await;
        let result = get_proc(
            &state,
            crate::parameters::decode(json!({
                "type_path": "/datum/meridian_fixture",
                "proc_name": "do_work"
            }))
            .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let payload = result_json(&result);

        assert_eq!(payload["overrides"][0]["has_body"], true);
        assert!(payload["overrides"][0]["documentation"]
            .as_str()
            .unwrap()
            .contains("Return the supplied value"));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn proc_inspection_resolves_inherited_procs() {
        let (directory, state) = parsed_fixture().await;
        let result = get_proc(
            &state,
            crate::parameters::decode(json!({
                "type_path": "/datum/meridian_fixture/child",
                "proc_name": "do_work"
            }))
            .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let payload = result_json(&result);

        assert_eq!(result.is_error, None);
        assert_eq!(payload["type_path"], "/datum/meridian_fixture/child");
        assert_eq!(payload["declared"], false);
        assert_eq!(payload["overrides"][0]["has_body"], true);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn variable_inspection_returns_declared_type_and_docs() {
        let (directory, state) = parsed_fixture().await;
        let result = get_var(
            &state,
            crate::parameters::decode(json!({
                "type_path": "/datum/meridian_fixture",
                "var_name": "items"
            }))
            .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let payload = result_json(&result);

        assert!(payload["declared_type"].as_str().unwrap().contains("list"));
        assert!(payload["documentation"]
            .as_str()
            .unwrap()
            .contains("Stored fixture values"));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn type_listing_is_paginated_with_stable_cursors() {
        let (directory, state) = parsed_fixture().await;
        let first = list_types(
            &state,
            crate::parameters::decode(json!({"prefix": "/datum/meridian_fixture", "limit": 1}))
                .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let first_payload = result_json(&first);

        assert_eq!(first_payload["count"], 1);
        assert_eq!(first_payload["total_count"], 2);
        assert_eq!(first_payload["pagination"]["next_cursor"], "1");
        assert_eq!(first_payload["truncated"], true);

        let second = list_types(
            &state,
            crate::parameters::decode(json!({
                "prefix": "/datum/meridian_fixture",
                "limit": 1,
                "cursor": "1"
            }))
            .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let second_payload = result_json(&second);

        assert_eq!(second_payload["count"], 1);
        assert_eq!(second_payload["pagination"]["has_more"], false);
        assert!(second_payload["pagination"]["next_cursor"].is_null());
        assert_eq!(second_payload["truncated"], false);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn symbol_search_rejects_unbounded_limits() {
        let (directory, _state) = parsed_fixture().await;
        let error = crate::parameters::decode::<crate::parameters::SearchSymbolsParams>(
            json!({"query":"meridian_fixture","limit":201}),
        )
        .unwrap_err();
        assert_eq!(error.field, "limit");

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn failed_reparse_preserves_the_active_project_profile() {
        let (directory, dme_path) = write_environment_fixture();
        let state = ServerState::new();
        let first = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        assert_eq!(first.is_error, None);
        let generation = state.state_generation().await;
        let active_dme = state
            .snapshot()
            .await
            .unwrap()
            .project_profile
            .as_ref()
            .expect("successful parse should discover a profile")
            .dme_path()
            .to_owned();

        let missing_dme = directory.join("missing.dme");
        let failed = parse_environment(&state, json!({"dme_path": missing_dme}))
            .await
            .unwrap();

        assert_eq!(failed.is_error, Some(true));
        assert_eq!(state.state_generation().await, generation);
        assert_eq!(
            state
                .snapshot()
                .await
                .unwrap()
                .project_profile
                .as_ref()
                .expect("failed parse should preserve the prior profile")
                .dme_path(),
            active_dme
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn nonfatal_parser_errors_preserve_the_active_snapshot() {
        let (directory, dme_path) = write_environment_fixture();
        let state = ServerState::new();
        let first = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        assert_eq!(first.is_error, None);
        let generation = state.state_generation().await;

        let invalid_dme = directory.join("invalid.dme");
        std::fs::write(&invalid_dme, "#include \"missing.dm\"\n").unwrap();
        let failed = parse_environment(&state, json!({"dme_path": invalid_dme}))
            .await
            .unwrap();

        assert_eq!(failed.is_error, Some(true), "parse result: {failed:?}");
        assert_eq!(state.state_generation().await, generation);
        let preserved = get_type(
            &state,
            crate::parameters::decode(json!({"type_path": "/datum/meridian_fixture"}))
                .expect("valid fixture request"),
        )
        .await
        .unwrap();
        assert_eq!(preserved.is_error, None, "lookup result: {preserved:?}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
