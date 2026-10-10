use crate::proc_resolution::ProcResolutionKind;
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct Omissions {
    #[serde(flatten)]
    pub fields: BTreeMap<String, String>,
}
impl Omissions {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

#[derive(Serialize, JsonSchema)]
pub struct RetrievalCapabilities {
    pub lexical: LexicalCapability,
    pub dense: DenseCapability,
    pub semantic_chunk_schema_version: u32,
}
#[derive(Serialize, JsonSchema)]
pub struct LexicalCapability {
    pub status: &'static str,
    pub algorithm: &'static str,
    pub documents: usize,
}
#[derive(Serialize, JsonSchema)]
pub struct DenseCapability {
    pub status: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct ParseEnvironmentData {
    pub success: bool,
    pub reused: bool,
    pub environment: String,
    pub total_types: usize,
    pub indexed_symbols: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub state_generation: u64,
    pub spacemandmm_revision: &'static str,
    pub spacemandmm_local_patch: &'static str,
    pub spacemandmm_local_patch_sha256: &'static str,
    pub retrieval: RetrievalCapabilities,
    pub timings_ms: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}
pub type ParseEnvironmentOutput = crate::result::AnalysisOutput<ParseEnvironmentData>;

#[derive(Serialize, JsonSchema)]
pub struct TypeVariable {
    pub name: String,
    pub has_value: bool,
    pub constant: Option<String>,
    pub declared_here: bool,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
#[derive(Serialize, JsonSchema)]
pub struct TypeProcedure {
    pub name: String,
    pub parameter_count: usize,
    pub override_count: usize,
    pub declared_here: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct TypePagination {
    pub offset: usize,
    pub count: usize,
    pub total_count: usize,
    pub next_cursor: Option<String>,
    pub evaluation_complete: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct GetTypeData {
    pub path: String,
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vars: Option<Vec<TypeVariable>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procs: Option<Vec<TypeProcedure>>,
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<TypePagination>,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
pub type GetTypeOutput = crate::result::AnalysisOutput<GetTypeData>;

#[derive(Serialize, JsonSchema)]
pub struct Parameter {
    pub name: String,
    pub has_default: bool,
}
#[derive(Serialize, JsonSchema, Default)]
pub struct SourceFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_start_line: Option<Option<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_start_column: Option<Option<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_total_lines: Option<Option<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_truncated: Option<Option<bool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_boundary: Option<Option<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_origin: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line_limit: Option<usize>,
}
#[derive(Serialize, JsonSchema)]
pub struct ProcImplementation {
    pub owner: String,
    pub override_index: usize,
    pub parameters: Vec<Parameter>,
    pub documentation: String,
    pub location: String,
    pub has_body: bool,
    #[serde(flatten)]
    pub source: SourceFields,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
#[derive(Serialize, JsonSchema)]
pub struct GetProcData {
    pub name: String,
    pub type_path: String,
    pub requested_type_path: String,
    pub implementation_owner: String,
    pub declaration_owner: String,
    pub resolution_kind: ProcResolutionKind,
    pub declared: bool,
    pub overrides: Vec<ProcImplementation>,
    pub resolution_diagnostics: Vec<String>,
    pub state_generation: u64,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
pub type GetProcOutput = crate::result::AnalysisOutput<GetProcData>;
#[derive(Serialize, JsonSchema)]
pub struct GetVarData {
    pub name: String,
    pub type_path: String,
    pub declared: bool,
    pub declared_type: Option<String>,
    pub value_owner: String,
    pub declaration_owner: Option<String>,
    pub inherited: bool,
    pub documentation: String,
    pub constant: Option<String>,
    pub has_expression: bool,
    pub location: String,
    pub declaration_location: Option<String>,
    pub state_generation: u64,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
pub type GetVarOutput = crate::result::AnalysisOutput<GetVarData>;

#[derive(Serialize, JsonSchema)]
pub struct DefinitionData {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defined_in: Option<String>,
    pub file: String,
    pub line: u32,
    pub column: u16,
    pub declaration_kind: &'static str,
    pub resolved_type_owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_kind: Option<ProcResolutionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_diagnostics: Option<Vec<String>>,
    pub state_generation: u64,
    pub spacemandmm_revision: &'static str,
}
pub type GetDefinitionOutput = crate::result::AnalysisOutput<DefinitionData>;

#[derive(Serialize, JsonSchema)]
pub struct RetrievalStats {
    pub mode: &'static str,
    pub algorithm: &'static str,
    pub candidates_considered: usize,
    pub documents_scored: usize,
}
#[derive(Serialize, JsonSchema)]
pub struct SearchHit {
    pub score: f64,
    pub kind: &'static str,
    pub symbol: String,
    pub name: String,
    pub type_path: String,
    pub implementation_owner: Option<String>,
    pub declaration_owner: Option<String>,
    pub parent: Option<String>,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub docs: String,
    pub parameters: Vec<String>,
    pub override_index: Option<usize>,
    pub override_count: Option<usize>,
    #[serde(flatten)]
    pub source: SourceFields,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
#[derive(Serialize, JsonSchema)]
pub struct SearchContextData {
    pub query: String,
    pub query_terms: Vec<String>,
    pub indexed_documents: usize,
    pub count: usize,
    pub results: Vec<SearchHit>,
    pub state_generation: u64,
    pub source_origin: &'static str,
    pub source_line_limit: usize,
    pub retrieval: RetrievalStats,
    pub evaluation_complete: bool,
}
pub type SearchContextOutput = crate::result::AnalysisOutput<SearchContextData>;

#[derive(Serialize, JsonSchema)]
pub struct ListTypeRow {
    pub path: String,
    pub var_count: usize,
    pub proc_count: usize,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
#[derive(Serialize, JsonSchema)]
pub struct LegacyPagination {
    pub cursor: String,
    pub limit: usize,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct ListTypesData {
    pub count: usize,
    pub total_count: usize,
    pub types: Vec<ListTypeRow>,
    pub pagination: LegacyPagination,
}
pub type ListTypesOutput = crate::result::Success<ListTypesData>;
#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SymbolSearchRow {
    Macro {
        name: String,
        location: String,
        file: String,
        line: u32,
        column: u16,
    },
    Proc {
        name: String,
        type_path: String,
        implementation_owner: String,
        declaration_owner: String,
        resolution_kind: ProcResolutionKind,
        location: String,
    },
    Type {
        path: String,
        location: String,
    },
    Var {
        name: String,
        type_path: String,
        location: String,
    },
}
#[derive(Serialize, JsonSchema)]
pub struct SearchSymbolsData {
    pub count: usize,
    pub results: Vec<SymbolSearchRow>,
}
pub type SearchSymbolsOutput = crate::result::AnalysisOutput<SearchSymbolsData>;
#[derive(Serialize, JsonSchema)]
pub struct DiagnosticFilters {
    pub file_path: Option<String>,
    pub severity: Option<String>,
    pub component: Option<String>,
    pub rule: Option<String>,
    pub configured: Option<bool>,
}
#[derive(Serialize, JsonSchema)]
pub struct DiagnosticSummary {
    pub total: usize,
    pub by_severity: BTreeMap<String, usize>,
    pub by_component: BTreeMap<String, usize>,
    pub by_rule: BTreeMap<String, usize>,
    pub configured: usize,
    pub unconfigured: usize,
}
#[derive(Serialize, JsonSchema)]
pub struct DiagnosticRow {
    #[serde(flatten)]
    pub diagnostic: crate::analysis_snapshot::DiagnosticRecord,
    #[serde(skip_serializing_if = "Omissions::is_empty")]
    pub field_omissions: Omissions,
}
#[derive(Serialize, JsonSchema)]
pub struct CheckErrorsData {
    pub filters: DiagnosticFilters,
    pub summary: DiagnosticSummary,
    pub count: usize,
    pub total_count: usize,
    pub diagnostics: Vec<DiagnosticRow>,
    pub pagination: LegacyPagination,
}
pub type CheckErrorsOutput = crate::result::Success<CheckErrorsData>;
