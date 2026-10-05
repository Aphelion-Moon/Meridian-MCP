use crate::capabilities::SPACEMANDMM_REVISION;
use crate::index::LanguageIndex;
use crate::proc_resolution::ProcResolver;
use crate::search::{SearchDocuments, SearchIndex};
use crate::source_fingerprint::SourceFingerprint;
use crate::spaceman::dmi::{IconReference, IconReferenceResolution};
use crate::spaceman::language::ReferenceTable;
use crate::ProjectProfile;
use dreammaker::config::Config;
use dreammaker::constants::{ConstFn, Constant};
use dreammaker::objtree::ObjectTree;
use dreammaker::preprocessor::DefineHistory;
use dreammaker::{Context, FileId};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct DiagnosticRecord {
    pub rule: Option<String>,
    pub severity: String,
    pub component: String,
    pub message: String,
    pub file: String,
    pub line: u32,
    pub column: u16,
    pub notes: Vec<DiagnosticNoteRecord>,
    pub configured: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct DiagnosticNoteRecord {
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct MacroDefinitionRecord {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub column: u16,
    pub parameters: Vec<String>,
    pub variadic: bool,
    pub definition: String,
}

#[derive(Clone, Debug)]
pub struct AnalysisContext {
    pub path_policy: crate::PathPolicy,
    pub config: Config,
    file_paths: HashMap<FileId, PathBuf>,
}

impl AnalysisContext {
    pub fn file_path(&self, file: FileId) -> &Path {
        self.file_paths
            .get(&file)
            .map(PathBuf::as_path)
            .unwrap_or_else(|| Path::new("(unknown)"))
    }

    fn extract(context: &Context, environment_path: &Path, path_policy: crate::PathPolicy) -> Self {
        let project_root = environment_path.parent().unwrap_or_else(|| Path::new("."));
        let mut file_paths = HashMap::new();
        context.file_list().for_each(|reported| {
            let Some(file_id) = context.get_file(reported) else {
                return;
            };
            let resolved = if reported.is_absolute() {
                reported.to_path_buf()
            } else {
                project_root.join(reported)
            };
            file_paths.insert(file_id, resolved);
        });

        Self {
            path_policy,
            config: context.config().clone(),
            file_paths,
        }
    }
}

pub struct AnalysisBuild {
    pub environment_path: PathBuf,
    pub context: AnalysisContext,
    pub objtree: ObjectTree,
    pub macro_definitions: Vec<MacroDefinitionRecord>,
    pub(crate) search_index: SearchIndex,
    pub diagnostics: Vec<DiagnosticRecord>,
    pub project_profile: Option<ProjectProfile>,
    pub language_index: LanguageIndex,
    pub reference_table: ReferenceTable,
    pub icon_references: Vec<IconReference>,
    pub proc_resolver: ProcResolver,
    pub source_inputs: Vec<PathBuf>,
    pub source_fingerprint: SourceFingerprint,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, schemars::JsonSchema)]
pub struct AnalysisBuildTimings {
    pub analysis_indexes: u64,
    pub fingerprint: u64,
}

impl AnalysisBuild {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parse(
        environment_path: PathBuf,
        context: &Context,
        objtree: ObjectTree,
        defines: DefineHistory,
        search_documents: SearchDocuments,
        diagnostics: Vec<DiagnosticRecord>,
        project_profile: Option<ProjectProfile>,
        parse_started_at: SystemTime,
        path_policy: crate::PathPolicy,
    ) -> (Self, AnalysisBuildTimings) {
        let indexes_started = Instant::now();
        let extracted_context = AnalysisContext::extract(context, &environment_path, path_policy);
        let project_root = environment_path.parent().unwrap_or_else(|| Path::new("."));
        let macro_definitions: Vec<MacroDefinitionRecord> = defines
            .iter()
            .map(|(range, (name, define))| MacroDefinitionRecord {
                name: name.to_string(),
                file: {
                    let reported = context.file_path(range.start.file);
                    if reported.is_absolute() {
                        reported.to_path_buf()
                    } else {
                        project_root.join(&*reported)
                    }
                    .display()
                    .to_string()
                },
                line: range.start.line,
                column: range.start.column,
                parameters: define.params.iter().map(ToString::to_string).collect(),
                variadic: define.variadic,
                definition: define.display_with_name(name).to_string(),
            })
            .collect();
        // These passes each walk the finished object tree and share nothing
        // mutable, so they run concurrently. The only ordering constraint is
        // that the search index and language index both consume the resolver.
        let (proc_resolver, (reference_table, (icon_references, source_inputs))) = rayon::join(
            || ProcResolver::build(&extracted_context, &objtree),
            || {
                rayon::join(
                    || ReferenceTable::build(&objtree),
                    || {
                        rayon::join(
                            || {
                                build_icon_references(
                                    &extracted_context,
                                    &objtree,
                                    &environment_path,
                                )
                            },
                            || {
                                build_source_inputs(
                                    &extracted_context,
                                    &environment_path,
                                    project_profile.as_ref(),
                                )
                            },
                        )
                    },
                )
            },
        );
        let (search_index, language_index) = rayon::join(
            || {
                search_documents
                    .canonicalize_procs(&proc_resolver)
                    .into_index()
            },
            || {
                LanguageIndex::build(
                    &extracted_context,
                    &objtree,
                    &macro_definitions,
                    &proc_resolver,
                )
            },
        );
        let analysis_indexes = indexes_started.elapsed().as_millis() as u64;
        let fingerprint_started = Instant::now();
        let config_candidate = project_profile
            .as_ref()
            .map(ProjectProfile::spaceman_config_candidate)
            .unwrap_or_else(|| project_root.join("SpacemanDMM.toml"));
        let source_fingerprint = SourceFingerprint::capture_with_discovery(
            &source_inputs,
            &[config_candidate],
            parse_started_at,
        );
        let fingerprint = fingerprint_started.elapsed().as_millis() as u64;
        (
            Self {
                environment_path,
                context: extracted_context,
                objtree,
                macro_definitions,
                search_index,
                diagnostics,
                project_profile,
                language_index,
                reference_table,
                icon_references,
                proc_resolver,
                source_inputs,
                source_fingerprint,
            },
            AnalysisBuildTimings {
                analysis_indexes,
                fingerprint,
            },
        )
    }
}

pub struct AnalysisSnapshot {
    pub snapshot_id: String,
    /// Distinguishes snapshots even across separate ServerState instances.
    pub(crate) instance_id: u64,
    pub environment_path: PathBuf,
    pub context: Arc<AnalysisContext>,
    pub objtree: Arc<ObjectTree>,
    pub macro_definitions: Arc<[MacroDefinitionRecord]>,
    pub(crate) search_index: Arc<SearchIndex>,
    pub diagnostics: Arc<[DiagnosticRecord]>,
    pub project_profile: Option<ProjectProfile>,
    pub language_index: Arc<LanguageIndex>,
    pub reference_table: Arc<ReferenceTable>,
    pub icon_references: Arc<[IconReference]>,
    pub proc_resolver: Arc<ProcResolver>,
    pub source_inputs: Arc<[PathBuf]>,
    pub source_fingerprint: Arc<SourceFingerprint>,
    pub total_types: usize,
    pub generation: u64,
    pub spacemandmm_revision: &'static str,
}

impl AnalysisSnapshot {
    pub(crate) fn from_build(build: AnalysisBuild, generation: u64) -> Self {
        static NEXT_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let total_types = build.objtree.iter_types().count();
        Self {
            snapshot_id: String::new(),
            instance_id: NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            environment_path: build.environment_path,
            context: Arc::new(build.context),
            objtree: Arc::new(build.objtree),
            macro_definitions: Arc::from(build.macro_definitions),
            search_index: Arc::new(build.search_index),
            diagnostics: Arc::from(build.diagnostics),
            project_profile: build.project_profile,
            language_index: Arc::new(build.language_index),
            reference_table: Arc::new(build.reference_table),
            icon_references: Arc::from(build.icon_references),
            proc_resolver: Arc::new(build.proc_resolver),
            source_inputs: Arc::from(build.source_inputs),
            source_fingerprint: Arc::new(build.source_fingerprint),
            total_types,
            generation,
            spacemandmm_revision: SPACEMANDMM_REVISION,
        }
    }

    pub fn proc_resolver(&self) -> &ProcResolver {
        &self.proc_resolver
    }

    pub fn identity(&self) -> crate::identity::AnalysisIdentity {
        crate::identity::AnalysisIdentity {
            snapshot_id: self.snapshot_id.clone(),
            generation: self.generation,
            state_generation: self.generation,
            environment: self.environment_path.clone(),
            source: "cached_snapshot",
            completeness: "complete",
            disk_state: "unknown",
            recomputed: false,
            refresh_with: "dm_parse_environment",
        }
    }

    pub fn source_inputs(&self) -> &[PathBuf] {
        &self.source_inputs
    }

    pub fn indexed_symbol_count(&self) -> usize {
        self.search_index.len()
    }

    /// Counts of the diagnostics recorded when this snapshot was built.
    pub fn diagnostic_counts(&self) -> (usize, usize) {
        let errors = self
            .diagnostics
            .iter()
            .filter(|record| record.severity == "error")
            .count();
        let warnings = self
            .diagnostics
            .iter()
            .filter(|record| record.severity == "warning")
            .count();
        (errors, warnings)
    }
}

fn build_source_inputs(
    context: &AnalysisContext,
    environment_path: &Path,
    profile: Option<&ProjectProfile>,
) -> Vec<PathBuf> {
    let mut inputs = context.file_paths.values().cloned().collect::<Vec<_>>();
    inputs.push(environment_path.to_owned());
    if let Some(config) = profile.and_then(ProjectProfile::spaceman_config) {
        inputs.push(config.to_owned());
    }
    // Actual parser opens were authorized by the context's read policy. Keep
    // requested paths, including failed inputs and aliases, so disappearance or
    // alias retargeting cannot silently remove a dependency from reuse checks.
    let resolved = inputs
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect::<Vec<_>>();
    inputs.extend(resolved);
    inputs.sort();
    inputs.dedup();
    inputs
}

fn build_icon_references(
    context: &AnalysisContext,
    objtree: &ObjectTree,
    environment_path: &Path,
) -> Vec<IconReference> {
    let project_root = environment_path.parent().unwrap_or_else(|| Path::new("."));
    let mut references = Vec::new();
    for ty in objtree.iter_types() {
        let direct_icon = ty.vars.get("icon");
        let direct_state = ty.vars.get("icon_state");
        if direct_icon.is_none() && direct_state.is_none() {
            continue;
        }
        let location = direct_state
            .map(|value| value.value.location)
            .or_else(|| direct_icon.map(|value| value.value.location))
            .unwrap_or(ty.location);
        let file = context.file_path(location.file).display().to_string();
        let icon_value = ty.get_value("icon");
        let state_value = ty.get_value("icon_state");
        let resolution = match icon_value.and_then(|value| value.constant.as_ref()) {
            Some(Constant::Null(_)) | None
                if icon_value.is_none_or(|value| value.expression.is_none()) =>
            {
                continue;
            }
            Some(Constant::Resource(resource)) => {
                static_icon_resolution(project_root, resource, state_value)
            }
            Some(Constant::Call(ConstFn::Icon, arguments)) => {
                match arguments.first().map(|(value, _)| value) {
                    Some(Constant::Resource(resource)) => {
                        static_icon_resolution(project_root, resource, state_value)
                    }
                    _ => IconReferenceResolution::Dynamic {
                        reason: "icon() resource is not statically resolvable".to_owned(),
                    },
                }
            }
            _ => IconReferenceResolution::Dynamic {
                reason: "icon expression is not a literal resource".to_owned(),
            },
        };
        references.push(IconReference {
            type_path: ty.path.to_string(),
            file,
            line: location.line,
            resolution,
        });
    }
    references.sort_by(|left, right| {
        (&left.file, left.line, &left.type_path).cmp(&(&right.file, right.line, &right.type_path))
    });
    references
}

fn static_icon_resolution(
    project_root: &Path,
    resource: &str,
    state_value: Option<&dreammaker::objtree::VarValue>,
) -> IconReferenceResolution {
    let dmi_path = project_root.join(resource.replace('\\', "/"));
    let state = match state_value.and_then(|value| value.constant.as_ref()) {
        Some(Constant::String(state)) => Some(state.to_string()),
        Some(Constant::Null(_)) | None
            if state_value.is_none_or(|value| value.expression.is_none()) =>
        {
            None
        }
        _ => {
            return IconReferenceResolution::Dynamic {
                reason: format!(
                    "icon_state expression is dynamic for statically resolved {}",
                    dmi_path.display()
                ),
            };
        }
    };
    IconReferenceResolution::Static { dmi_path, state }
}

pub(crate) fn configured_diagnostic_rules(
    environment_path: &Path,
    context: &Context,
) -> HashSet<String> {
    let Some(root) = environment_path.parent() else {
        return HashSet::new();
    };
    let config_path = root.join("SpacemanDMM.toml");
    if !config_path.exists() {
        return HashSet::new();
    }
    let Ok(config_path) = context.resolve_read_path(&config_path) else {
        return HashSet::new();
    };
    let Ok(source) = std::fs::read_to_string(config_path) else {
        return HashSet::new();
    };
    let Ok(value) = source.parse::<toml::Value>() else {
        return HashSet::new();
    };
    value
        .get("diagnostics")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

pub(crate) fn collect_diagnostics(
    context: &Context,
    configured_rules: &HashSet<String>,
) -> Vec<DiagnosticRecord> {
    context
        .errors()
        .iter()
        .map(|error| {
            let location = error.location();
            DiagnosticRecord {
                rule: error.errortype().map(str::to_owned),
                severity: error.severity().to_string(),
                component: error.component().name().unwrap_or("parser").to_owned(),
                message: error.description().to_owned(),
                file: context.file_path(location.file).display().to_string(),
                line: location.line,
                column: location.column,
                notes: error
                    .notes()
                    .iter()
                    .map(|note| DiagnosticNoteRecord {
                        message: format!("{note:?}"),
                    })
                    .collect(),
                configured: error
                    .errortype()
                    .is_some_and(|rule| configured_rules.contains(rule)),
            }
        })
        .collect()
}
