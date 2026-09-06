use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dreammaker::objtree::ObjectTree;
use dreammaker::Context;
use foldhash::{HashMap, HashMapExt};

use crate::proc_resolution::ProcResolver;
use crate::source::IndexedSource;

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;
const EXACT_SYMBOL_BOOST: f64 = 12.0;
const EXACT_NAME_BOOST: f64 = 6.0;
const PHRASE_BOOST: f64 = 2.0;
const INDEX_SOURCE_LINES: usize = 80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SymbolKind {
    Type,
    Proc,
    Var,
}

impl SymbolKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Proc => "proc",
            Self::Var => "var",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SearchDocument {
    pub(crate) kind: SymbolKind,
    pub(crate) symbol: String,
    pub(crate) name: String,
    pub(crate) type_path: String,
    pub(crate) implementation_owner: Option<String>,
    pub(crate) declaration_owner: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) docs: String,
    pub(crate) source: Option<String>,
    pub(crate) parameters: Vec<String>,
    pub(crate) override_index: Option<usize>,
    pub(crate) override_count: Option<usize>,
}

pub(crate) struct SearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) kind: Option<SymbolKind>,
    pub(crate) type_prefix: Option<&'a str>,
    pub(crate) file_filter: Option<&'a str>,
    pub(crate) limit: usize,
}

pub(crate) struct SearchHit<'a> {
    pub(crate) score: f64,
    pub(crate) document: &'a SearchDocument,
}

pub(crate) struct SearchExecution<'a> {
    pub(crate) hits: Vec<SearchHit<'a>>,
    pub(crate) candidates_considered: usize,
    pub(crate) documents_scored: usize,
}

pub(crate) struct SearchDocuments {
    documents: Vec<SearchDocument>,
}

#[derive(Clone, Debug)]
struct Posting {
    document_id: usize,
    term_frequency: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchIndex {
    documents: Vec<SearchDocument>,
    postings: HashMap<String, Vec<Posting>>,
    exact_symbols: HashMap<String, Vec<usize>>,
    exact_names: HashMap<String, Vec<usize>>,
    document_lengths: Vec<f64>,
    average_document_length: f64,
}

impl SearchIndex {
    pub(crate) fn new(documents: Vec<SearchDocument>) -> Self {
        let mut postings: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut exact_symbols: HashMap<String, Vec<usize>> = HashMap::new();
        let mut exact_names: HashMap<String, Vec<usize>> = HashMap::new();
        let mut document_lengths = Vec::with_capacity(documents.len());

        for (document_id, document) in documents.iter().enumerate() {
            exact_symbols
                .entry(document.symbol.to_lowercase())
                .or_default()
                .push(document_id);
            exact_names
                .entry(document.name.to_lowercase())
                .or_default()
                .push(document_id);
            let terms = weighted_terms(document);
            let length = terms.values().map(|frequency| *frequency as f64).sum();
            document_lengths.push(length);

            for (term, frequency) in terms {
                postings.entry(term).or_default().push(Posting {
                    document_id,
                    term_frequency: frequency as f64,
                });
            }
        }

        let average_document_length = if document_lengths.is_empty() {
            0.0
        } else {
            document_lengths.iter().sum::<f64>() / document_lengths.len() as f64
        };

        Self {
            documents,
            postings,
            exact_symbols,
            exact_names,
            document_lengths,
            average_document_length,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.documents.len()
    }

    pub(crate) fn proc_source(
        &self,
        owner: &str,
        name: &str,
        override_index: usize,
    ) -> Option<&str> {
        self.exact_symbols
            .get(&member_symbol(owner, "proc", name).to_lowercase())?
            .iter()
            .map(|index| &self.documents[*index])
            .find(|document| {
                document.kind == SymbolKind::Proc
                    && document.type_path == owner
                    && document.name == name
                    && document.override_index == Some(override_index)
            })?
            .source
            .as_deref()
    }

    pub(crate) fn source_line_limit(&self) -> usize {
        INDEX_SOURCE_LINES
    }
}

impl SearchDocuments {
    pub(crate) fn from_object_tree(
        objtree: &ObjectTree,
        context: &Context,
        environment_path: &Path,
    ) -> Self {
        let root = environment_path.parent().unwrap_or_else(|| Path::new("."));
        let mut source_cache = SourceCache::new(context);
        let mut documents = Vec::new();

        for ty in objtree.iter_types() {
            let type_path = ty.path.to_string();
            let parent = ty.parent_type().map(|parent| parent.path.to_string());

            if !ty.is_root() && !ty.location.is_builtins() {
                let file = resolve_context_file(context, root, ty.location.file);
                let source = source_cache.source_line(&file, ty.location.line);
                documents.push(SearchDocument {
                    kind: SymbolKind::Type,
                    symbol: type_path.clone(),
                    name: ty.name().to_string(),
                    type_path: type_path.clone(),
                    implementation_owner: None,
                    declaration_owner: None,
                    parent: parent.clone(),
                    file: file.display().to_string(),
                    line: ty.location.line,
                    column: u32::from(ty.location.column),
                    docs: ty.docs.text().trim().to_string(),
                    source,
                    parameters: Vec::new(),
                    override_index: None,
                    override_count: None,
                });
            }

            for (name, var) in &ty.vars {
                let location = var.value.location;
                if location.is_builtins() {
                    continue;
                }

                let file = resolve_context_file(context, root, location.file);
                documents.push(SearchDocument {
                    kind: SymbolKind::Var,
                    symbol: member_symbol(&type_path, "var", name),
                    name: name.to_string(),
                    type_path: type_path.clone(),
                    implementation_owner: None,
                    declaration_owner: None,
                    parent: parent.clone(),
                    file: file.display().to_string(),
                    line: location.line,
                    column: u32::from(location.column),
                    docs: var.value.docs.text().trim().to_string(),
                    source: source_cache.source_line(&file, location.line),
                    parameters: Vec::new(),
                    override_index: None,
                    override_count: None,
                });
            }

            for (name, proc) in &ty.procs {
                let override_count = proc
                    .value
                    .iter()
                    .filter(|value| !value.location.is_builtins())
                    .count();
                let mut override_index = 0;
                for value in &proc.value {
                    let location = value.location;
                    if location.is_builtins() {
                        continue;
                    }
                    override_index += 1;

                    let file = resolve_context_file(context, root, location.file);
                    documents.push(SearchDocument {
                        kind: SymbolKind::Proc,
                        symbol: member_symbol(&type_path, "proc", name),
                        name: name.to_string(),
                        type_path: type_path.clone(),
                        implementation_owner: None,
                        declaration_owner: None,
                        parent: parent.clone(),
                        file: file.display().to_string(),
                        line: location.line,
                        column: u32::from(location.column),
                        docs: value.docs.text().trim().to_string(),
                        source: source_cache.source_declaration(
                            &file,
                            location.line,
                            INDEX_SOURCE_LINES,
                        ),
                        parameters: value
                            .parameters
                            .iter()
                            .map(|parameter| parameter.name.to_string())
                            .collect(),
                        override_index: Some(override_index),
                        override_count: Some(override_count),
                    });
                }
            }
        }

        Self { documents }
    }

    pub(crate) fn canonicalize_procs(mut self, resolver: &ProcResolver) -> Self {
        let mut canonical = HashMap::new();
        for resolution in resolver
            .resolutions()
            .filter(|resolution| resolution.requested_type_path == resolution.implementation_owner)
        {
            let override_count = resolution
                .implementations
                .iter()
                .filter(|implementation| implementation.owner == resolution.implementation_owner)
                .count();
            for implementation in resolution
                .implementations
                .iter()
                .filter(|implementation| implementation.owner == resolution.implementation_owner)
            {
                canonical.insert(
                    (
                        resolution.proc_name.clone(),
                        normalize_path_text(&implementation.location.file),
                        implementation.location.line,
                        u32::from(implementation.location.column),
                    ),
                    (
                        resolution.implementation_owner.clone(),
                        resolution.declaration_owner.clone(),
                        implementation.override_index,
                        override_count,
                    ),
                );
            }
        }

        let mut seen = BTreeSet::new();
        self.documents.retain_mut(|document| {
            if document.kind != SymbolKind::Proc {
                return true;
            }
            let Some((implementation_owner, declaration_owner, override_index, override_count)) =
                canonical.get(&(
                    document.name.clone(),
                    normalize_path_text(&document.file),
                    document.line,
                    document.column,
                ))
            else {
                return false;
            };
            document.type_path = implementation_owner.clone();
            document.symbol = member_symbol(implementation_owner, "proc", &document.name);
            document.implementation_owner = Some(implementation_owner.clone());
            document.declaration_owner = Some(declaration_owner.clone());
            document.override_index = Some(*override_index);
            document.override_count = Some(*override_count);
            seen.insert((
                document.symbol.clone(),
                document.file.clone(),
                document.line,
                document.column,
                *override_index,
            ))
        });
        self
    }

    pub(crate) fn into_index(self) -> SearchIndex {
        SearchIndex::new(self.documents)
    }
}

impl SearchIndex {
    pub(crate) fn query_terms(query: &str) -> Vec<String> {
        tokenize(query)
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn search(&self, request: &SearchRequest<'_>) -> SearchExecution<'_> {
        let query_terms = Self::query_terms(request.query);
        if query_terms.is_empty() || request.limit == 0 {
            return SearchExecution {
                hits: Vec::new(),
                candidates_considered: 0,
                documents_scored: 0,
            };
        }

        let normalized_query = request.query.trim().to_lowercase();
        let mut scores = HashMap::<usize, f64>::new();
        if let Some(exact_symbols) = self.exact_symbols.get(&normalized_query) {
            scores.extend(exact_symbols.iter().map(|document_id| (*document_id, 0.0)));
        } else {
            for term in query_terms {
                let Some(postings) = self.postings.get(&term) else {
                    continue;
                };

                let document_frequency = postings.len() as f64;
                let document_count = self.documents.len() as f64;
                let inverse_document_frequency = (1.0
                    + (document_count - document_frequency + 0.5) / (document_frequency + 0.5))
                    .ln();

                for posting in postings {
                    let document_length = self.document_lengths[posting.document_id];
                    let length_normalization = if self.average_document_length == 0.0 {
                        1.0
                    } else {
                        1.0 - BM25_B + BM25_B * document_length / self.average_document_length
                    };
                    let numerator = posting.term_frequency * (BM25_K1 + 1.0);
                    let denominator = posting.term_frequency + BM25_K1 * length_normalization;
                    *scores.entry(posting.document_id).or_default() +=
                        inverse_document_frequency * numerator / denominator;
                }
            }
            for document_id in self
                .exact_names
                .get(&normalized_query)
                .into_iter()
                .flatten()
            {
                scores.entry(*document_id).or_default();
            }
        }
        let candidates = if scores.is_empty() {
            self.documents
                .iter()
                .enumerate()
                .filter(|(_, document)| {
                    contains_case_insensitive(&document.symbol, &normalized_query)
                })
                .map(|(document_id, _)| document_id)
                .collect::<Vec<_>>()
        } else {
            scores.keys().copied().collect::<Vec<_>>()
        };

        let candidates_considered = candidates.len();
        let type_prefix = request.type_prefix.map(str::to_lowercase);
        let file_filter = request.file_filter.map(str::to_lowercase);
        let mut documents_scored = 0;
        let mut hits: Vec<SearchHit<'_>> = candidates
            .into_iter()
            .filter_map(|document_id| {
                let document = &self.documents[document_id];
                if !request.kind.is_none_or(|kind| document.kind == kind) {
                    return None;
                }
                if !type_prefix
                    .as_ref()
                    .is_none_or(|prefix| starts_with_case_insensitive(&document.type_path, prefix))
                {
                    return None;
                }
                if !file_filter
                    .as_ref()
                    .is_none_or(|filter| contains_case_insensitive(&document.file, filter))
                {
                    return None;
                }
                documents_scored += 1;
                let mut score = scores.get(&document_id).copied().unwrap_or_default();
                if equals_case_insensitive(&document.symbol, &normalized_query) {
                    score += EXACT_SYMBOL_BOOST;
                } else if equals_case_insensitive(&document.name, &normalized_query) {
                    score += EXACT_NAME_BOOST;
                } else if contains_case_insensitive(&document.symbol, &normalized_query) {
                    score += PHRASE_BOOST;
                }

                (score > 0.0).then_some(SearchHit { score, document })
            })
            .collect();

        if hits.len() > request.limit {
            hits.select_nth_unstable_by(request.limit, compare_hits);
            hits.truncate(request.limit);
        }
        hits.sort_by(compare_hits);
        SearchExecution {
            hits,
            candidates_considered,
            documents_scored,
        }
    }
}

fn compare_hits(left: &SearchHit<'_>, right: &SearchHit<'_>) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then_with(|| left.document.symbol.cmp(&right.document.symbol))
        .then_with(|| left.document.file.cmp(&right.document.file))
        .then_with(|| left.document.line.cmp(&right.document.line))
        .then_with(|| left.document.column.cmp(&right.document.column))
}

fn equals_case_insensitive(text: &str, normalized_query: &str) -> bool {
    if text.is_ascii() && normalized_query.is_ascii() {
        text.eq_ignore_ascii_case(normalized_query)
    } else {
        text.to_lowercase() == normalized_query
    }
}

fn starts_with_case_insensitive(text: &str, normalized_prefix: &str) -> bool {
    if text.is_ascii() && normalized_prefix.is_ascii() {
        text.get(..normalized_prefix.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(normalized_prefix))
    } else {
        text.to_lowercase().starts_with(normalized_prefix)
    }
}

fn contains_case_insensitive(text: &str, normalized_needle: &str) -> bool {
    if text.is_ascii() && normalized_needle.is_ascii() {
        if text.bytes().all(|byte| !byte.is_ascii_uppercase()) {
            text.contains(normalized_needle)
        } else {
            text.to_ascii_lowercase().contains(normalized_needle)
        }
    } else {
        text.to_lowercase().contains(normalized_needle)
    }
}

fn weighted_terms(document: &SearchDocument) -> HashMap<String, u32> {
    let mut terms = HashMap::new();
    add_weighted_terms(&mut terms, &document.name, 10);
    add_weighted_terms(&mut terms, &document.symbol, 8);
    add_weighted_terms(&mut terms, &document.type_path, 5);
    add_weighted_terms(&mut terms, &document.docs, 4);
    add_weighted_terms(&mut terms, &document.parameters.join(" "), 4);
    add_weighted_terms(
        &mut terms,
        document.parent.as_deref().unwrap_or_default(),
        2,
    );
    add_weighted_terms(&mut terms, &document.file, 2);
    add_weighted_terms(
        &mut terms,
        document.source.as_deref().unwrap_or_default(),
        1,
    );
    terms
}

fn add_weighted_terms(terms: &mut HashMap<String, u32>, text: &str, weight: u32) {
    for term in tokenize(text) {
        *terms.entry(term).or_default() += weight;
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn member_symbol(type_path: &str, kind: &str, name: &str) -> String {
    if type_path.is_empty() {
        format!("/{kind}/{name}")
    } else {
        format!("{type_path}/{kind}/{name}")
    }
}

fn normalize_path_text(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('\\', "/").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}

fn resolve_context_file(
    context: &Context,
    environment_root: &Path,
    file_id: dreammaker::FileId,
) -> PathBuf {
    let path_ref = context.file_path(file_id);
    let context_path = path_ref.to_path_buf();
    if context_path.is_absolute() {
        context_path
    } else {
        environment_root.join(context_path)
    }
}

struct SourceCache<'a> {
    context: &'a Context,
    files: HashMap<PathBuf, Option<IndexedSource>>,
}

impl<'a> SourceCache<'a> {
    fn new(context: &'a Context) -> Self {
        Self {
            context,
            files: HashMap::new(),
        }
    }

    fn source(&mut self, file: &Path) -> Option<&IndexedSource> {
        if !self.files.contains_key(file) {
            self.files.insert(
                file.to_path_buf(),
                self.context
                    .resolve_read_path(file)
                    .ok()
                    .and_then(|path| IndexedSource::read(&path).ok()),
            );
        }
        self.files.get(file).and_then(Option::as_ref)
    }

    fn source_line(&mut self, file: &Path, line: u32) -> Option<String> {
        self.source(file)?.line(line).map(str::to_owned)
    }

    fn source_declaration(&mut self, file: &Path, line: u32, max_lines: usize) -> Option<String> {
        self.source(file)?.declaration(line, max_lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreammaker::Context;
    use std::sync::atomic::{AtomicU64, Ordering};

    static ENVIRONMENT_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn document(
        kind: SymbolKind,
        symbol: &str,
        name: &str,
        type_path: &str,
        file: &str,
        docs: &str,
        source: &str,
    ) -> SearchDocument {
        SearchDocument {
            kind,
            symbol: symbol.to_string(),
            name: name.to_string(),
            type_path: type_path.to_string(),
            implementation_owner: None,
            declaration_owner: None,
            parent: None,
            file: file.to_string(),
            line: 1,
            column: 1,
            docs: docs.to_string(),
            source: Some(source.to_string()),
            parameters: Vec::new(),
            override_index: None,
            override_count: None,
        }
    }

    fn request(query: &str) -> SearchRequest<'_> {
        SearchRequest {
            query,
            kind: None,
            type_prefix: None,
            file_filter: None,
            limit: 10,
        }
    }

    fn write_environment_fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let unique_suffix = format!(
            "{}_{}",
            std::process::id(),
            ENVIRONMENT_FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let directory =
            std::env::temp_dir().join(format!("meridian_mcp_environment_{unique_suffix}"));
        std::fs::create_dir_all(&directory).expect("fixture directory should be writable");
        let source_path = directory.join("fixture.dm");
        std::fs::write(
            &source_path,
            "/datum/example\n\tvar/temperature = 300\n\n/// Reset the gas mixture temperature after an air update.\n/datum/example/proc/reset_temperature(target_temperature)\n\ttemperature = target_temperature\n\treturn temperature\n",
        )
        .expect("source fixture should be writable");
        let environment_path = directory.join("fixture.dme");
        std::fs::write(&environment_path, "#include \"fixture.dm\"\n")
            .expect("environment fixture should be writable");
        (directory, environment_path)
    }

    #[test]
    fn behavioral_terms_rank_relevant_document_above_noise() {
        let index = SearchIndex::new(vec![
            document(
                SymbolKind::Proc,
                "/datum/controller/subsystem/air/proc/reset_temperature",
                "reset_temperature",
                "/datum/controller/subsystem/air",
                "code/air.dm",
                "Reset turf temperature after an air change.",
                "turf.air.temperature = initial_temperature",
            ),
            document(
                SymbolKind::Proc,
                "/datum/controller/subsystem/chat/proc/send_message",
                "send_message",
                "/datum/controller/subsystem/chat",
                "code/chat.dm",
                "Send a message to a client.",
                "client << message",
            ),
        ]);

        let hits = index.search(&request("turf air temperature reset")).hits;

        assert_eq!(hits[0].document.name, "reset_temperature");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn exact_symbol_match_receives_a_boost() {
        let index = SearchIndex::new(vec![
            document(
                SymbolKind::Proc,
                "/datum/example/proc/update_state",
                "update_state",
                "/datum/example",
                "code/first.dm",
                "Update state.",
                "return state",
            ),
            document(
                SymbolKind::Proc,
                "/datum/other/proc/update_state",
                "update_state",
                "/datum/other",
                "code/second.dm",
                "Update state.",
                "return state",
            ),
        ]);

        let hits = index
            .search(&request("/datum/other/proc/update_state"))
            .hits;

        assert_eq!(hits[0].document.symbol, "/datum/other/proc/update_state");
    }

    #[test]
    fn exact_symbol_queries_skip_broad_term_candidates() {
        let index = SearchIndex::new(vec![
            document(
                SymbolKind::Proc,
                "/datum/example/proc/update_state",
                "update_state",
                "/datum/example",
                "code/first.dm",
                "Update state.",
                "return state",
            ),
            document(
                SymbolKind::Proc,
                "/datum/other/proc/update_state",
                "update_state",
                "/datum/other",
                "code/second.dm",
                "Update state.",
                "return state",
            ),
        ]);

        let execution = index.search(&request("/datum/other/proc/update_state"));

        assert_eq!(execution.candidates_considered, 1);
        assert_eq!(execution.documents_scored, 1);
        assert_eq!(
            execution.hits[0].document.symbol,
            "/datum/other/proc/update_state"
        );
    }

    #[test]
    fn filters_and_limit_are_applied_before_ranking() {
        let index = SearchIndex::new(vec![
            document(
                SymbolKind::Proc,
                "/turf/open/proc/update_air",
                "update_air",
                "/turf/open",
                "modular_aphelion/air.dm",
                "Update air.",
                "return",
            ),
            document(
                SymbolKind::Var,
                "/turf/open/var/air",
                "air",
                "/turf/open",
                "modular_aphelion/air.dm",
                "Air mixture.",
                "var/datum/gas_mixture/air",
            ),
            document(
                SymbolKind::Proc,
                "/obj/item/proc/update_air",
                "update_air",
                "/obj/item",
                "code/items.dm",
                "Update air.",
                "return",
            ),
        ]);
        let filtered = SearchRequest {
            query: "air update",
            kind: Some(SymbolKind::Proc),
            type_prefix: Some("/turf"),
            file_filter: Some("modular_aphelion"),
            limit: 1,
        };

        let hits = index.search(&filtered).hits;

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].document.symbol, "/turf/open/proc/update_air");
    }

    #[test]
    fn equal_scores_use_stable_symbol_file_and_line_order() {
        let mut second = document(
            SymbolKind::Proc,
            "/datum/example/proc/beta",
            "shared",
            "/datum/example",
            "code/z.dm",
            "Shared behavior.",
            "return shared",
        );
        second.line = 20;
        let index = SearchIndex::new(vec![
            second,
            document(
                SymbolKind::Proc,
                "/datum/example/proc/alpha",
                "shared",
                "/datum/example",
                "code/a.dm",
                "Shared behavior.",
                "return shared",
            ),
        ]);

        let hits = index.search(&request("shared")).hits;

        assert_eq!(hits[0].document.symbol, "/datum/example/proc/alpha");
        assert_eq!(hits[1].document.symbol, "/datum/example/proc/beta");
    }

    #[test]
    fn partial_symbol_queries_use_substring_candidates() {
        let index = SearchIndex::new(vec![
            document(
                SymbolKind::Proc,
                "/datum/dogmos_kennel/proc/status",
                "status",
                "/datum/dogmos_kennel",
                "code/dogmos.dm",
                "",
                "return",
            ),
            document(
                SymbolKind::Proc,
                "/datum/unrelated/proc/status",
                "status",
                "/datum/unrelated",
                "code/other.dm",
                "",
                "return",
            ),
        ]);

        let execution = index.search(&request("dogmos_kenn"));

        assert_eq!(
            execution.hits[0].document.symbol,
            "/datum/dogmos_kennel/proc/status"
        );

        let filtered = SearchRequest {
            query: "dogmos_kenn",
            kind: Some(SymbolKind::Proc),
            type_prefix: Some("/datum/dogmos"),
            file_filter: Some("dogmos"),
            limit: 1,
        };
        let filtered_execution = index.search(&filtered);
        assert_eq!(filtered_execution.hits.len(), 1);
        assert_eq!(
            filtered_execution.hits[0].document.symbol,
            "/datum/dogmos_kennel/proc/status"
        );
    }

    #[test]
    fn absent_terms_do_not_score_the_whole_corpus() {
        let index = SearchIndex::new(
            (0..10_000)
                .map(|number| {
                    document(
                        SymbolKind::Type,
                        &format!("/datum/noise_{number}"),
                        &format!("noise_{number}"),
                        "/datum",
                        "code/noise.dm",
                        "unrelated fixture",
                        "return",
                    )
                })
                .collect(),
        );

        let execution = index.search(&request("term_that_is_not_present"));

        assert!(execution.hits.is_empty());
        assert_eq!(execution.documents_scored, 0);
    }

    #[test]
    fn object_tree_index_preserves_proc_source_docs_and_relationships() {
        let (directory, environment_path) = write_environment_fixture();
        let context = Context::default();
        let objtree = context
            .parse_environment(&environment_path)
            .expect("fixture environment should parse");
        let index =
            SearchDocuments::from_object_tree(&objtree, &context, &environment_path).into_index();

        let hits = index
            .search(&request("gas mixture temperature air reset"))
            .hits;
        let proc_hit = hits
            .iter()
            .find(|hit| hit.document.kind == SymbolKind::Proc)
            .expect("documented proc should be indexed");

        assert_eq!(proc_hit.document.name, "reset_temperature");
        assert_eq!(proc_hit.document.type_path, "/datum/example");
        assert_eq!(proc_hit.document.parent.as_deref(), Some("/datum"));
        assert!(proc_hit.document.file.ends_with("fixture.dm"));
        assert!(proc_hit.document.docs.contains("gas mixture temperature"));
        assert!(proc_hit
            .document
            .source
            .as_deref()
            .is_some_and(|source| source.contains("target_temperature")));
        assert_eq!(proc_hit.document.parameters, ["target_temperature"]);
        assert_eq!(proc_hit.document.override_index, Some(1));
        assert_eq!(proc_hit.document.override_count, Some(1));

        let _ = std::fs::remove_dir_all(directory);
    }
}
