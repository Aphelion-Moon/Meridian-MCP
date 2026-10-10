use crate::index::{DocumentSymbol, ImplementationHit, ReferenceHit, SymbolId};
use crate::outputs::{budget::Budget, Omissions};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
pub struct Pagination {
    pub offset: usize,
    pub limit: usize,
    pub next_cursor: Option<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct PageData<T> {
    pub count: usize,
    pub total_count: usize,
    pub detail: &'static str,
    pub pagination: Pagination,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<T>,
}
#[derive(Serialize, JsonSchema)]
pub struct DocumentSymbolsData {
    pub symbols: Vec<DocumentSymbolRow>,
    #[serde(flatten)]
    pub page: PageData<DocumentSymbolRow>,
}
#[derive(Serialize, JsonSchema)]
pub struct ImplementationsData {
    pub implementations: Vec<ImplementationRow>,
    #[serde(flatten)]
    pub page: PageData<ImplementationRow>,
}
#[derive(Serialize, JsonSchema)]
pub struct ReferencesData {
    pub references: Vec<ReferenceRow>,
    pub skipped_dynamic: usize,
    pub skipped_dynamic_scope: &'static str,
    #[serde(flatten)]
    pub page: PageData<ReferenceRow>,
}
pub type DocumentSymbolsOutput = crate::result::Success<DocumentSymbolsData>;
pub type FindImplementationsOutput = crate::result::Success<ImplementationsData>;
pub type FindReferencesOutput = crate::result::Success<ReferencesData>;

pub(crate) trait PageRow: Serialize + Default {
    fn factor(rows: &mut [Self]) -> Self;
}
pub(crate) trait ProjectRow: Serialize {
    type Output: PageRow;
    fn project(self, budget: &mut Budget) -> Self::Output;
}
fn text(value: &str, field: &str, budget: &mut Budget, omissions: &mut Omissions) -> String {
    budget.text(value, 64 * 1024, field, omissions)
}
fn symbol(value: &SymbolId, budget: &mut Budget, omissions: &mut Omissions) -> SymbolId {
    match value {
        SymbolId::Type { path } => SymbolId::Type {
            path: text(path, "symbol.path", budget, omissions).into(),
        },
        SymbolId::Var { owner, name } => SymbolId::Var {
            owner: text(owner, "symbol.owner", budget, omissions).into(),
            name: text(name, "symbol.name", budget, omissions).into(),
        },
        SymbolId::Proc {
            owner,
            name,
            override_index,
        } => SymbolId::Proc {
            owner: text(owner, "symbol.owner", budget, omissions).into(),
            name: text(name, "symbol.name", budget, omissions).into(),
            override_index: *override_index,
        },
        SymbolId::Macro { name, file, line } => SymbolId::Macro {
            name: text(name, "symbol.name", budget, omissions).into(),
            file: text(file, "symbol.file", budget, omissions).into(),
            line: *line,
        },
    }
}
#[derive(Default, Serialize, JsonSchema)]
pub struct DocumentSymbolRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<crate::index::SymbolId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::index::SymbolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_owner: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration_owner: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u16>,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
impl PageRow for DocumentSymbolRow {
    fn factor(rows: &mut [Self]) -> Self {
        let mut shared = Self::default();
        if rows.len() > 1 {
            if rows.iter().all(|row| row.id == rows[0].id) {
                shared.id = rows[0].id.take();
                for row in rows.iter_mut().skip(1) {
                    row.id = None;
                }
            }
            if rows.iter().all(|row| row.name == rows[0].name) {
                shared.name = rows[0].name.take();
                for row in rows.iter_mut().skip(1) {
                    row.name = None;
                }
            }
            if rows.iter().all(|row| row.kind == rows[0].kind) {
                shared.kind = rows[0].kind.take();
                for row in rows.iter_mut().skip(1) {
                    row.kind = None;
                }
            }
            if rows.iter().all(|row| row.owner == rows[0].owner) {
                shared.owner = rows[0].owner.take();
                for row in rows.iter_mut().skip(1) {
                    row.owner = None;
                }
            }
            if rows
                .iter()
                .all(|row| row.implementation_owner == rows[0].implementation_owner)
            {
                shared.implementation_owner = rows[0].implementation_owner.take();
                for row in rows.iter_mut().skip(1) {
                    row.implementation_owner = None;
                }
            }
            if rows
                .iter()
                .all(|row| row.declaration_owner == rows[0].declaration_owner)
            {
                shared.declaration_owner = rows[0].declaration_owner.take();
                for row in rows.iter_mut().skip(1) {
                    row.declaration_owner = None;
                }
            }
            if rows.iter().all(|row| row.file == rows[0].file) {
                shared.file = rows[0].file.take();
                for row in rows.iter_mut().skip(1) {
                    row.file = None;
                }
            }
        }
        shared
    }
}
#[derive(Default, Serialize, JsonSchema)]
pub struct ImplementationRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<crate::index::SymbolId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_in: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherited_from: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u16>,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
impl PageRow for ImplementationRow {
    fn factor(rows: &mut [Self]) -> Self {
        let mut shared = Self::default();
        if rows.len() > 1 {
            if rows.iter().all(|row| row.symbol == rows[0].symbol) {
                shared.symbol = rows[0].symbol.take();
                for row in rows.iter_mut().skip(1) {
                    row.symbol = None;
                }
            }
            if rows
                .iter()
                .all(|row| row.declared_in == rows[0].declared_in)
            {
                shared.declared_in = rows[0].declared_in.take();
                for row in rows.iter_mut().skip(1) {
                    row.declared_in = None;
                }
            }
            if rows
                .iter()
                .all(|row| row.implementation_owner == rows[0].implementation_owner)
            {
                shared.implementation_owner = rows[0].implementation_owner.take();
                for row in rows.iter_mut().skip(1) {
                    row.implementation_owner = None;
                }
            }
            if rows
                .iter()
                .all(|row| row.declaration_owner == rows[0].declaration_owner)
            {
                shared.declaration_owner = rows[0].declaration_owner.take();
                for row in rows.iter_mut().skip(1) {
                    row.declaration_owner = None;
                }
            }
            if rows
                .iter()
                .all(|row| row.inherited_from == rows[0].inherited_from)
            {
                shared.inherited_from = rows[0].inherited_from.take();
                for row in rows.iter_mut().skip(1) {
                    row.inherited_from = None;
                }
            }
            if rows.iter().all(|row| row.file == rows[0].file) {
                shared.file = rows[0].file.take();
                for row in rows.iter_mut().skip(1) {
                    row.file = None;
                }
            }
        }
        shared
    }
}
#[derive(Default, Serialize, JsonSchema)]
pub struct ReferenceRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<crate::index::SymbolId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::index::ReferenceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u16>,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
impl PageRow for ReferenceRow {
    fn factor(rows: &mut [Self]) -> Self {
        let mut shared = Self::default();
        if rows.len() > 1 {
            if rows.iter().all(|row| row.symbol == rows[0].symbol) {
                shared.symbol = rows[0].symbol.take();
                for row in rows.iter_mut().skip(1) {
                    row.symbol = None;
                }
            }
            if rows.iter().all(|row| row.kind == rows[0].kind) {
                shared.kind = rows[0].kind.take();
                for row in rows.iter_mut().skip(1) {
                    row.kind = None;
                }
            }
            if rows.iter().all(|row| row.file == rows[0].file) {
                shared.file = rows[0].file.take();
                for row in rows.iter_mut().skip(1) {
                    row.file = None;
                }
            }
        }
        shared
    }
}
impl ProjectRow for &DocumentSymbol {
    type Output = DocumentSymbolRow;
    fn project(self, budget: &mut Budget) -> Self::Output {
        let mut omissions = Omissions::default();
        let row = self;
        DocumentSymbolRow {
            id: Some(symbol(&row.id, budget, &mut omissions)),
            name: Some(text(&row.name, "name", budget, &mut omissions)),
            kind: Some(row.kind),
            owner: Some(
                row.owner
                    .as_ref()
                    .map(|v| text(v, "owner", budget, &mut omissions)),
            ),
            implementation_owner: Some(
                row.implementation_owner
                    .as_ref()
                    .map(|v| text(v, "implementation_owner", budget, &mut omissions)),
            ),
            declaration_owner: Some(
                row.declaration_owner
                    .as_ref()
                    .map(|v| text(v, "declaration_owner", budget, &mut omissions)),
            ),
            file: Some(text(&row.file, "file", budget, &mut omissions)),
            line: Some(row.line),
            column: Some(row.column),
            field_omissions: omissions,
        }
    }
}
impl ProjectRow for &ImplementationHit {
    type Output = ImplementationRow;
    fn project(self, budget: &mut Budget) -> Self::Output {
        let mut omissions = Omissions::default();
        let row = self;
        ImplementationRow {
            symbol: Some(symbol(&row.symbol, budget, &mut omissions)),
            declared_in: Some(text(
                &row.declared_in,
                "declared_in",
                budget,
                &mut omissions,
            )),
            implementation_owner: Some(text(
                &row.implementation_owner,
                "implementation_owner",
                budget,
                &mut omissions,
            )),
            declaration_owner: Some(text(
                &row.declaration_owner,
                "declaration_owner",
                budget,
                &mut omissions,
            )),
            inherited_from: Some(
                row.inherited_from
                    .as_ref()
                    .map(|v| text(v, "inherited_from", budget, &mut omissions)),
            ),
            file: Some(text(&row.file, "file", budget, &mut omissions)),
            line: Some(row.line),
            column: Some(row.column),
            field_omissions: omissions,
        }
    }
}
impl ProjectRow for &ReferenceHit {
    type Output = ReferenceRow;
    fn project(self, budget: &mut Budget) -> Self::Output {
        let mut omissions = Omissions::default();
        let row = self;
        ReferenceRow {
            symbol: Some(symbol(&row.symbol, budget, &mut omissions)),
            kind: Some(row.kind),
            file: Some(text(&row.file, "file", budget, &mut omissions)),
            line: Some(row.line),
            column: Some(row.column),
            field_omissions: omissions,
        }
    }
}
impl ProjectRow for ReferenceHit {
    type Output = ReferenceRow;
    fn project(self, budget: &mut Budget) -> Self::Output {
        (&self).project(budget)
    }
}
