use crate::analysis_snapshot::AnalysisContext;
use dreammaker::objtree::ObjectTree;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SAME_NAME_CANDIDATES: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcResolutionKind {
    LocalImplementation,
    InheritedImplementation,
    NotFound,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ResolvedProcImplementation {
    pub owner: String,
    pub override_index: usize,
    pub location: SourceLocation,
    pub has_body: bool,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ProcResolution {
    pub requested_type_path: String,
    pub proc_name: String,
    pub implementation_owner: String,
    pub declaration_owner: String,
    pub resolution_kind: ProcResolutionKind,
    pub implementations: Vec<ResolvedProcImplementation>,
}

impl ProcResolution {
    pub fn diagnostics(&self) -> Vec<String> {
        match self.resolution_kind {
            ProcResolutionKind::LocalImplementation => Vec::new(),
            ProcResolutionKind::InheritedImplementation => vec![format!(
                "requested type inherits the implementation from {}",
                self.implementation_owner
            )],
            ProcResolutionKind::NotFound => vec!["procedure was not found".to_owned()],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, thiserror::Error, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcResolutionError {
    #[error("Procedure ancestry exceeds the bounded query work limit")]
    HierarchyLimit,
    #[error("Type not found: {requested_type_path}")]
    TypeNotFound { requested_type_path: String },
    #[error("Proc not found: {requested_type_path}/{proc_name}")]
    NotFound {
        requested_type_path: String,
        proc_name: String,
        searched_type_chain: Vec<String>,
        same_name_candidates: Vec<String>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct ProcResolver {
    parents: BTreeMap<String, Option<String>>,
    declarations: BTreeMap<String, BTreeSet<String>>,
    local_implementations: BTreeMap<String, BTreeMap<String, Vec<ResolvedProcImplementation>>>,
    owners_by_proc_name: BTreeMap<String, BTreeSet<String>>,
    canonical_resolutions: Vec<ProcResolution>,
}

/// A query borrows implementations; it does not clone every override merely to
/// discard it after applying the response budget.
pub(crate) struct ProcView<'a> {
    pub implementation_owner: &'a str,
    pub declaration_owner: &'a str,
    pub resolution_kind: ProcResolutionKind,
    pub implementations: ProcImplementations<'a>,
}
#[derive(Clone)]
pub(crate) struct ProcImplementations<'a> {
    resolver: &'a ProcResolver,
    owner: Option<&'a str>,
    name: &'a str,
    local: std::slice::Iter<'a, ResolvedProcImplementation>,
    ancestors: usize,
}
impl<'a> Iterator for ProcImplementations<'a> {
    type Item = &'a ResolvedProcImplementation;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(value) = self.local.next() {
                return Some(value);
            }
            let owner = self.owner?;
            if self.ancestors == 4096 {
                return None;
            }
            self.ancestors += 1;
            self.owner = self.resolver.parents.get(owner).and_then(|v| v.as_deref());
            self.local = self
                .resolver
                .local_implementations
                .get(owner)
                .and_then(|procs| procs.get(self.name))
                .map(|rows| rows.as_slice())
                .unwrap_or(&[])
                .iter();
        }
    }
}

impl ProcResolver {
    pub(crate) fn view<'a>(
        &'a self,
        requested: &'a str,
        name: &'a str,
    ) -> Result<ProcView<'a>, ProcResolutionError> {
        if !self.parents.contains_key(requested) {
            return Err(ProcResolutionError::TypeNotFound {
                requested_type_path: requested.into(),
            });
        }
        let mut owner = Some(requested);
        let mut implementation_owner = None;
        let mut declaration_owner = None;
        let mut searched = Vec::new();
        while let Some(current) = owner {
            if searched.len() == 4096 {
                return Err(ProcResolutionError::HierarchyLimit);
            }
            searched.push(current.to_owned());
            if declaration_owner.is_none()
                && self
                    .declarations
                    .get(current)
                    .is_some_and(|names| names.contains(name))
            {
                declaration_owner = Some(current);
            }
            if implementation_owner.is_none()
                && self
                    .local_implementations
                    .get(current)
                    .is_some_and(|procs| procs.contains_key(name))
            {
                implementation_owner = Some(current);
            }
            owner = self.parents.get(current).and_then(|v| v.as_deref());
        }
        let implementation_owner =
            implementation_owner.ok_or_else(|| ProcResolutionError::NotFound {
                requested_type_path: requested.into(),
                proc_name: name.into(),
                searched_type_chain: searched,
                same_name_candidates: self
                    .owners_by_proc_name
                    .get(name)
                    .into_iter()
                    .flatten()
                    .take(MAX_SAME_NAME_CANDIDATES)
                    .cloned()
                    .collect(),
            })?;
        Ok(ProcView {
            implementation_owner,
            declaration_owner: declaration_owner.unwrap_or(implementation_owner),
            resolution_kind: if implementation_owner == requested {
                ProcResolutionKind::LocalImplementation
            } else {
                ProcResolutionKind::InheritedImplementation
            },
            implementations: ProcImplementations {
                resolver: self,
                owner: Some(requested),
                name,
                local: [].iter(),
                ancestors: 0,
            },
        })
    }
    pub fn build(context: &AnalysisContext, objtree: &ObjectTree) -> Self {
        let mut resolver = Self::default();
        for ty in objtree.iter_types() {
            let owner = ty.path.to_string();
            resolver.parents.insert(
                owner.clone(),
                ty.parent_type().map(|parent| parent.path.to_string()),
            );
            let declarations = ty
                .procs
                .iter()
                .filter(|(_, proc_entry)| proc_entry.declaration.is_some())
                .map(|(name, _)| name.to_string())
                .collect::<BTreeSet<_>>();
            if !declarations.is_empty() {
                resolver.declarations.insert(owner.clone(), declarations);
            }

            let mut local = BTreeMap::<String, Vec<ResolvedProcImplementation>>::new();
            for proc_ref in ty.iter_self_procs() {
                let value = proc_ref.get();
                let proc_name = proc_ref.name().to_owned();
                local
                    .entry(proc_name.clone())
                    .or_default()
                    .push(ResolvedProcImplementation {
                        owner: owner.clone(),
                        override_index: proc_ref.index(),
                        location: SourceLocation {
                            file: context.file_path(value.location.file).display().to_string(),
                            line: value.location.line,
                            column: value.location.column,
                        },
                        has_body: value.code.is_some() || value.body_range.is_some(),
                        parameters: value
                            .parameters
                            .iter()
                            .map(|parameter| parameter.name.to_string())
                            .collect(),
                    });
                resolver
                    .owners_by_proc_name
                    .entry(proc_name)
                    .or_default()
                    .insert(owner.clone());
            }
            if !local.is_empty() {
                resolver.local_implementations.insert(owner, local);
            }
        }

        let canonical_keys = resolver
            .local_implementations
            .iter()
            .flat_map(|(owner, procs)| {
                procs
                    .keys()
                    .map(|proc_name| (owner.clone(), proc_name.clone()))
            })
            .collect::<Vec<_>>();
        resolver.canonical_resolutions = canonical_keys
            .into_iter()
            .filter_map(|(owner, proc_name)| resolver.resolve(&owner, &proc_name).ok())
            .collect();
        resolver
    }

    pub fn resolve(
        &self,
        requested_type_path: &str,
        proc_name: &str,
    ) -> Result<ProcResolution, ProcResolutionError> {
        if !self.parents.contains_key(requested_type_path) {
            return Err(ProcResolutionError::TypeNotFound {
                requested_type_path: requested_type_path.to_owned(),
            });
        }

        let mut searched_type_chain = Vec::new();
        let mut implementations = Vec::new();
        let mut implementation_owner = None;
        let mut declaration_owner = None;
        let mut current = Some(requested_type_path);
        while let Some(owner) = current {
            searched_type_chain.push(owner.to_owned());
            if declaration_owner.is_none()
                && self
                    .declarations
                    .get(owner)
                    .is_some_and(|names| names.contains(proc_name))
            {
                declaration_owner = Some(owner.to_owned());
            }
            if let Some(local) = self
                .local_implementations
                .get(owner)
                .and_then(|procs| procs.get(proc_name))
            {
                implementation_owner.get_or_insert_with(|| owner.to_owned());
                implementations.extend(local.iter().cloned());
            }
            current = self.parents.get(owner).and_then(|parent| parent.as_deref());
        }

        let Some(implementation_owner) = implementation_owner else {
            return Err(ProcResolutionError::NotFound {
                requested_type_path: requested_type_path.to_owned(),
                proc_name: proc_name.to_owned(),
                searched_type_chain,
                same_name_candidates: self
                    .owners_by_proc_name
                    .get(proc_name)
                    .into_iter()
                    .flatten()
                    .take(MAX_SAME_NAME_CANDIDATES)
                    .cloned()
                    .collect(),
            });
        };
        let declaration_owner = declaration_owner.unwrap_or_else(|| implementation_owner.clone());
        Ok(ProcResolution {
            requested_type_path: requested_type_path.to_owned(),
            proc_name: proc_name.to_owned(),
            resolution_kind: if implementation_owner == requested_type_path {
                ProcResolutionKind::LocalImplementation
            } else {
                ProcResolutionKind::InheritedImplementation
            },
            implementation_owner,
            declaration_owner,
            implementations,
        })
    }

    pub fn resolutions(&self) -> impl Iterator<Item = &ProcResolution> {
        self.canonical_resolutions.iter()
    }
}
