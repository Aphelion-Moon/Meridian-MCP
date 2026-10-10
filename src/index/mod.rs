use crate::analysis_snapshot::{AnalysisContext, MacroDefinitionRecord};
use crate::proc_resolution::ProcResolver;
use dreammaker::objtree::ObjectTree;
use dreammaker::FileId;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SymbolId {
    Type {
        path: Arc<str>,
    },
    Proc {
        owner: Arc<str>,
        name: Arc<str>,
        override_index: usize,
    },
    Var {
        owner: Arc<str>,
        name: Arc<str>,
    },
    Macro {
        name: Arc<str>,
        file: Arc<str>,
        line: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Type,
    Proc,
    Var,
    Macro,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct DocumentSymbol {
    pub id: SymbolId,
    pub name: Arc<str>,
    pub kind: SymbolKind,
    pub owner: Option<Arc<str>>,
    pub implementation_owner: Option<Arc<str>>,
    pub declaration_owner: Option<Arc<str>>,
    pub file: Arc<str>,
    pub line: u32,
    pub column: u16,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Call,
    Read,
    Write,
    TypePath,
    MacroExpansion,
    Declaration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ReferenceHit {
    pub symbol: SymbolId,
    pub kind: ReferenceKind,
    pub file: String,
    pub line: u32,
    pub column: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ImplementationHit {
    pub symbol: SymbolId,
    pub declared_in: Arc<str>,
    pub implementation_owner: Arc<str>,
    pub declaration_owner: Arc<str>,
    pub inherited_from: Option<Arc<str>>,
    pub file: Arc<str>,
    pub line: u32,
    pub column: u16,
}

#[derive(Clone, Debug, Default)]
pub struct LanguageIndex {
    documents: BTreeMap<PathBuf, Vec<DocumentSymbol>>,
    implementations: Vec<ImplementationHit>,
    implementation_ranges: BTreeMap<Arc<str>, Range<usize>>,
    hierarchy_valid: bool,
}

/// Build-local interning shares repeated text between rows and the two indexes.
/// The lookup tables are dropped after construction; only referenced text lives
/// with the snapshot. File IDs avoid repeatedly formatting the same path.
#[derive(Default)]
struct TextPool {
    strings: HashSet<Arc<str>>,
    files: HashMap<FileId, Arc<str>>,
}

impl TextPool {
    fn intern(&mut self, text: &str) -> Arc<str> {
        if let Some(shared) = self.strings.get(text) {
            return shared.clone();
        }
        let shared: Arc<str> = text.into();
        self.strings.insert(shared.clone());
        shared
    }

    fn file(&mut self, context: &AnalysisContext, id: FileId) -> Arc<str> {
        if let Some(file) = self.files.get(&id) {
            return file.clone();
        }
        let file = self.intern(&context.file_path(id).to_string_lossy());
        self.files.insert(id, file.clone());
        file
    }
}

impl LanguageIndex {
    pub fn build(
        context: &AnalysisContext,
        objtree: &ObjectTree,
        macros: &[MacroDefinitionRecord],
        proc_resolver: &ProcResolver,
    ) -> Self {
        let mut index = Self::default();
        let mut text = TextPool::default();
        for macro_record in macros {
            let name = text.intern(&macro_record.name);
            let file = text.intern(&macro_record.file);
            index.insert(DocumentSymbol {
                id: SymbolId::Macro {
                    name: name.clone(),
                    file: file.clone(),
                    line: macro_record.line,
                },
                name,
                kind: SymbolKind::Macro,
                owner: None,
                implementation_owner: None,
                declaration_owner: None,
                file,
                line: macro_record.line,
                column: macro_record.column,
            });
        }
        for ty in objtree.iter_types() {
            let owner = text.intern(&ty.path);
            let file = text.file(context, ty.location.file);
            index.insert(DocumentSymbol {
                id: SymbolId::Type {
                    path: owner.clone(),
                },
                name: text.intern(owner.rsplit('/').next().unwrap_or("/")),
                kind: SymbolKind::Type,
                owner: None,
                implementation_owner: None,
                declaration_owner: None,
                file: file.clone(),
                line: ty.location.line,
                column: ty.location.column,
            });
            index.implementations.push(ImplementationHit {
                symbol: SymbolId::Type {
                    path: owner.clone(),
                },
                declared_in: owner.clone(),
                implementation_owner: owner.clone(),
                declaration_owner: owner.clone(),
                inherited_from: ty.parent_type().map(|parent| text.intern(&parent.path)),
                file: file.clone(),
                line: ty.location.line,
                column: ty.location.column,
            });
            for (name, var) in &ty.vars {
                let Some(declaration_owner) = ty.iter_parent_types().find(|parent| {
                    parent
                        .vars
                        .get(name)
                        .is_some_and(|value| value.declaration.is_some())
                }) else {
                    continue;
                };
                let declaration_owner = text.intern(&declaration_owner.path);
                let file = text.file(context, var.value.location.file);
                let name_text = text.intern(name);
                let symbol = SymbolId::Var {
                    owner: owner.clone(),
                    name: name_text.clone(),
                };
                index.insert(DocumentSymbol {
                    id: symbol.clone(),
                    name: name_text,
                    kind: SymbolKind::Var,
                    owner: Some(owner.clone()),
                    implementation_owner: Some(owner.clone()),
                    declaration_owner: Some(declaration_owner.clone()),
                    file: file.clone(),
                    line: var.value.location.line,
                    column: var.value.location.column,
                });
                index.implementations.push(ImplementationHit {
                    symbol,
                    declared_in: owner.clone(),
                    implementation_owner: owner.clone(),
                    declaration_owner,
                    inherited_from: ty
                        .parent_type()
                        .filter(|parent| parent.get_value(name).is_some())
                        .map(|parent| text.intern(&parent.path)),
                    file,
                    line: var.value.location.line,
                    column: var.value.location.column,
                });
            }
            for proc_ref in ty.iter_self_procs() {
                let value = proc_ref.get();
                let resolution = proc_resolver
                    .resolve(&owner, proc_ref.name())
                    .expect("a local proc implementation must resolve from its owner");
                let file = text.file(context, value.location.file);
                let name = text.intern(proc_ref.name());
                let declaration_owner = text.intern(&resolution.declaration_owner);
                let symbol = SymbolId::Proc {
                    owner: owner.clone(),
                    name: name.clone(),
                    override_index: proc_ref.index(),
                };
                index.insert(DocumentSymbol {
                    id: symbol.clone(),
                    name,
                    kind: SymbolKind::Proc,
                    owner: Some(owner.clone()),
                    implementation_owner: Some(owner.clone()),
                    declaration_owner: Some(declaration_owner.clone()),
                    file: file.clone(),
                    line: value.location.line,
                    column: value.location.column,
                });
                index.implementations.push(ImplementationHit {
                    symbol,
                    declared_in: owner.clone(),
                    implementation_owner: owner.clone(),
                    declaration_owner,
                    inherited_from: proc_ref
                        .parent_proc()
                        .map(|parent| text.intern(&parent.ty().path)),
                    file,
                    line: value.location.line,
                    column: value.location.column,
                });
            }
        }
        for symbols in index.documents.values_mut() {
            symbols.sort_by(|left, right| {
                (left.line, left.column, left.kind, &left.name).cmp(&(
                    right.line,
                    right.column,
                    right.kind,
                    &right.name,
                ))
            });
            symbols.dedup();
        }
        index.order_implementations(objtree, &mut text);
        index
    }

    /// Contiguous semantic subtrees avoid scanning or cloning the entire index
    /// for a narrow implementation query. Siblings have stable path order.
    fn order_implementations(&mut self, objtree: &ObjectTree, text: &mut TextPool) {
        let mut children: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
        for ty in objtree.iter_types() {
            children
                .entry(ty.parent_type().map(|parent| parent.get().path.as_str()))
                .or_default()
                .push(ty.get().path.as_str());
        }
        for children in children.values_mut() {
            children.sort_unstable();
        }
        let mut stack: Vec<_> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|path| (*path, false))
            .collect();
        let mut ranks = HashMap::new();
        let mut subtrees = Vec::new();
        while let Some((path, exiting)) = stack.pop() {
            if exiting {
                subtrees.push((path, ranks[path]..ranks.len()));
            } else if !ranks.contains_key(path) {
                ranks.insert(path, ranks.len());
                stack.push((path, true));
                if let Some(children) = children.get(&Some(path)) {
                    stack.extend(children.iter().rev().map(|path| (*path, false)));
                }
            }
        }
        self.hierarchy_valid = ranks.len() == objtree.iter_types().count();
        self.implementations.sort_by_cached_key(|hit| {
            (
                ranks
                    .get(hit.declared_in.as_ref())
                    .copied()
                    .unwrap_or(usize::MAX),
                hit.line,
                hit.column,
            )
        });
        let mut boundaries = vec![0; ranks.len() + 1];
        for hit in &self.implementations {
            if let Some(rank) = ranks.get(hit.declared_in.as_ref()) {
                boundaries[rank + 1] += 1;
            }
        }
        for rank in 1..boundaries.len() {
            boundaries[rank] += boundaries[rank - 1];
        }
        for (path, range) in subtrees {
            self.implementation_ranges.insert(
                text.intern(path),
                boundaries[range.start]..boundaries[range.end],
            );
        }
    }

    fn insert(&mut self, symbol: DocumentSymbol) {
        self.documents
            .entry(normalize_path(Path::new(symbol.file.as_ref())))
            .or_default()
            .push(symbol);
    }

    pub fn document_symbols(&self, file: &Path) -> &[DocumentSymbol] {
        self.documents
            .get(&normalize_path(file))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn macros(&self) -> impl Iterator<Item = &DocumentSymbol> {
        self.documents
            .values()
            .flatten()
            .filter(|symbol| symbol.kind == SymbolKind::Macro)
    }

    pub fn source_files(&self) -> Vec<PathBuf> {
        let mut files = self
            .documents
            .values()
            .flatten()
            .map(|symbol| PathBuf::from(symbol.file.as_ref()))
            .collect::<Vec<_>>();
        files.sort();
        files.dedup();
        files
    }

    pub fn proc_at(&self, file: &Path, line: u32) -> Option<&DocumentSymbol> {
        self.document_symbols(file)
            .iter()
            .take_while(|symbol| symbol.line <= line)
            .last()
            .filter(|symbol| symbol.kind == SymbolKind::Proc)
    }

    pub fn implementations(&self, owner: &str, member: Option<&str>) -> Vec<ImplementationHit> {
        self.implementation_iter(owner, member).cloned().collect()
    }

    pub fn hierarchy_is_valid(&self) -> bool {
        self.hierarchy_valid
    }

    pub fn implementation_iter<'a>(
        &'a self,
        owner: &str,
        member: Option<&'a str>,
    ) -> impl Iterator<Item = &'a ImplementationHit> + Clone {
        let range = self
            .implementation_ranges
            .get(owner)
            .cloned()
            .unwrap_or(0..0);
        self.implementations[range]
            .iter()
            .filter(move |hit| match (&hit.symbol, member) {
                (SymbolId::Type { .. }, None) => true,
                (SymbolId::Proc { name, .. } | SymbolId::Var { name, .. }, Some(member)) => {
                    name.as_ref() == member
                }
                _ => false,
            })
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(
            path.to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase(),
        )
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}
