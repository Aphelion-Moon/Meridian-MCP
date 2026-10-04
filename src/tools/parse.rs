use anyhow::{anyhow, Result};
use serde_json::{json, Value};
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
const MAX_PARSE_TIMEOUT_MS: u64 = 1_800_000;
const DEFAULT_TYPE_LIMIT: u64 = 100;
const MAX_TYPE_LIMIT: u64 = 500;
const DEFAULT_SYMBOL_LIMIT: u64 = 50;
const MAX_SYMBOL_LIMIT: u64 = 200;

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

fn validated_parse_timeout(args: &Value) -> Result<Duration> {
    let timeout_ms = args
        .get("timeout_ms")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow!("timeout_ms must be an integer"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_PARSE_TIMEOUT_MS);
    if !(1..=MAX_PARSE_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(anyhow!(
            "timeout_ms must be between 1 and {MAX_PARSE_TIMEOUT_MS}"
        ));
    }
    Ok(Duration::from_millis(timeout_ms))
}

/// Parse a DreamMaker environment
#[cfg(test)]
pub async fn parse_environment(state: &ServerState, args: Value) -> Result<ToolResult> {
    let path = args
        .get("dme_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing dme_path argument"))?;
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

pub(crate) async fn parse_environment_with_policy(
    state: &ServerState,
    args: Value,
    policy: &PathPolicy,
) -> Result<ToolResult> {
    let request_started = Instant::now();
    let dme_path = args
        .get("dme_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing dme_path argument"))?;
    let timeout = match validated_parse_timeout(&args) {
        Ok(timeout) => timeout,
        Err(error) => {
            return parse_failure(
                state,
                ToolErrorCode::InvalidInput,
                error.to_string(),
                Some("Use timeout_ms between 1 and 1800000 milliseconds.".to_owned()),
            )
            .await
        }
    };
    let deadline = tokio::time::Instant::from_std(request_started + timeout);
    let path = PathBuf::from(dme_path);
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);

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
    let policy = policy.clone();
    // Reuse validation includes blocking-pool scheduling; full build stage
    // timings measure work inside the worker and exclude that scheduling delay.
    let reuse_started = Instant::now();
    #[cfg(test)]
    let worker_test = state.parse_worker_test.clone();
    // Capture admission before spawning. Dropping any caller future or join
    // handle cannot release it while this non-abortable job is queued/running.
    // Returning it with the result also covers the snapshot-installation await.
    let handle = tokio::task::spawn_blocking(move || {
        let outcome = (|| -> Result<ParseOutcome> {
            #[cfg(test)]
            let _worker_test = worker_test.enter();
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
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
                    return Ok(ParseOutcome::Reused(reused));
                }
            } else {
                drop(candidate);
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "parse deadline expired"
            );
            info!("Parsing environment: {}", path.display());
            build_environment(path, policy).map(|build| ParseOutcome::Built(Box::new(build)))
        })();
        (outcome, permit)
    });
    let (outcome, _permit) = match tokio::time::timeout_at(deadline, handle).await {
        Ok(Ok(result)) if tokio::time::Instant::now() < deadline => result,
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
            json!({
                "queue_wait": queue_wait,
                "reuse_validation": reuse_started.elapsed().as_millis() as u64,
            }),
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
                .install_analysis_before_deadline(snapshot, deadline)
                .await
            else {
                return parse_timeout(state, timeout).await;
            };
            (
                snapshot,
                false,
                json!({
                    "queue_wait": queue_wait,
                    "preprocess_parse": preprocess_parse,
                    "dreamchecker": dreamchecker,
                    "search_documents": search_documents_ms,
                    "analysis_indexes": build_timings.analysis_indexes,
                    "fingerprint": build_timings.fingerprint,
                }),
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
    timings["total"] = json!(total);
    let (errors, warnings) = snapshot.diagnostic_counts();
    let mut result = json!({
        "success": true,
        "reused": reused,
        "environment": display_path(&snapshot.environment_path),
        "total_types": snapshot.total_types,
        "indexed_symbols": snapshot.indexed_symbol_count(),
        "error_count": errors,
        "warning_count": warnings,
        "state_generation": snapshot.generation,
        "spacemandmm_revision": snapshot.spacemandmm_revision,
        "spacemandmm_local_patch": crate::capabilities::SPACEMANDMM_LOCAL_PATCH,
        "spacemandmm_local_patch_sha256": crate::capabilities::SPACEMANDMM_LOCAL_PATCH_SHA256,
        "retrieval": {
            "lexical": {"status":"ready", "algorithm":"bm25", "documents":snapshot.indexed_symbol_count()},
            "dense": {"status":"not_configured"},
            "semantic_chunk_schema_version": SEMANTIC_CHUNK_SCHEMA_VERSION,
        },
        "timings_ms": timings,
    });
    if !reused {
        result["duration_ms"] = json!(total);
    }
    Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
}

async fn parse_timeout(state: &ServerState, timeout: Duration) -> Result<ToolResult> {
    parse_failure(state, ToolErrorCode::TimedOut,
        format!("parse request exceeded its total {} ms budget; any unfinished worker retains admission until it exits", timeout.as_millis()),
        Some("Wait for the active parser worker to finish, then retry.".to_owned())).await
}

fn build_environment(parse_path: PathBuf, policy: PathPolicy) -> Result<ParsedEnvironment> {
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

    // Always run DreamChecker so semantic diagnostics describe this parsed
    // snapshot. Its cost is reported separately in the parse response.
    let dreamchecker_started = Instant::now();
    dreamchecker::run(&context, &objtree);
    let configured_rules = configured_diagnostic_rules(&parse_path, &context);
    let diagnostics = collect_diagnostics(&context, &configured_rules);
    let dreamchecker = dreamchecker_started.elapsed().as_millis() as u64;
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
    Ok(structured_error(
        code,
        error,
        recovery,
        json!({
        "state_preserved": true,
        "active_environment": metadata.active_environment.as_deref().map(display_path),
        "state_generation": metadata.generation
        }),
    ))
}

/// Helper to get file path string from a location
fn get_file_path(context: &AnalysisContext, file_id: dreammaker::FileId) -> String {
    context.file_path(file_id).display().to_string()
}

/// Get type information
pub async fn get_type(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;

    let type_path = args
        .get("type_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing type_path argument"))?;

    match objtree.find(type_path) {
        Some(ty) => {
            // Collect variables
            let vars: Vec<Value> = ty
                .vars
                .iter()
                .map(|(name, var)| {
                    json!({
                        "name": name.to_string(),
                        "has_value": var.value.expression.is_some(),
                        "constant": var.value.constant.as_ref().map(|c| format!("{c:?}")),
                        "declared_here": var.declaration.is_some()
                    })
                })
                .collect();

            // Collect procs
            let procs: Vec<Value> = ty
                .procs
                .iter()
                .map(|(name, proc)| {
                    let param_count = proc.value.first().map(|v| v.parameters.len()).unwrap_or(0);
                    json!({
                        "name": name.to_string(),
                        "parameter_count": param_count,
                        "override_count": proc.value.len(),
                        "declared_here": proc.declaration.is_some()
                    })
                })
                .collect();

            // Get parent
            let parent = ty.parent_type().map(|p| p.path.to_string());
            let children: Vec<_> = ty.children().map(|child| child.path.to_string()).collect();

            let file_path = get_file_path(context, ty.location.file);

            let result = json!({
                "path": ty.path,
                "parent": parent,
                "children": children,
                "documentation": ty.docs.text(),
                "vars": vars,
                "procs": procs,
                "location": format!("{}:{}:{}",
                    file_path,
                    ty.location.line,
                    ty.location.column
                )
            });

            Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
        }
        None => Ok(ToolResult::error(format!("Type not found: {type_path}"))),
    }
}

/// Get proc information
pub async fn get_proc(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let (include_source, max_source_lines) =
        super::search::source_options(&args, snapshot.search_index.source_line_limit())?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;

    let type_path = args
        .get("type_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing type_path argument"))?;

    let proc_name = args
        .get("proc_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing proc_name argument"))?;

    let resolution = match snapshot.proc_resolver().resolve(type_path, proc_name) {
        Ok(resolution) => resolution,
        Err(error) => {
            return Ok(structured_error(
                ToolErrorCode::NotFound,
                error.to_string(),
                Some("Check the exact type and proc names in the active analysis snapshot.".into()),
                serde_json::to_value(error)?,
            ));
        }
    };
    let mut values = Vec::with_capacity(resolution.implementations.len());
    for implementation in &resolution.implementations {
        let owner = objtree
            .find(&implementation.owner)
            .expect("resolved proc owner should remain in the object tree");
        let proc_ref = owner
            .iter_self_procs()
            .find(|proc_ref| {
                proc_ref.name() == proc_name && proc_ref.index() == implementation.override_index
            })
            .expect("resolved proc implementation should remain in the object tree");
        let value = proc_ref.get();
        let parameters = value
            .parameters
            .iter()
            .map(|parameter| {
                json!({
                    "name": parameter.name.to_string(),
                    "has_default": parameter.default.is_some(),
                })
            })
            .collect::<Vec<_>>();
        let file_path = get_file_path(context, value.location.file);
        let source = if value.location == dreammaker::Location::BUILTINS {
            None
        } else {
            snapshot.search_index.proc_source(
                &implementation.owner,
                proc_name,
                implementation.override_index,
            )
        };
        let mut detail = json!({
            "owner": implementation.owner,
            "override_index": implementation.override_index,
            "parameters": parameters,
            "documentation": value.docs.text(),
            "location": format!("{}:{}:{}", file_path, value.location.line, value.location.column),
            "has_body": implementation.has_body,
        });
        if include_source {
            let fields = detail
                .as_object_mut()
                .expect("procedure detail is an object");
            fields.insert("source_origin".into(), json!("analysis_snapshot"));
            fields.insert("source_line_limit".into(), json!(max_source_lines));
            super::search::add_source_fields(fields, source, max_source_lines);
        }
        values.push(detail);
    }

    let result = json!({
        "name": proc_name,
        "type_path": type_path,
        "requested_type_path": resolution.requested_type_path,
        "implementation_owner": resolution.implementation_owner,
        "declaration_owner": resolution.declaration_owner,
        "resolution_kind": resolution.resolution_kind,
        "declared": resolution.implementation_owner == type_path,
        "overrides": values,
        "resolution_diagnostics": resolution.diagnostics(),
        "state_generation": snapshot.generation,
    });
    Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
}

/// Get variable information
pub async fn get_var(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;

    let type_path = args
        .get("type_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing type_path argument"))?;

    let var_name = args
        .get("var_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing var_name argument"))?;

    match objtree.find(type_path) {
        Some(ty) => match ty.get_value(var_name) {
            Some(value) => {
                let value_owner = ty
                    .iter_parent_types()
                    .find(|owner| owner.vars.contains_key(var_name))
                    .expect("resolved variable must have a value owner");
                let declaration = ty.get_var_declaration(var_name);
                let declaration_owner = ty.iter_parent_types().find(|owner| {
                    owner
                        .vars
                        .get(var_name)
                        .is_some_and(|var| var.declaration.is_some())
                });
                let file_path = get_file_path(context, value.location.file);
                // An override often changes only the value; retain declaration docs.
                let documentation = if value.docs.is_empty() {
                    declaration_owner
                        .and_then(|owner| owner.vars.get(var_name).map(|var| var.value.docs.text()))
                        .unwrap_or_default()
                } else {
                    value.docs.text()
                };

                let result = json!({
                    "name": var_name,
                    "type_path": type_path,
                    "declared": ty.vars.get(var_name).is_some_and(|var| var.declaration.is_some()),
                    "declared_type": declaration.map(|declaration| declaration.var_type.to_string()),
                    "value_owner": value_owner.path,
                    "declaration_owner": declaration_owner.map(|owner| owner.path.clone()),
                    "inherited": value_owner.path != ty.path,
                    "documentation": documentation,
                    "constant": value.constant.as_ref().map(|c| format!("{c:?}")),
                    "has_expression": value.expression.is_some(),
                    "location": format!("{}:{}:{}",
                        file_path,
                        value.location.line,
                        value.location.column
                    ),
                    "declaration_location": declaration.map(|declaration| format!("{}:{}:{}",
                        get_file_path(context, declaration.location.file),
                        declaration.location.line, declaration.location.column)),
                    "state_generation": snapshot.generation,
                });

                Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
            }
            None => Ok(ToolResult::error(format!(
                "Variable not found: {type_path}/{var_name}"
            ))),
        },
        None => Ok(ToolResult::error(format!("Type not found: {type_path}"))),
    }
}

/// List types in the object tree
pub async fn list_types(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;

    let prefix = args.get("prefix").and_then(|v| v.as_str()).unwrap_or("");

    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .map(|d| d as usize);
    let limit = super::bounded_u64(&args, "limit", DEFAULT_TYPE_LIMIT, 1, MAX_TYPE_LIMIT)? as usize;
    let cursor = match args.get("cursor") {
        Some(Value::String(cursor)) => cursor
            .parse::<usize>()
            .map_err(|_| anyhow!("cursor must be a non-negative integer string"))?,
        Some(_) => return Err(anyhow!("cursor must be a non-negative integer string")),
        None => 0,
    };

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
    let types: Vec<Value> = matching_types
        .into_iter()
        .skip(cursor)
        .take(limit)
        .map(|ty| {
            json!({
                "path": ty.path.to_string(),
                "var_count": ty.vars.len(),
                "proc_count": ty.procs.len()
            })
        })
        .collect();
    let next_offset = cursor.saturating_add(types.len());
    let has_more = next_offset < total_count;
    let next_cursor = has_more.then(|| next_offset.to_string());

    let result = json!({
        "count": types.len(),
        "total_count": total_count,
        "types": types,
        "pagination": {
            "cursor": cursor.to_string(),
            "limit": limit,
            "next_cursor": next_cursor,
            "has_more": has_more,
        }
    });
    let mut metadata = ToolMetadata::complete(Some(snapshot.generation));
    metadata.truncated = has_more;
    if has_more {
        metadata
            .truncation_reasons
            .push("result_page_limit".to_owned());
    }

    Ok(json_success(metadata, result))
}

/// Search for symbols
pub async fn search_symbols(state: &ServerState, args: Value) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;

    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing query argument"))?
        .to_lowercase();

    let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("all");
    if !matches!(kind, "type" | "proc" | "var" | "macro" | "all") {
        return Err(anyhow!("kind must be one of: type, proc, var, macro, all"));
    }

    let limit =
        super::bounded_u64(&args, "limit", DEFAULT_SYMBOL_LIMIT, 1, MAX_SYMBOL_LIMIT)? as usize;

    let mut results: Vec<Value> = Vec::new();

    if kind == "all" || kind == "macro" {
        for symbol in snapshot.language_index.macros() {
            if results.len() >= limit {
                break;
            }
            if symbol.name.to_lowercase().contains(&query) {
                results.push(json!({"kind":"macro","name":symbol.name,"location":format!("{}:{}", symbol.file, symbol.line),"file":symbol.file,"line":symbol.line,"column":symbol.column}));
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
            results.push(json!({
                "kind": "proc",
                "name": resolution.proc_name,
                "type_path": resolution.implementation_owner,
                "implementation_owner": resolution.implementation_owner,
                "declaration_owner": resolution.declaration_owner,
                "resolution_kind": resolution.resolution_kind,
                "location": format!("{}:{}", first.location.file, first.location.line),
            }));
        }
    }

    for ty in objtree.iter_types() {
        if results.len() >= limit {
            break;
        }

        // Search types
        if (kind == "all" || kind == "type") && ty.path.to_lowercase().contains(&query) {
            let file_path = get_file_path(context, ty.location.file);
            results.push(json!({
                "kind": "type",
                "path": ty.path.to_string(),
                "location": format!("{}:{}",
                    file_path,
                    ty.location.line
                )
            }));
        }

        // Search vars
        if kind == "all" || kind == "var" {
            for (name, var) in ty.vars.iter() {
                if results.len() >= limit {
                    break;
                }
                if name.to_lowercase().contains(&query) {
                    let file_path = get_file_path(context, var.value.location.file);
                    results.push(json!({
                        "kind": "var",
                        "name": name.to_string(),
                        "type_path": ty.path.to_string(),
                        "location": format!("{}:{}",
                            file_path,
                            var.value.location.line
                        )
                    }));
                }
            }
        }
    }

    let result = json!({
        "count": results.len(),
        "results": results
    });

    Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
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
        let result = get_proc(&state, json!({"type_path":"", "proc_name":"long_excerpt"}))
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
        assert!(validated_parse_timeout(&json!({"timeout_ms": 0})).is_err());
        assert!(validated_parse_timeout(&json!({"timeout_ms": 1_800_001})).is_err());
        assert_eq!(
            validated_parse_timeout(&json!({"timeout_ms": 1_800_000})).unwrap(),
            Duration::from_millis(1_800_000),
        );
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
        let result = get_type(&state, json!({"type_path": "/datum/meridian_fixture"}))
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
            json!({
                "type_path": "/datum/meridian_fixture",
                "proc_name": "do_work"
            }),
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
            json!({
                "type_path": "/datum/meridian_fixture/child",
                "proc_name": "do_work"
            }),
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
            json!({
                "type_path": "/datum/meridian_fixture",
                "var_name": "items"
            }),
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
            json!({"prefix": "/datum/meridian_fixture", "limit": 1}),
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
            json!({
                "prefix": "/datum/meridian_fixture",
                "limit": 1,
                "cursor": "1"
            }),
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
        let (directory, state) = parsed_fixture().await;
        let error = search_symbols(&state, json!({"query": "meridian_fixture", "limit": 201}))
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("limit must be between 1 and 200"));

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
        let preserved = get_type(&state, json!({"type_path": "/datum/meridian_fixture"}))
            .await
            .unwrap();
        assert_eq!(preserved.is_error, None, "lookup result: {preserved:?}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
