//! Strict wire requests. Handler inputs are these requests or domain values moved from them.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
mod deserializer;

const MIN_POSITIVE_INTEGER: u64 = 1;
const PARSE_ENVIRONMENT_TIMEOUT_MS_MAX: u64 = 1800000;
const MIN_NONNEGATIVE_INTEGER: u64 = 0;
const CHECK_FIXTURE_SYNC_ISSUE_LIMIT_MAX: u64 = 200;
const NATIVE_EVIDENCE_COMPARE_RUNS_MIN: usize = 2;
const NATIVE_EVIDENCE_COMPARE_RUNS_MAX: usize = 20;
const MAX_SOURCE_LINES: u64 = crate::source::MAX_SOURCE_LINES as u64;
const LIST_TYPES_LIMIT_MAX: u64 = 500;
const GET_TYPE_LIMIT_MAX: u64 = crate::outputs::budget::MEMBER_WORK as u64;
const SEARCH_SYMBOLS_LIMIT_MAX: u64 = 200;
const SEARCH_CONTEXT_LIMIT_MAX: u64 = 50;
const CHECK_ERRORS_FILE_PATH_MIN: usize = 1;
const CHECK_ERRORS_RULE_MIN: usize = 1;
const CHECK_ERRORS_LIMIT_MAX: u64 = 100;
const DOCUMENT_SYMBOLS_LIMIT_MAX: u64 = 20000;
const FIND_REFERENCES_MEMBER_NAME_MIN: usize = 1;
const FIND_REFERENCES_LIMIT_MAX: u64 = 10000;
const FIND_IMPLEMENTATIONS_MEMBER_NAME_MIN: usize = 1;
const FIND_IMPLEMENTATIONS_LIMIT_MAX: u64 = 10000;
const OUTPUT_MAX_BYTES_MAX: u64 = 65536;
const DIAGNOSTIC_LIMIT_MAX: u64 = 200;
const COMPILE_TIMEOUT_MS_MAX: u64 = 1800000;
const COMPILE_IDLE_TIMEOUT_MS_MIN: u64 = 1000;
const COMPILE_IDLE_TIMEOUT_MS_MAX: u64 = 900000;
const RIFT_COMPILE_TIMEOUT_MS_MIN: u64 = 1000;
const RIFT_COMPILE_TIMEOUT_MS_MAX: u64 = 1800000;
const RIFT_COMPILE_IDLE_TIMEOUT_MS_MIN: u64 = 1000;
const RIFT_COMPILE_IDLE_TIMEOUT_MS_MAX: u64 = 900000;
const MAX_PROTOCOL_U32: u64 = 4294967295;
const MINIMUM_SIMILARITY_MIN: f64 = 0.9;
const MINIMUM_SIMILARITY_MAX: f64 = 1.0;
const FIND_DMI_DUPLICATES_MAX_MATCHES_MAX: u64 = 10000;
const AUDIT_ICONS_MAX_MATCHES_MAX: u64 = 10000;
const RENDER_MAP_Z_LEVEL_MAX: u64 = 4294967295;
const DIFF_MAPS_LIMIT_MAX: u64 = 100000;
const RENDER_MAPS_FILES_ITEM_CHUNKS_MAX: usize = 512;
const RENDER_MAPS_FILES_MAX: usize = 128;
const DEBUG_LAUNCH_STARTUP_TIMEOUT_MS_MAX: u64 = 60000;
const DEBUG_SET_BREAKPOINTS_BREAKPOINTS_ITEM_LINE_MAX: u64 = 4294967295;
const MAX_BREAKPOINT_CONDITION_BYTES: usize = 4096;
const DEBUG_SET_BREAKPOINTS_BREAKPOINTS_MAX: usize = 10000;
const DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MIN: usize = 1;
const DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MAX: usize = 4096;
const DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_MAX: usize = 10000;
const DEBUG_STACK_TRACE_COUNT_MAX: u64 = 1000;
const PROTOCOL_I32_MIN: i64 = -2147483648;
const PROTOCOL_I32_MAX: i64 = 2147483647;
const DEBUG_EVALUATE_EXPRESSION_MAX: usize = 16384;
const DEBUG_WAIT_FOR_EVENT_TIMEOUT_MS_MAX: u64 = 300000;
const RUN_PORT_MAX: u64 = 65535;
const RUN_STARTUP_TIMEOUT_MS_MAX: u64 = 300000;
const WAIT_FOR_OUTPUT_TIMEOUT_MS_MAX: u64 = 300000;
const TOPIC_TIMEOUT_MS_MAX: u64 = 60000;
const MAX_TOPIC_BYTES: usize = u16::MAX as usize - 6;
const MAX_TOPIC_CHARACTERS: usize = MAX_TOPIC_BYTES + 1;
const MAX_WORKLOAD_VALUE_BYTES: usize = crate::tracy_experiment::MAX_IDENTITY_VALUE_BYTES;
const TRACY_LAUNCH_FEATURE_SET_MAX: usize = crate::tracy_experiment::MAX_FEATURES;
const TRACY_LAUNCH_GAME_PORT_MAX: u64 = 65535;
const TRACY_LAUNCH_STARTUP_TIMEOUT_MS_MIN: u64 = 1000;
const TRACY_LAUNCH_STARTUP_TIMEOUT_MS_MAX: u64 = 60000;
const TRACY_LAUNCH_INITIALIZATION_TIMEOUT_MS_MAX: u64 = 300000;
const TRACY_CAPTURE_FEATURE_SET_MAX: usize = crate::tracy_experiment::MAX_FEATURES;
const TRACY_CAPTURE_DURATION_MS_MAX: u64 = 300000;
const TRACY_CAPTURE_MEMORY_LIMIT_MB_MIN: u64 = 16;
const TRACY_CAPTURE_MEMORY_LIMIT_MB_MAX: u64 = 4096;
const TRACY_CAPTURE_PHASE_MIN: usize = 1;
const TRACY_CAPTURE_PHASE_MAX: usize = 64;
const TRACY_HOTSPOTS_LIMIT_MAX: u64 = 1000;
const TRACY_ZONE_NAME_MAX: usize = 4096;
const TRACY_ZONE_LIMIT_MAX: u64 = 1000;
const TRACY_COMPARE_LIMIT_MAX: u64 = 1000;
const TRACY_CONTROL_STATS_TRACE_PATHS_MIN: usize = 3;
const TRACY_CONTROL_STATS_TRACE_PATHS_MAX: usize = 20;
const TRACY_CONTROL_STATS_ZONE_KEYS_MAX: usize = 32;

fn range<T: PartialOrd>(
    field: &str,
    value: T,
    minimum: Option<T>,
    maximum: Option<T>,
) -> Result<(), InputError> {
    if minimum.is_some_and(|min| value < min) || maximum.is_some_and(|max| value > max) {
        Err(InputError::new(
            field,
            "request field is outside its advertised bounds",
        ))
    } else {
        Ok(())
    }
}

fn text(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: Option<usize>,
) -> Result<(), InputError> {
    range(field, value.len(), Some(minimum), maximum)?;
    if value.contains('\0') {
        return Err(InputError::new(
            field,
            "request string contains a null byte",
        ));
    }
    Ok(())
}

pub(crate) trait Request: serde::de::DeserializeOwned + JsonSchema {
    fn validate(&self) -> Result<(), InputError>;
}

#[derive(Debug)]
pub(crate) struct InputError {
    pub field: String,
    pub reason: &'static str,
}
impl InputError {
    pub fn new(field: impl Into<String>, reason: &'static str) -> Self {
        let mut field = field.into();
        while field.len() > 256 {
            field.pop();
        }
        Self { field, reason }
    }
    pub fn result(&self) -> crate::mcp::ToolResult {
        self.result_with_code("invalid_input")
    }
    pub fn result_with_code(&self, code: &str) -> crate::mcp::ToolResult {
        crate::mcp::ToolResult::error(serde_json::json!({"code":code,"message":self.reason,
            "recovery":"Use the advertised fields, types and bounds.","details":{"field":self.field}}).to_string())
    }
}
impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid_input at {}: {}", self.field, self.reason)
    }
}
impl std::error::Error for InputError {}

// Missing optional fields are distinct from explicit null. Memory window fields
// intentionally retain Option's nullable wire semantics in their domain model.
pub(crate) fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
pub(crate) fn decode<T: Request>(value: Value) -> Result<T, InputError> {
    use serde::de::IntoDeserializer;
    let request: T =
        serde_path_to_error::deserialize(deserializer::ObjectRequests(value.into_deserializer()))
            .map_err(|error| {
            let mut field = error.path().to_string();
            // Serde associates an unknown field with its enclosing object. Extract
            // only the key, never the caller's scalar value or the complete error.
            let detail = error.inner().to_string();
            if let Some(key) = detail
                .strip_prefix("unknown field `")
                .or_else(|| detail.strip_prefix("missing field `"))
                .and_then(|s| s.split('`').next())
            {
                if field == "." {
                    field.clear();
                }
                if field.rsplit('.').next() != Some(key) {
                    if !field.is_empty() {
                        field.push('.');
                    }
                    field.push_str(key);
                }
            }
            InputError::new(
                field,
                "request field has an invalid type, enum value, or name",
            )
        })?;
    request.validate()?;
    Ok(request)
}
pub(crate) fn schema<T: JsonSchema>() -> Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true);
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
        .expect("request schema")
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerStatusParams {}
impl Request for ServerStatusParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParseEnvironmentParams {
    /// Path to the .dme environment file
    pub dme_path: String,
    /// Reparse even when the active snapshot already matches this environment on disk (default false).
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub force: Option<bool>,
    /// Total request budget including admission, blocking scheduling, reuse validation, parsing, and installation (default 600000). Unfinished workers retain exclusive admission after timeout or cancellation.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = PARSE_ENVIRONMENT_TIMEOUT_MS_MAX))]
    pub timeout_ms: Option<u64>,
}
impl Request for ParseEnvironmentParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dme_path", &self.dme_path, 0, None)?;
        if let Some(value) = self.timeout_ms {
            range(
                "timeout_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(PARSE_ENVIRONMENT_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckFixtureSyncParams {
    pub fixture_manifest_path: String,
    /// Maximum returned issues; validation and total counts remain complete. Argument and serialized byte budgets also apply.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = CHECK_FIXTURE_SYNC_ISSUE_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(50)))]
    pub issue_limit: Option<u64>,
}
impl Request for CheckFixtureSyncParams {
    fn validate(&self) -> Result<(), InputError> {
        text(
            "fixture_manifest_path",
            &self.fixture_manifest_path,
            0,
            None,
        )?;
        if let Some(value) = self.issue_limit {
            range(
                "issue_limit",
                value,
                None,
                Some(CHECK_FIXTURE_SYNC_ISSUE_LIMIT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeEvidenceCompareParams {
    #[schemars(length(min = NATIVE_EVIDENCE_COMPARE_RUNS_MIN, max = NATIVE_EVIDENCE_COMPARE_RUNS_MAX))]
    pub runs: Vec<crate::native_evidence::model::NativeEvidenceRequest>,
}
impl Request for NativeEvidenceCompareParams {
    fn validate(&self) -> Result<(), InputError> {
        if !(NATIVE_EVIDENCE_COMPARE_RUNS_MIN..=NATIVE_EVIDENCE_COMPARE_RUNS_MAX)
            .contains(&self.runs.len())
        {
            return Err(InputError::new(
                "runs",
                "runs must contain 2 through 20 requests",
            ));
        }
        for (i, request) in self.runs.iter().enumerate() {
            request
                .validate()
                .map_err(|e| InputError::new(format!("runs[{i}].{}", e.field), e.reason))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTypeParams {
    /// The type path (e.g., '/obj/item', '/mob/living')
    pub type_path: String,
    /// Omitted selects all existing full sections. Selection never changes scalar identity fields.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<GetTypeSection>", length(min = 1, max = 4), extend("uniqueItems" = true))]
    pub sections: Option<Vec<GetTypeSection>>,
    /// Optional member count; omission fills the aggregate bounded response budget.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64", range(min = MIN_POSITIVE_INTEGER, max = GET_TYPE_LIMIT_MAX))]
    pub limit: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String", length(min = 1, max = 256))]
    pub cursor: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DocumentSymbolsDetail")]
    pub detail: Option<DocumentSymbolsDetail>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for GetTypeParams {
    fn validate(&self) -> Result<(), InputError> {
        text("type_path", &self.type_path, 0, None)?;
        if let Some(limit) = self.limit {
            range(
                "limit",
                limit,
                Some(MIN_POSITIVE_INTEGER),
                Some(GET_TYPE_LIMIT_MAX),
            )?;
        }
        if let Some(cursor) = &self.cursor {
            text("cursor", cursor, 1, Some(256))?;
        }
        if let Some(sections) = &self.sections {
            if sections.is_empty()
                || sections.len() > 4
                || sections
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != sections.len()
            {
                return Err(InputError::new(
                    "sections",
                    "select one to four distinct type sections",
                ));
            }
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, Ord, PartialEq, PartialOrd,
)]
#[serde(rename_all = "snake_case")]
pub enum GetTypeSection {
    Documentation,
    Vars,
    Procs,
    Children,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetProcParams {
    /// The type path containing the proc
    pub type_path: String,
    /// Name of the procedure
    pub proc_name: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(true)))]
    pub include_source: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = MAX_SOURCE_LINES))]
    #[schemars(extend("default" = serde_json::json!(80)))]
    pub max_source_lines: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for GetProcParams {
    fn validate(&self) -> Result<(), InputError> {
        text("type_path", &self.type_path, 0, None)?;
        text("proc_name", &self.proc_name, 0, None)?;
        if let Some(value) = self.max_source_lines {
            range(
                "max_source_lines",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(MAX_SOURCE_LINES),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetVarParams {
    /// The type path containing the variable
    pub type_path: String,
    /// Name of the variable
    pub var_name: String,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for GetVarParams {
    fn validate(&self) -> Result<(), InputError> {
        text("type_path", &self.type_path, 0, None)?;
        text("var_name", &self.var_name, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTypesParams {
    /// Optional path prefix to filter types (e.g., '/obj')
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub prefix: Option<String>,
    /// Maximum depth to traverse (default: unlimited)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    pub max_depth: Option<u64>,
    /// Maximum types returned in one page
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = LIST_TYPES_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    /// Opaque next_cursor from a previous response
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^v2:[0-9]+:[0-9a-f]{16}$"))]
    pub cursor: Option<String>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for ListTypesParams {
    fn validate(&self) -> Result<(), InputError> {
        if let Some(value) = &self.prefix {
            text("prefix", value, 0, None)?;
        }
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(LIST_TYPES_LIMIT_MAX),
            )?;
        }
        if let Some(value) = &self.cursor {
            if value.len() > 64 || !value.starts_with("v2:") || value.contains('\0') {
                return Err(InputError::new(
                    "cursor",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum SearchSymbolsKind {
    #[serde(rename = "type")]
    Type,
    #[serde(rename = "proc")]
    Proc,
    #[serde(rename = "var")]
    Var,
    #[serde(rename = "macro")]
    Macro,
    #[serde(rename = "all")]
    All,
}
impl SearchSymbolsKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Proc => "proc",
            Self::Var => "var",
            Self::Macro => "macro",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchSymbolsParams {
    /// Search query (supports partial matches)
    pub query: String,
    /// Kind of symbol to search for (default: all)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SearchSymbolsKind")]
    pub kind: Option<SearchSymbolsKind>,
    /// Maximum number of results (default: 50)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = SEARCH_SYMBOLS_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(50)))]
    pub limit: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for SearchSymbolsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("query", &self.query, 0, None)?;
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(SEARCH_SYMBOLS_LIMIT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum SearchContextKind {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "type")]
    Type,
    #[serde(rename = "proc")]
    Proc,
    #[serde(rename = "var")]
    Var,
}
impl SearchContextKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Type => "type",
            Self::Proc => "proc",
            Self::Var => "var",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchContextParams {
    /// Natural-language behavior, exact symbol, or identifier terms to find
    pub query: String,
    /// Optional symbol kind filter (default: all)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SearchContextKind")]
    pub kind: Option<SearchContextKind>,
    /// Optional canonical type-path prefix, such as /turf/open
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub type_prefix: Option<String>,
    /// Optional case-insensitive source-path substring
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub file_filter: Option<String>,
    /// Maximum ranked results (default: 10)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = SEARCH_CONTEXT_LIMIT_MAX))]
    pub limit: Option<u64>,
    /// Include physical snapshot excerpts with boundaries and truncation (default: true); false returns metadata only
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub include_source: Option<bool>,
    /// Maximum source lines per result, up to the 200-line snapshot budget (default: 40)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = MAX_SOURCE_LINES))]
    pub max_source_lines: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for SearchContextParams {
    fn validate(&self) -> Result<(), InputError> {
        if self.query.trim().is_empty() {
            return Err(InputError::new("query", "query must not be empty"));
        }
        for (field, value) in [
            ("type_prefix", &self.type_prefix),
            ("file_filter", &self.file_filter),
        ] {
            if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
                return Err(InputError::new(field, "filter must not be empty"));
            }
        }
        text("query", &self.query, 0, None)?;
        if let Some(value) = &self.type_prefix {
            text("type_prefix", value, 0, None)?;
        }
        if let Some(value) = &self.file_filter {
            text("file_filter", value, 0, None)?;
        }
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(SEARCH_CONTEXT_LIMIT_MAX),
            )?;
        }
        if let Some(value) = self.max_source_lines {
            range(
                "max_source_lines",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(MAX_SOURCE_LINES),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum CheckErrorsSeverity {
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "hint")]
    Hint,
}
impl CheckErrorsSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum CheckErrorsComponent {
    #[serde(rename = "parser")]
    Parser,
    #[serde(rename = "dreamchecker")]
    Dreamchecker,
}
impl CheckErrorsComponent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parser => "parser",
            Self::Dreamchecker => "dreamchecker",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckErrorsParams {
    /// Only include diagnostics whose normalized source path contains this value. Both slash styles are accepted.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(length(min = CHECK_ERRORS_FILE_PATH_MIN))]
    pub file_path: Option<String>,
    /// Only include diagnostics with this severity.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CheckErrorsSeverity")]
    pub severity: Option<CheckErrorsSeverity>,
    /// Only include diagnostics emitted by this SpacemanDMM component.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CheckErrorsComponent")]
    pub component: Option<CheckErrorsComponent>,
    /// Only include diagnostics with this exact rule identifier.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(length(min = CHECK_ERRORS_RULE_MIN))]
    pub rule: Option<String>,
    /// Only include diagnostics whose rule is or is not explicitly configured in SpacemanDMM.toml.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub configured: Option<bool>,
    /// Opaque continuation cursor returned by a previous call using the same filters and analysis generation.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^v2:[0-9]+:[0-9a-f]{16}$"))]
    pub cursor: Option<String>,
    /// Maximum diagnostics returned in this page. Summary counts cover every matching diagnostic.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = CHECK_ERRORS_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(50)))]
    pub limit: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for CheckErrorsParams {
    fn validate(&self) -> Result<(), InputError> {
        if let Some(value) = &self.file_path {
            text("file_path", value, CHECK_ERRORS_FILE_PATH_MIN, None)?;
        }
        if let Some(value) = &self.rule {
            text("rule", value, CHECK_ERRORS_RULE_MIN, None)?;
        }
        if let Some(value) = &self.cursor {
            if value.len() > 64 || !value.starts_with("v2:") || value.contains('\0') {
                return Err(InputError::new(
                    "cursor",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(CHECK_ERRORS_LIMIT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetDefinitionParams {
    /// The type path
    pub type_path: String,
    /// Optional: name of var or proc to find definition of
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub member_name: Option<String>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for GetDefinitionParams {
    fn validate(&self) -> Result<(), InputError> {
        text("type_path", &self.type_path, 0, None)?;
        if let Some(value) = &self.member_name {
            text("member_name", value, 0, None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum DocumentSymbolsDetail {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "compact")]
    Compact,
}
impl DocumentSymbolsDetail {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentSymbolsParams {
    pub file_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = DOCUMENT_SYMBOLS_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    /// Opaque next_cursor from the same query and parsed snapshot.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub cursor: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DocumentSymbolsDetail")]
    #[schemars(extend("default" = serde_json::json!("full")))]
    pub detail: Option<DocumentSymbolsDetail>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for DocumentSymbolsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("file_path", &self.file_path, 0, None)?;
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(DOCUMENT_SYMBOLS_LIMIT_MAX),
            )?;
        }
        if let Some(value) = &self.cursor {
            text("cursor", value, 0, None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum FindReferencesKind {
    #[serde(rename = "call")]
    Call,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "type_path")]
    TypePath,
    #[serde(rename = "macro_expansion")]
    MacroExpansion,
    #[serde(rename = "declaration")]
    Declaration,
}
impl FindReferencesKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Read => "read",
            Self::Write => "write",
            Self::TypePath => "type_path",
            Self::MacroExpansion => "macro_expansion",
            Self::Declaration => "declaration",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum FindReferencesDetail {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "compact")]
    Compact,
}
impl FindReferencesDetail {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindReferencesParams {
    pub type_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(length(min = FIND_REFERENCES_MEMBER_NAME_MIN))]
    pub member_name: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "FindReferencesKind")]
    pub kind: Option<FindReferencesKind>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub include_declaration: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = FIND_REFERENCES_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    /// Opaque next_cursor from the same query and parsed snapshot.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub cursor: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "FindReferencesDetail")]
    #[schemars(extend("default" = serde_json::json!("full")))]
    pub detail: Option<FindReferencesDetail>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for FindReferencesParams {
    fn validate(&self) -> Result<(), InputError> {
        text("type_path", &self.type_path, 0, None)?;
        if let Some(value) = &self.member_name {
            text("member_name", value, FIND_REFERENCES_MEMBER_NAME_MIN, None)?;
        }
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(FIND_REFERENCES_LIMIT_MAX),
            )?;
        }
        if let Some(value) = &self.cursor {
            text("cursor", value, 0, None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum FindImplementationsDetail {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "compact")]
    Compact,
}
impl FindImplementationsDetail {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindImplementationsParams {
    pub type_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(length(min = FIND_IMPLEMENTATIONS_MEMBER_NAME_MIN))]
    pub member_name: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = FIND_IMPLEMENTATIONS_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    /// Opaque next_cursor from the same query and parsed snapshot.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub cursor: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "FindImplementationsDetail")]
    #[schemars(extend("default" = serde_json::json!("full")))]
    pub detail: Option<FindImplementationsDetail>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for FindImplementationsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("type_path", &self.type_path, 0, None)?;
        if let Some(value) = &self.member_name {
            text(
                "member_name",
                value,
                FIND_IMPLEMENTATIONS_MEMBER_NAME_MIN,
                None,
            )?;
        }
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(FIND_IMPLEMENTATIONS_LIMIT_MAX),
            )?;
        }
        if let Some(value) = &self.cursor {
            text("cursor", value, 0, None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateDocsParams {
    pub output_directory: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub overwrite: Option<bool>,
    /// Include bounded stdout/stderr tails (default: true)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub include_output: Option<bool>,
    /// Maximum UTF-8 bytes per returned stream (default: 8192); JSON byte budget also applies
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = OUTPUT_MAX_BYTES_MAX))]
    pub output_max_bytes: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for GenerateDocsParams {
    fn validate(&self) -> Result<(), InputError> {
        if self.output_directory.is_empty() {
            return Err(InputError::new(
                "output_directory",
                "output_directory must not be empty",
            ));
        }
        text("output_directory", &self.output_directory, 0, None)?;
        if let Some(value) = self.output_max_bytes {
            range(
                "output_max_bytes",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(OUTPUT_MAX_BYTES_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompileParams {
    /// Path to the .dme file to compile
    pub dme_path: String,
    /// Optional path to the DreamMaker executable
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub compiler_path: Option<String>,
    /// Include bounded stdout/stderr tails; false omits raw output, retaining diagnostics and build evidence
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(true)))]
    pub include_output: Option<bool>,
    /// Maximum returned UTF-8 bytes per output stream; JSON escaping may reduce this further
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = OUTPUT_MAX_BYTES_MAX))]
    #[schemars(extend("default" = serde_json::json!(8192)))]
    pub output_max_bytes: Option<u64>,
    /// Maximum rows per severity, also subject to byte budgets; counts cover all parsed captured output
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = DIAGNOSTIC_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(50)))]
    pub diagnostic_limit: Option<u64>,
    /// Directory used to resolve a relative DME path and run the compiler; defaults to the DME directory
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub working_directory: Option<String>,
    /// Optional preprocessor defines, with or without the -D prefix
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    pub defines: Option<Vec<String>>,
    /// Compiler timeout in milliseconds (default: 600000, capped at 1800000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = COMPILE_TIMEOUT_MS_MAX))]
    pub timeout_ms: Option<u64>,
    /// Fail if DreamMaker produces no output and consumes no CPU for this long (default: 45000, capped at 900000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = COMPILE_IDLE_TIMEOUT_MS_MIN, max = COMPILE_IDLE_TIMEOUT_MS_MAX))]
    pub idle_timeout_ms: Option<u64>,
    /// Request best-effort endpoint observation (default: false)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub capture_network: Option<bool>,
    /// Optional contained declarative fixture manifest
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub fixture_manifest_path: Option<String>,
}
impl Request for CompileParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dme_path", &self.dme_path, 0, None)?;
        if let Some(value) = &self.compiler_path {
            text("compiler_path", value, 0, None)?;
        }
        if let Some(value) = self.output_max_bytes {
            range(
                "output_max_bytes",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(OUTPUT_MAX_BYTES_MAX),
            )?;
        }
        if let Some(value) = self.diagnostic_limit {
            range("diagnostic_limit", value, None, Some(DIAGNOSTIC_LIMIT_MAX))?;
        }
        if let Some(value) = &self.working_directory {
            text("working_directory", value, 0, None)?;
        }
        for (i, value) in self.defines.iter().flatten().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("defines[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        if let Some(value) = self.timeout_ms {
            range(
                "timeout_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(COMPILE_TIMEOUT_MS_MAX),
            )?;
        }
        if let Some(value) = self.idle_timeout_ms {
            range(
                "idle_timeout_ms",
                value,
                Some(COMPILE_IDLE_TIMEOUT_MS_MIN),
                Some(COMPILE_IDLE_TIMEOUT_MS_MAX),
            )?;
        }
        if let Some(value) = &self.fixture_manifest_path {
            text("fixture_manifest_path", value, 0, None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum RiftCompileNetworkMode {
    #[serde(rename = "offline")]
    Offline,
    #[serde(rename = "allow")]
    Allow,
}
impl RiftCompileNetworkMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Allow => "allow",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RiftCompileParams {
    /// Dependency network mode (default: offline)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "RiftCompileNetworkMode")]
    pub network_mode: Option<RiftCompileNetworkMode>,
    /// Wall timeout in milliseconds (default: 1800000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = RIFT_COMPILE_TIMEOUT_MS_MIN, max = RIFT_COMPILE_TIMEOUT_MS_MAX))]
    pub timeout_ms: Option<u64>,
    /// No-output and no-CPU timeout in milliseconds (default: 120000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = RIFT_COMPILE_IDLE_TIMEOUT_MS_MIN, max = RIFT_COMPILE_IDLE_TIMEOUT_MS_MAX))]
    pub idle_timeout_ms: Option<u64>,
    /// Request best-effort endpoint observation (default: false)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub capture_network: Option<bool>,
    /// Remove only canonical root build artifacts before building (default: false)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub force_rebuild: Option<bool>,
    /// Include bounded stdout/stderr tails (default: true)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub include_output: Option<bool>,
    /// Maximum UTF-8 bytes per returned stream (default: 8192); JSON byte budget also applies
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = OUTPUT_MAX_BYTES_MAX))]
    pub output_max_bytes: Option<u64>,
    /// Maximum returned error lines (default: 50); full error count is preserved
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = DIAGNOSTIC_LIMIT_MAX))]
    pub diagnostic_limit: Option<u64>,
    /// Optional contained declarative fixture manifest
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub fixture_manifest_path: Option<String>,
}
impl Request for RiftCompileParams {
    fn validate(&self) -> Result<(), InputError> {
        if let Some(value) = self.timeout_ms {
            range(
                "timeout_ms",
                value,
                Some(RIFT_COMPILE_TIMEOUT_MS_MIN),
                Some(RIFT_COMPILE_TIMEOUT_MS_MAX),
            )?;
        }
        if let Some(value) = self.idle_timeout_ms {
            range(
                "idle_timeout_ms",
                value,
                Some(RIFT_COMPILE_IDLE_TIMEOUT_MS_MIN),
                Some(RIFT_COMPILE_IDLE_TIMEOUT_MS_MAX),
            )?;
        }
        if let Some(value) = self.output_max_bytes {
            range(
                "output_max_bytes",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(OUTPUT_MAX_BYTES_MAX),
            )?;
        }
        if let Some(value) = self.diagnostic_limit {
            range("diagnostic_limit", value, None, Some(DIAGNOSTIC_LIMIT_MAX))?;
        }
        if let Some(value) = &self.fixture_manifest_path {
            text("fixture_manifest_path", value, 0, None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DmiInfoParams {
    pub dmi_path: String,
}
impl Request for DmiInfoParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dmi_path", &self.dmi_path, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompareDmiStatesParams {
    pub left_dmi_path: String,
    pub left_state: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub left_duplicate_index: Option<u64>,
    pub right_dmi_path: String,
    pub right_state: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub right_duplicate_index: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "f64")]
    #[schemars(range(min = MINIMUM_SIMILARITY_MIN, max = MINIMUM_SIMILARITY_MAX))]
    pub minimum_similarity: Option<f64>,
}
impl Request for CompareDmiStatesParams {
    fn validate(&self) -> Result<(), InputError> {
        text("left_dmi_path", &self.left_dmi_path, 0, None)?;
        text("left_state", &self.left_state, 0, None)?;
        if let Some(value) = self.left_duplicate_index {
            range("left_duplicate_index", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        text("right_dmi_path", &self.right_dmi_path, 0, None)?;
        text("right_state", &self.right_state, 0, None)?;
        if let Some(value) = self.right_duplicate_index {
            range("right_duplicate_index", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        if let Some(value) = self.minimum_similarity {
            if !(MINIMUM_SIMILARITY_MIN..=MINIMUM_SIMILARITY_MAX).contains(&value)
                || !value.is_finite()
            {
                return Err(InputError::new(
                    "minimum_similarity",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindDmiDuplicatesParams {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub scope_path: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub include_glob: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "f64")]
    #[schemars(range(min = MINIMUM_SIMILARITY_MIN, max = MINIMUM_SIMILARITY_MAX))]
    pub minimum_similarity: Option<f64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub include_frame_matches: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = FIND_DMI_DUPLICATES_MAX_MATCHES_MAX))]
    pub max_matches: Option<u64>,
}
impl Request for FindDmiDuplicatesParams {
    fn validate(&self) -> Result<(), InputError> {
        if let Some(value) = &self.scope_path {
            text("scope_path", value, 0, None)?;
        }
        if let Some(value) = &self.include_glob {
            text("include_glob", value, 0, None)?;
        }
        if let Some(value) = self.minimum_similarity {
            if !(MINIMUM_SIMILARITY_MIN..=MINIMUM_SIMILARITY_MAX).contains(&value)
                || !value.is_finite()
            {
                return Err(InputError::new(
                    "minimum_similarity",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        if let Some(value) = self.max_matches {
            range(
                "max_matches",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(FIND_DMI_DUPLICATES_MAX_MATCHES_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditIconsParams {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub scope_path: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub include_glob: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "f64")]
    #[schemars(range(min = MINIMUM_SIMILARITY_MIN, max = MINIMUM_SIMILARITY_MAX))]
    pub minimum_similarity: Option<f64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub include_unused: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = AUDIT_ICONS_MAX_MATCHES_MAX))]
    pub max_matches: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for AuditIconsParams {
    fn validate(&self) -> Result<(), InputError> {
        if let Some(value) = &self.scope_path {
            text("scope_path", value, 0, None)?;
        }
        if let Some(value) = &self.include_glob {
            text("include_glob", value, 0, None)?;
        }
        if let Some(value) = self.minimum_similarity {
            if !(MINIMUM_SIMILARITY_MIN..=MINIMUM_SIMILARITY_MAX).contains(&value)
                || !value.is_finite()
            {
                return Err(InputError::new(
                    "minimum_similarity",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        if let Some(value) = self.max_matches {
            range(
                "max_matches",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(AUDIT_ICONS_MAX_MATCHES_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum ExtractDmiKind {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "png")]
    Png,
    #[serde(rename = "gif")]
    Gif,
    #[serde(rename = "contact_sheet")]
    ContactSheet,
    #[serde(rename = "frame")]
    Frame,
}
impl ExtractDmiKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::ContactSheet => "contact_sheet",
            Self::Frame => "frame",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum ExtractDmiDirection {
    #[serde(rename = "north")]
    North,
    #[serde(rename = "south")]
    South,
    #[serde(rename = "east")]
    East,
    #[serde(rename = "west")]
    West,
    #[serde(rename = "northeast")]
    Northeast,
    #[serde(rename = "northwest")]
    Northwest,
    #[serde(rename = "southeast")]
    Southeast,
    #[serde(rename = "southwest")]
    Southwest,
}
impl ExtractDmiDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::Northeast => "northeast",
            Self::Northwest => "northwest",
            Self::Southeast => "southeast",
            Self::Southwest => "southwest",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractDmiParams {
    pub dmi_path: String,
    pub state: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub duplicate_index: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ExtractDmiKind")]
    pub kind: Option<ExtractDmiKind>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ExtractDmiDirection")]
    pub direction: Option<ExtractDmiDirection>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub frame: Option<u64>,
    pub output_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub overwrite: Option<bool>,
}
impl Request for ExtractDmiParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dmi_path", &self.dmi_path, 0, None)?;
        text("state", &self.state, 0, None)?;
        if let Some(value) = self.duplicate_index {
            range("duplicate_index", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        if let Some(value) = self.frame {
            range("frame", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        text("output_path", &self.output_path, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderMapParams {
    /// Path to the .dmm map file
    pub dmm_path: String,
    /// Z-level to render (default: 1)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = RENDER_MAP_Z_LEVEL_MAX))]
    pub z_level: Option<u64>,
    /// Path to save the PNG (default: same as dmm with .png extension)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub output_path: Option<String>,
    /// Replace an existing output only when explicitly true (default: false)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub overwrite: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "[Coordinate; 3]")]
    pub min: Option<[Coordinate; 3]>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "[Coordinate; 3]")]
    pub max: Option<[Coordinate; 3]>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    pub enable_passes: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    pub disable_passes: Option<Vec<String>>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for RenderMapParams {
    fn validate(&self) -> Result<(), InputError> {
        render_coordinates(self.min.as_ref(), self.max.as_ref())?;
        crate::tools::map::pass_selection(
            self.enable_passes.as_deref().unwrap_or_default(),
            self.disable_passes.as_deref().unwrap_or_default(),
        )
        .map_err(|_| InputError::new("enable_passes", "unknown or conflicting render passes"))?;
        text("dmm_path", &self.dmm_path, 0, None)?;
        if let Some(value) = self.z_level {
            range(
                "z_level",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(RENDER_MAP_Z_LEVEL_MAX),
            )?;
        }
        if let Some(value) = &self.output_path {
            text("output_path", value, 0, None)?;
        }
        for (i, value) in self.enable_passes.iter().flatten().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("enable_passes[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        for (i, value) in self.disable_passes.iter().flatten().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("disable_passes[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffMapsParams {
    pub left_dmm_path: String,
    pub right_dmm_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = DIFF_MAPS_LIMIT_MAX))]
    pub limit: Option<u64>,
}
impl Request for DiffMapsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("left_dmm_path", &self.left_dmm_path, 0, None)?;
        text("right_dmm_path", &self.right_dmm_path, 0, None)?;
        if let Some(value) = self.limit {
            range("limit", value, None, Some(DIFF_MAPS_LIMIT_MAX))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRenderPassesParams {}
impl Request for ListRenderPassesParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderMapsFilesItemChunksItem {
    pub output_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER))]
    pub z_level: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "[Coordinate; 3]")]
    pub min: Option<[Coordinate; 3]>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "[Coordinate; 3]")]
    pub max: Option<[Coordinate; 3]>,
}
impl Request for RenderMapsFilesItemChunksItem {
    fn validate(&self) -> Result<(), InputError> {
        render_coordinates(self.min.as_ref(), self.max.as_ref())?;
        text("output_path", &self.output_path, 0, None)?;
        if let Some(value) = self.z_level {
            range("z_level", value, Some(MIN_POSITIVE_INTEGER), None)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderMapsFilesItem {
    pub dmm_path: String,
    #[schemars(length(max = RENDER_MAPS_FILES_ITEM_CHUNKS_MAX))]
    pub chunks: Vec<RenderMapsFilesItemChunksItem>,
}
impl Request for RenderMapsFilesItem {
    fn validate(&self) -> Result<(), InputError> {
        text("dmm_path", &self.dmm_path, 0, None)?;
        {
            let value = &self.chunks;
            if value.len() > RENDER_MAPS_FILES_ITEM_CHUNKS_MAX {
                return Err(InputError::new(
                    "chunks",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.chunks.iter().enumerate() {
            value
                .validate()
                .map_err(|e| InputError::new(format!("chunks[{i}].{}", e.field), e.reason))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderMapsParams {
    #[schemars(length(max = RENDER_MAPS_FILES_MAX))]
    pub files: Vec<RenderMapsFilesItem>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    pub enable_passes: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    pub disable_passes: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub overwrite: Option<bool>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for RenderMapsParams {
    fn validate(&self) -> Result<(), InputError> {
        crate::tools::map::pass_selection(
            self.enable_passes.as_deref().unwrap_or_default(),
            self.disable_passes.as_deref().unwrap_or_default(),
        )
        .map_err(|_| InputError::new("enable_passes", "unknown or conflicting render passes"))?;
        if self
            .files
            .iter()
            .map(|file| file.chunks.len())
            .sum::<usize>()
            > RENDER_MAPS_FILES_ITEM_CHUNKS_MAX
        {
            return Err(InputError::new(
                "files",
                "batch exceeds the total chunk limit",
            ));
        }
        {
            let value = &self.files;
            if value.len() > RENDER_MAPS_FILES_MAX {
                return Err(InputError::new(
                    "files",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.files.iter().enumerate() {
            value
                .validate()
                .map_err(|e| InputError::new(format!("files[{i}].{}", e.field), e.reason))?;
        }
        for (i, value) in self.enable_passes.iter().flatten().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("enable_passes[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        for (i, value) in self.disable_passes.iter().flatten().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("disable_passes[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapInfoParams {
    /// Path to the .dmm map file
    pub dmm_path: String,
}
impl Request for MapInfoParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dmm_path", &self.dmm_path, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindOnMapParams {
    /// Path to the .dmm map file
    pub dmm_path: String,
    /// Type path to search for (e.g., '/obj/machinery/door')
    pub type_path: String,
}
impl Request for FindOnMapParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dmm_path", &self.dmm_path, 0, None)?;
        text("type_path", &self.type_path, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum DebugLaunchHostMode {
    #[serde(rename = "interactive")]
    Interactive,
    #[serde(rename = "headless")]
    Headless,
}
impl DebugLaunchHostMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Headless => "headless",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugLaunchParams {
    pub dmb_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DebugLaunchHostMode")]
    #[schemars(extend("default" = serde_json::json!("interactive")))]
    pub host_mode: Option<DebugLaunchHostMode>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub memory_profile: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = DEBUG_LAUNCH_STARTUP_TIMEOUT_MS_MAX))]
    pub startup_timeout_ms: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub require_verified_provenance: Option<bool>,
}
impl Request for DebugLaunchParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dmb_path", &self.dmb_path, 0, None)?;
        if let Some(value) = self.startup_timeout_ms {
            range(
                "startup_timeout_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(DEBUG_LAUNCH_STARTUP_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugStopParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugStopParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugSetBreakpointsBreakpointsItem {
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = DEBUG_SET_BREAKPOINTS_BREAKPOINTS_ITEM_LINE_MAX))]
    pub line: u64,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(length(max = MAX_BREAKPOINT_CONDITION_BYTES), extend("x-maxUtf8Bytes" = MAX_BREAKPOINT_CONDITION_BYTES))]
    /// Optional condition, limited to 4096 UTF-8 bytes.
    pub condition: Option<String>,
}
impl Request for DebugSetBreakpointsBreakpointsItem {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = self.line;
            range(
                "line",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(DEBUG_SET_BREAKPOINTS_BREAKPOINTS_ITEM_LINE_MAX),
            )?;
        }
        if let Some(value) = &self.condition {
            text("condition", value, 0, Some(MAX_BREAKPOINT_CONDITION_BYTES))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugSetBreakpointsParams {
    pub source_path: String,
    #[schemars(length(max = DEBUG_SET_BREAKPOINTS_BREAKPOINTS_MAX))]
    pub breakpoints: Vec<DebugSetBreakpointsBreakpointsItem>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugSetBreakpointsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("source_path", &self.source_path, 0, None)?;
        {
            let value = &self.breakpoints;
            if value.len() > DEBUG_SET_BREAKPOINTS_BREAKPOINTS_MAX {
                return Err(InputError::new(
                    "breakpoints",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.breakpoints.iter().enumerate() {
            value
                .validate()
                .map_err(|e| InputError::new(format!("breakpoints[{i}].{}", e.field), e.reason))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugSetFunctionBreakpointsBreakpointsItem {
    /// Canonical proc path, limited to 4096 UTF-8 bytes.
    #[schemars(length(min = DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MIN, max = DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MAX), extend("x-maxUtf8Bytes" = DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MAX))]
    pub proc_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub override_id: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub offset: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schemars(length(max = MAX_BREAKPOINT_CONDITION_BYTES), extend("x-maxUtf8Bytes" = MAX_BREAKPOINT_CONDITION_BYTES))]
    /// Optional condition, limited to 4096 UTF-8 bytes.
    pub condition: Option<String>,
}
impl Request for DebugSetFunctionBreakpointsBreakpointsItem {
    fn validate(&self) -> Result<(), InputError> {
        if !self.proc_path.starts_with('/') {
            return Err(InputError::new("proc_path", "proc_path must be canonical"));
        }
        {
            let value = &self.proc_path;
            if value.len() < DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MIN
                || value.len() > DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_ITEM_PROC_PATH_MAX
                || value.contains('\0')
            {
                return Err(InputError::new(
                    "proc_path",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        if let Some(value) = self.override_id {
            range("override_id", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        if let Some(value) = self.offset {
            range("offset", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        if let Some(value) = &self.condition {
            text("condition", value, 0, Some(MAX_BREAKPOINT_CONDITION_BYTES))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugSetFunctionBreakpointsParams {
    #[schemars(length(max = DEBUG_SET_FUNCTION_BREAKPOINTS_BREAKPOINTS_MAX))]
    pub breakpoints: Vec<DebugSetFunctionBreakpointsBreakpointsItem>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugSetFunctionBreakpointsParams {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = &self.breakpoints;
            if value.len() > DEBUG_SET_BREAKPOINTS_BREAKPOINTS_MAX {
                return Err(InputError::new(
                    "breakpoints",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.breakpoints.iter().enumerate() {
            value
                .validate()
                .map_err(|e| InputError::new(format!("breakpoints[{i}].{}", e.field), e.reason))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugSetExceptionBreakpointsParams {
    pub break_on_runtimes: bool,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugSetExceptionBreakpointsParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum DebugControlAction {
    #[serde(rename = "pause")]
    Pause,
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "step_in")]
    StepIn,
    #[serde(rename = "step_over")]
    StepOver,
    #[serde(rename = "step_out")]
    StepOut,
}
impl DebugControlAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Continue => "continue",
            Self::StepIn => "step_in",
            Self::StepOver => "step_over",
            Self::StepOut => "step_out",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugControlParams {
    pub action: DebugControlAction,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub thread_id: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugControlParams {
    fn validate(&self) -> Result<(), InputError> {
        if matches!(
            self.action,
            DebugControlAction::StepIn | DebugControlAction::StepOver | DebugControlAction::StepOut
        ) && self.thread_id.is_none()
        {
            return Err(InputError::new(
                "thread_id",
                "step controls require thread_id",
            ));
        }
        if let Some(value) = self.thread_id {
            range("thread_id", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugThreadsParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugThreadsParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugStackTraceParams {
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub thread_id: u64,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub start_frame: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = DEBUG_STACK_TRACE_COUNT_MAX))]
    pub count: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugStackTraceParams {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = self.thread_id;
            range("thread_id", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        if let Some(value) = self.start_frame {
            range("start_frame", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        if let Some(value) = self.count {
            range(
                "count",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(DEBUG_STACK_TRACE_COUNT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugScopesParams {
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub frame_id: u64,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugScopesParams {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = self.frame_id;
            range("frame_id", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugVariablesParams {
    #[schemars(range(min = PROTOCOL_I32_MIN, max = PROTOCOL_I32_MAX))]
    pub variables_reference: i64,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugVariablesParams {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = self.variables_reference;
            range(
                "variables_reference",
                value,
                Some(PROTOCOL_I32_MIN),
                Some(PROTOCOL_I32_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum DebugEvaluateContext {
    #[serde(rename = "watch")]
    Watch,
    #[serde(rename = "repl")]
    Repl,
    #[serde(rename = "hover")]
    Hover,
}
impl DebugEvaluateContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Watch => "watch",
            Self::Repl => "repl",
            Self::Hover => "hover",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugEvaluateParams {
    /// Permitted debugger expression, limited to 16384 UTF-8 bytes. Debugger control commands are excluded.
    #[schemars(length(max = DEBUG_EVALUATE_EXPRESSION_MAX), extend("x-maxUtf8Bytes" = DEBUG_EVALUATE_EXPRESSION_MAX))]
    pub expression: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub frame_id: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DebugEvaluateContext")]
    pub context: Option<DebugEvaluateContext>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugEvaluateParams {
    fn validate(&self) -> Result<(), InputError> {
        if self.expression.trim_start().starts_with('#') {
            return Err(InputError::new(
                "expression",
                "console commands require explicit controls",
            ));
        }
        text(
            "expression",
            &self.expression,
            0,
            Some(DEBUG_EVALUATE_EXPRESSION_MAX),
        )?;
        if let Some(value) = self.frame_id {
            range("frame_id", value, None, Some(MAX_PROTOCOL_U32))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugExceptionInfoParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugExceptionInfoParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugSourceParams {
    #[schemars(extend("enum" = [1]))]
    pub source_reference: u64,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugSourceParams {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = self.source_reference;
            if ![1].contains(&value) {
                return Err(InputError::new(
                    "source_reference",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum DebugWaitForEventKindsItemKind {
    #[serde(rename = "breakpoint")]
    Breakpoint,
    #[serde(rename = "step")]
    Step,
    #[serde(rename = "pause")]
    Pause,
    #[serde(rename = "runtime")]
    Runtime,
    #[serde(rename = "output")]
    Output,
    #[serde(rename = "terminated")]
    Terminated,
}
impl DebugWaitForEventKindsItemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Breakpoint => "breakpoint",
            Self::Step => "step",
            Self::Pause => "pause",
            Self::Runtime => "runtime",
            Self::Output => "output",
            Self::Terminated => "terminated",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugWaitForEventParams {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<DebugWaitForEventKindsItemKind>")]
    pub kinds: Option<Vec<DebugWaitForEventKindsItemKind>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER))]
    pub after_sequence: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = DEBUG_WAIT_FOR_EVENT_TIMEOUT_MS_MAX))]
    pub timeout_ms: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for DebugWaitForEventParams {
    fn validate(&self) -> Result<(), InputError> {
        if let Some(value) = self.timeout_ms {
            range(
                "timeout_ms",
                value,
                None,
                Some(DEBUG_WAIT_FOR_EVENT_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunParams {
    /// Path to the compiled .dmb file
    pub dmb_path: String,
    /// Port to run the server on (default: 1337)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = RUN_PORT_MAX))]
    pub port: Option<u64>,
    /// Optional working directory used to resolve a relative DMB path and run DreamDaemon
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub working_directory: Option<String>,
    /// Additional DreamDaemon options. DMB, port, working directory and loopback binding are managed; use -params for world parameters.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    pub daemon_args: Option<Vec<String>>,
    /// Optional output marker to wait for before returning
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub wait_for: Option<String>,
    /// Interpret wait_for as a regular expression (default: false)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub wait_regex: Option<bool>,
    /// Maximum wait for wait_for in milliseconds (default: 30000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = RUN_STARTUP_TIMEOUT_MS_MAX))]
    pub startup_timeout_ms: Option<u64>,
    /// Reject an unmanaged DMB unless it has a current Meridian-MCP build record
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub require_verified_provenance: Option<bool>,
}
impl Request for RunParams {
    fn validate(&self) -> Result<(), InputError> {
        super::tools::runtime::arguments::validate_daemon_args(
            self.daemon_args.as_deref().unwrap_or_default(),
        )
        .map_err(|_| {
            InputError::new(
                "daemon_args",
                "daemon arguments cannot override managed launch controls",
            )
        })?;
        if self.wait_regex.unwrap_or(false)
            && self
                .wait_for
                .as_ref()
                .is_some_and(|pattern| regex::Regex::new(pattern).is_err())
        {
            return Err(InputError::new(
                "wait_for",
                "invalid output regular expression",
            ));
        }
        text("dmb_path", &self.dmb_path, 0, None)?;
        if let Some(value) = self.port {
            range(
                "port",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(RUN_PORT_MAX),
            )?;
        }
        if let Some(value) = &self.working_directory {
            text("working_directory", value, 0, None)?;
        }
        for (i, value) in self.daemon_args.iter().flatten().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("daemon_args[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        if let Some(value) = &self.wait_for {
            text("wait_for", value, 0, None)?;
        }
        if let Some(value) = self.startup_timeout_ms {
            range(
                "startup_timeout_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(RUN_STARTUP_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitForOutputParams {
    /// Output text or regular expression to wait for
    pub pattern: String,
    /// Interpret pattern as a regular expression (default: false)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub regex: Option<bool>,
    /// Maximum wait in milliseconds (default: 30000, capped at 300000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER, max = WAIT_FOR_OUTPUT_TIMEOUT_MS_MAX))]
    pub timeout_ms: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for WaitForOutputParams {
    fn validate(&self) -> Result<(), InputError> {
        if self.regex.unwrap_or(false) && regex::Regex::new(&self.pattern).is_err() {
            return Err(InputError::new(
                "pattern",
                "invalid output regular expression",
            ));
        }
        text("pattern", &self.pattern, 0, None)?;
        if let Some(value) = self.timeout_ms {
            range(
                "timeout_ms",
                value,
                None,
                Some(WAIT_FOR_OUTPUT_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct StopParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for StopParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for StatusParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicParams {
    /// Topic text, limited to 65529 UTF-8 bytes after removing one optional leading question mark.
    /// The topic string to send (e.g., '?debug_screenshot' or '?debug_click=10,20')
    #[schemars(length(max = MAX_TOPIC_CHARACTERS), extend("x-maxPacketTopicUtf8Bytes" = MAX_TOPIC_BYTES))]
    pub topic: String,
    /// Total request timeout in milliseconds (default: 5000)
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TOPIC_TIMEOUT_MS_MAX))]
    pub timeout_ms: Option<u64>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for TopicParams {
    fn validate(&self) -> Result<(), InputError> {
        if self.topic.strip_prefix('?').unwrap_or(&self.topic).len() > MAX_TOPIC_BYTES {
            return Err(InputError::new(
                "topic",
                "Topic exceeds the packet byte ceiling",
            ));
        }
        text("topic", &self.topic, 0, None)?;
        if let Some(value) = self.timeout_ms {
            range(
                "timeout_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TOPIC_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyPrepareParams {
    pub dmb_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub overwrite: Option<bool>,
}
impl Request for TracyPrepareParams {
    fn validate(&self) -> Result<(), InputError> {
        text("dmb_path", &self.dmb_path, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyLaunchParams {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub experiment_name: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub map: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub seed: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub configuration_profile: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    #[schemars(length(max = TRACY_LAUNCH_FEATURE_SET_MAX))]
    #[schemars(inner(length(max = MAX_WORKLOAD_VALUE_BYTES)), extend("uniqueItems" = true, "x-itemMaxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    /// Up to 64 unique features, each limited to 512 UTF-8 bytes.
    pub feature_set: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub scenario: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub external_run_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, String>")]
    #[schemars(extend("maxProperties" = crate::tracy_experiment::MAX_ANNOTATIONS), inner(length(max = crate::tracy_experiment::MAX_ANNOTATION_VALUE_BYTES)))]
    /// Up to 32 annotations with 1-64 ASCII snake-case key bytes and at most 512 UTF-8 bytes per value.
    pub annotations: Option<BTreeMap<String, String>>,
    pub dmb_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TRACY_LAUNCH_GAME_PORT_MAX))]
    pub game_port: Option<u64>,
    /// Maximum time for both the Tracy client connection and producer-health readiness.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = TRACY_LAUNCH_STARTUP_TIMEOUT_MS_MIN, max = TRACY_LAUNCH_STARTUP_TIMEOUT_MS_MAX))]
    #[schemars(extend("default" = serde_json::json!(60000)))]
    pub startup_timeout_ms: Option<u64>,
    /// Existing contained directory for immutable experiment manifests, recovery journals, and diagnostics.
    pub experiment_directory: String,
    /// Optional contained profiling configuration directory. It must contain config.txt and explicitly enable RESUME_AFTER_INITIALIZATIONS in config.txt or dev_overrides.txt; its full contents are hash-bound into launch identity. The flag requires a post-initialization wake before it can take effect on a headless world.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub config_directory: Option<String>,
    /// When config_directory is supplied, wait for Meridian-Rift initialization, try bounded loopback Topic wakes, and if necessary hold one fixed MCP-owned loopback DreamSeeker guest connection until stop. Set false for a deliberately sleeping control run.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(true)))]
    pub wake_sleeping_world: Option<bool>,
    /// Maximum wait for the fixed Meridian-Rift initialization-complete marker before the post-initialization wake request.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TRACY_LAUNCH_INITIALIZATION_TIMEOUT_MS_MAX))]
    #[schemars(extend("default" = serde_json::json!(180000)))]
    pub initialization_timeout_ms: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub require_verified_provenance: Option<bool>,
}
impl Request for TracyLaunchParams {
    fn validate(&self) -> Result<(), InputError> {
        self.workload().map_err(workload_error)?;
        if let Some(value) = &self.experiment_name {
            text("experiment_name", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
            crate::tracy_experiment::validate_workload(crate::tracy_experiment::WorkloadInput {
                external_run_id: Some(value.clone()),
                ..Default::default()
            })
            .map_err(|_| InputError::new("experiment_name", "invalid experiment name"))?;
        }
        if let Some(value) = &self.map {
            text("map", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.seed {
            text("seed", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.configuration_profile {
            text(
                "configuration_profile",
                value,
                0,
                Some(MAX_WORKLOAD_VALUE_BYTES),
            )?;
        }
        if let Some(value) = &self.feature_set {
            if value.len() > TRACY_LAUNCH_FEATURE_SET_MAX {
                return Err(InputError::new(
                    "feature_set",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.feature_set.iter().flatten().enumerate() {
            if value.len() > MAX_WORKLOAD_VALUE_BYTES || value.contains('\0') {
                return Err(InputError::new(
                    format!("feature_set[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        if let Some(value) = &self.scenario {
            text("scenario", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.external_run_id {
            text("external_run_id", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.annotations {
            if value.len() > crate::tracy_experiment::MAX_ANNOTATIONS {
                return Err(InputError::new(
                    "annotations",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        text("dmb_path", &self.dmb_path, 0, None)?;
        if let Some(value) = self.game_port {
            range(
                "game_port",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TRACY_LAUNCH_GAME_PORT_MAX),
            )?;
        }
        if let Some(value) = self.startup_timeout_ms {
            range(
                "startup_timeout_ms",
                value,
                Some(TRACY_LAUNCH_STARTUP_TIMEOUT_MS_MIN),
                Some(TRACY_LAUNCH_STARTUP_TIMEOUT_MS_MAX),
            )?;
        }
        text("experiment_directory", &self.experiment_directory, 0, None)?;
        if let Some(value) = &self.config_directory {
            text("config_directory", value, 0, None)?;
        }
        if let Some(value) = self.initialization_timeout_ms {
            range(
                "initialization_timeout_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TRACY_LAUNCH_INITIALIZATION_TIMEOUT_MS_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyCaptureParams {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub map: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub seed: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub configuration_profile: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    #[schemars(length(max = TRACY_CAPTURE_FEATURE_SET_MAX))]
    #[schemars(inner(length(max = MAX_WORKLOAD_VALUE_BYTES)), extend("uniqueItems" = true, "x-itemMaxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    /// Up to 64 unique features, each limited to 512 UTF-8 bytes.
    pub feature_set: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub scenario: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    /// Workload label, limited to 512 UTF-8 bytes; host paths, control characters and environment expansions are excluded.
    #[schemars(length(max = MAX_WORKLOAD_VALUE_BYTES), extend("x-maxUtf8Bytes" = MAX_WORKLOAD_VALUE_BYTES))]
    pub external_run_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, String>")]
    #[schemars(extend("maxProperties" = crate::tracy_experiment::MAX_ANNOTATIONS), inner(length(max = crate::tracy_experiment::MAX_ANNOTATION_VALUE_BYTES)))]
    /// Up to 32 annotations with 1-64 ASCII snake-case key bytes and at most 512 UTF-8 bytes per value.
    pub annotations: Option<BTreeMap<String, String>>,
    pub output_path: String,
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TRACY_CAPTURE_DURATION_MS_MAX))]
    pub duration_ms: u64,
    #[schemars(range(min = TRACY_CAPTURE_MEMORY_LIMIT_MB_MIN, max = TRACY_CAPTURE_MEMORY_LIMIT_MB_MAX))]
    pub memory_limit_mb: u64,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub overwrite: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    #[schemars(extend("default" = serde_json::json!(false)))]
    pub capture_network: Option<bool>,
    #[schemars(length(min = TRACY_CAPTURE_PHASE_MIN, max = TRACY_CAPTURE_PHASE_MAX))]
    #[schemars(regex(pattern = "^[a-z0-9_-]+$"))]
    pub phase: String,
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = MAX_PROTOCOL_U32))]
    pub phase_iteration: u64,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, String>")]
    #[schemars(extend("maxProperties" = crate::tracy_experiment::MAX_ANNOTATIONS), inner(length(max = crate::tracy_experiment::MAX_ANNOTATION_VALUE_BYTES)))]
    /// Up to 32 annotations with 1-64 ASCII snake-case key bytes and at most 512 UTF-8 bytes per value.
    pub capture_annotations: Option<BTreeMap<String, String>>,
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for TracyCaptureParams {
    fn validate(&self) -> Result<(), InputError> {
        self.workload().map_err(workload_error)?;
        crate::tracy_experiment::validate_workload(crate::tracy_experiment::WorkloadInput {
            annotations: self.capture_annotations.clone().unwrap_or_default(),
            ..Default::default()
        })
        .map_err(|_| InputError::new("capture_annotations", "invalid capture annotation"))?;
        if let Some(value) = &self.map {
            text("map", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.seed {
            text("seed", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.configuration_profile {
            text(
                "configuration_profile",
                value,
                0,
                Some(MAX_WORKLOAD_VALUE_BYTES),
            )?;
        }
        if let Some(value) = &self.feature_set {
            if value.len() > TRACY_CAPTURE_FEATURE_SET_MAX {
                return Err(InputError::new(
                    "feature_set",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.feature_set.iter().flatten().enumerate() {
            if value.len() > MAX_WORKLOAD_VALUE_BYTES || value.contains('\0') {
                return Err(InputError::new(
                    format!("feature_set[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        if let Some(value) = &self.scenario {
            text("scenario", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.external_run_id {
            text("external_run_id", value, 0, Some(MAX_WORKLOAD_VALUE_BYTES))?;
        }
        if let Some(value) = &self.annotations {
            if value.len() > crate::tracy_experiment::MAX_ANNOTATIONS {
                return Err(InputError::new(
                    "annotations",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        text("output_path", &self.output_path, 0, None)?;
        {
            let value = self.duration_ms;
            range(
                "duration_ms",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TRACY_CAPTURE_DURATION_MS_MAX),
            )?;
        }
        {
            let value = self.memory_limit_mb;
            range(
                "memory_limit_mb",
                value,
                Some(TRACY_CAPTURE_MEMORY_LIMIT_MB_MIN),
                Some(TRACY_CAPTURE_MEMORY_LIMIT_MB_MAX),
            )?;
        }
        {
            let value = &self.phase;
            if value.len() < TRACY_CAPTURE_PHASE_MIN
                || value.len() > TRACY_CAPTURE_PHASE_MAX
                || value.contains('\0')
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                return Err(InputError::new(
                    "phase",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        {
            let value = self.phase_iteration;
            range(
                "phase_iteration",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(MAX_PROTOCOL_U32),
            )?;
        }
        if let Some(value) = &self.capture_annotations {
            if value.len() > crate::tracy_experiment::MAX_ANNOTATIONS {
                return Err(InputError::new(
                    "capture_annotations",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        Ok(())
    }
}

fn workload_error(error: crate::tracy_experiment::ExperimentError) -> InputError {
    use crate::tracy_experiment::ExperimentError::*;
    let field = match error {
        ValueTooLong { field } | UnsafeValue { field } if field != "annotation value" => field,
        TooManyFeatures | DuplicateFeature => "feature_set".into(),
        _ => "annotations".into(),
    };
    InputError::new(
        field,
        "invalid workload identity, feature set or annotation",
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyStatusParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for TracyStatusParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyStopParams {
    /// Optional identity expectation; a stale handle fails before work starts.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl Request for TracyStopParams {
    fn validate(&self) -> Result<(), InputError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum TracyHotspotsSort {
    #[serde(rename = "inclusive")]
    Inclusive,
    #[serde(rename = "self")]
    SelfTime,
    #[serde(rename = "count")]
    Count,
    #[serde(rename = "max")]
    Max,
}
impl TracyHotspotsSort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inclusive => "inclusive",
            Self::SelfTime => "self",
            Self::Count => "count",
            Self::Max => "max",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyHotspotsParams {
    pub trace_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TRACY_HOTSPOTS_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TracyHotspotsSort")]
    #[schemars(extend("default" = serde_json::json!("inclusive")))]
    pub sort: Option<TracyHotspotsSort>,
    /// Optional expectation for source correlation; artifact analysis needs no parse.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for TracyHotspotsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("trace_path", &self.trace_path, 0, None)?;
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TRACY_HOTSPOTS_LIMIT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyZoneParams {
    pub trace_path: String,
    #[schemars(length(max = TRACY_ZONE_NAME_MAX), extend("x-maxUtf8Bytes" = TRACY_ZONE_NAME_MAX))]
    /// Zone name, limited to 4096 UTF-8 bytes.
    pub name: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TRACY_ZONE_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    /// Optional expectation for source correlation; artifact analysis needs no parse.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for TracyZoneParams {
    fn validate(&self) -> Result<(), InputError> {
        text("trace_path", &self.trace_path, 0, None)?;
        {
            let value = &self.name;
            if value.len() > TRACY_ZONE_NAME_MAX || value.contains('\0') {
                return Err(InputError::new(
                    "name",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TRACY_ZONE_LIMIT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyFrameStatsParams {
    pub trace_path: String,
    /// Optional expectation for source correlation; artifact analysis needs no parse.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for TracyFrameStatsParams {
    fn validate(&self) -> Result<(), InputError> {
        text("trace_path", &self.trace_path, 0, None)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum TracyCompareComparisonMode {
    #[serde(rename = "same_experiment_same_phase")]
    SameExperimentSamePhase,
    #[serde(rename = "cross_experiment")]
    CrossExperiment,
}
impl TracyCompareComparisonMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SameExperimentSamePhase => "same_experiment_same_phase",
            Self::CrossExperiment => "cross_experiment",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyCompareParams {
    pub baseline_path: String,
    pub current_path: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TracyCompareComparisonMode")]
    #[schemars(extend("default" = serde_json::json!("same_experiment_same_phase")))]
    pub comparison_mode: Option<TracyCompareComparisonMode>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_NONNEGATIVE_INTEGER))]
    #[schemars(extend("default" = serde_json::json!(0)))]
    pub minimum_delta_ns: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    #[schemars(range(min = MIN_POSITIVE_INTEGER, max = TRACY_COMPARE_LIMIT_MAX))]
    #[schemars(extend("default" = serde_json::json!(100)))]
    pub limit: Option<u64>,
    /// Optional expectation for source correlation; artifact analysis needs no parse.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for TracyCompareParams {
    fn validate(&self) -> Result<(), InputError> {
        text("baseline_path", &self.baseline_path, 0, None)?;
        text("current_path", &self.current_path, 0, None)?;
        if let Some(value) = self.limit {
            range(
                "limit",
                value,
                Some(MIN_POSITIVE_INTEGER),
                Some(TRACY_COMPARE_LIMIT_MAX),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum TracyControlStatsFramePercentile {
    #[serde(rename = "p50")]
    P50,
    #[serde(rename = "p95")]
    P95,
    #[serde(rename = "p99")]
    P99,
}
impl TracyControlStatsFramePercentile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::P99 => "p99",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum TracyControlStatsComparisonMode {
    #[serde(rename = "same_experiment_same_phase")]
    SameExperimentSamePhase,
    #[serde(rename = "cross_experiment")]
    CrossExperiment,
}
impl TracyControlStatsComparisonMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SameExperimentSamePhase => "same_experiment_same_phase",
            Self::CrossExperiment => "cross_experiment",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracyControlStatsParams {
    #[schemars(length(min = TRACY_CONTROL_STATS_TRACE_PATHS_MIN, max = TRACY_CONTROL_STATS_TRACE_PATHS_MAX))]
    #[schemars(extend("uniqueItems" = true))]
    pub trace_paths: Vec<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TracyControlStatsFramePercentile")]
    #[schemars(extend("default" = serde_json::json!("p95")))]
    pub frame_percentile: Option<TracyControlStatsFramePercentile>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    #[schemars(length(max = TRACY_CONTROL_STATS_ZONE_KEYS_MAX))]
    #[schemars(inner(length(max = TRACY_ZONE_NAME_MAX)), extend("uniqueItems" = true, "x-itemMaxUtf8Bytes" = TRACY_ZONE_NAME_MAX))]
    pub zone_keys: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TracyControlStatsComparisonMode")]
    #[schemars(extend("default" = serde_json::json!("same_experiment_same_phase")))]
    pub comparison_mode: Option<TracyControlStatsComparisonMode>,
    /// Optional expectation for source correlation; artifact analysis needs no parse.
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::SnapshotId")]
    pub expected_snapshot: Option<crate::identity::SnapshotId>,
}
impl Request for TracyControlStatsParams {
    fn validate(&self) -> Result<(), InputError> {
        {
            let value = &self.trace_paths;
            if value.len() < TRACY_CONTROL_STATS_TRACE_PATHS_MIN
                || value.len() > TRACY_CONTROL_STATS_TRACE_PATHS_MAX
                || value
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != value.len()
            {
                return Err(InputError::new(
                    "trace_paths",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.trace_paths.iter().enumerate() {
            if value.contains('\0') {
                return Err(InputError::new(
                    format!("trace_paths[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        if let Some(value) = &self.zone_keys {
            if value.len() > TRACY_CONTROL_STATS_ZONE_KEYS_MAX
                || value
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != value.len()
            {
                return Err(InputError::new(
                    "zone_keys",
                    "request field is outside its advertised bounds",
                ));
            }
        }
        for (i, value) in self.zone_keys.iter().flatten().enumerate() {
            if value.len() > TRACY_ZONE_NAME_MAX || value.contains('\0') {
                return Err(InputError::new(
                    format!("zone_keys[{i}]"),
                    "string item is invalid or too long",
                ));
            }
        }
        Ok(())
    }
}

impl Request for crate::memory_evidence::MemoryRequest {
    fn validate(&self) -> Result<(), InputError> {
        if self.sample_limit > crate::memory_evidence::MAX_SAMPLE_LIMIT {
            return Err(InputError::new(
                "sample_limit",
                "sample_limit must be 0 through 100",
            ));
        }
        if self
            .end_ms
            .is_some_and(|end| end <= self.begin_ms.unwrap_or(0))
        {
            return Err(InputError::new("end_ms", "end_ms must follow begin_ms"));
        }
        Ok(())
    }
}
impl Request for crate::memory_evidence::MemoryCompareRequest {
    fn validate(&self) -> Result<(), InputError> {
        self.baseline
            .validate()
            .map_err(|e| InputError::new(format!("baseline.{}", e.field), e.reason))?;
        self.current
            .validate()
            .map_err(|e| InputError::new(format!("current.{}", e.field), e.reason))?;
        Ok(())
    }
}
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugMemoryParams {
    pub action: crate::native_memory::MemoryAction,
    #[serde(default = "crate::native_memory::default_duration")]
    #[schemars(range(min = 1, max = crate::native_memory::MAX_DURATION_MS))]
    pub duration_ms: u64,
    #[serde(default = "crate::native_memory::default_records")]
    #[schemars(range(min = 1, max = crate::native_memory::MAX_RECORDS))]
    pub max_records: usize,
    #[serde(default = "crate::native_memory::default_rows")]
    #[schemars(range(min = 1, max = crate::native_memory::MAX_ROW_LIMIT))]
    pub row_limit: usize,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::identity::RuntimeId")]
    pub expected_runtime: Option<crate::identity::RuntimeId>,
}
impl DebugMemoryParams {
    pub(crate) fn into_control(self) -> crate::native_memory::MemoryControl {
        crate::native_memory::MemoryControl {
            action: self.action,
            duration_ms: self.duration_ms,
            max_records: self.max_records,
            row_limit: self.row_limit,
        }
    }
}
impl Request for DebugMemoryParams {
    fn validate(&self) -> Result<(), InputError> {
        range(
            "duration_ms",
            self.duration_ms,
            Some(1),
            Some(crate::native_memory::MAX_DURATION_MS),
        )?;
        range(
            "max_records",
            self.max_records as u64,
            Some(1),
            Some(crate::native_memory::MAX_RECORDS as u64),
        )?;
        range(
            "row_limit",
            self.row_limit as u64,
            Some(1),
            Some(crate::native_memory::MAX_ROW_LIMIT as u64),
        )?;
        Ok(())
    }
}

impl Request for crate::native_memory::MemoryControl {
    fn validate(&self) -> Result<(), InputError> {
        if !(1..=crate::native_memory::MAX_DURATION_MS).contains(&self.duration_ms) {
            return Err(InputError::new(
                "duration_ms",
                "duration_ms must be 1 through 60000",
            ));
        }
        if !(1..=crate::native_memory::MAX_RECORDS).contains(&self.max_records) {
            return Err(InputError::new(
                "max_records",
                "max_records must be 1 through 100000",
            ));
        }
        if !(1..=crate::native_memory::MAX_ROW_LIMIT).contains(&self.row_limit) {
            return Err(InputError::new(
                "row_limit",
                "row_limit must be 1 through 1000",
            ));
        }
        Ok(())
    }
}
impl Request for crate::native_evidence::model::NativeEvidenceRequest {
    fn validate(&self) -> Result<(), InputError> {
        use crate::limits::*;
        range(
            "artifacts",
            self.artifacts.len(),
            Some(1),
            Some(MAX_EVIDENCE_ARTIFACTS),
        )?;
        artifact_paths(self)?;
        range("phases", self.phases.len(), None, Some(MAX_EVIDENCE_PHASES))?;
        let mut ids = std::collections::BTreeSet::new();
        for (index, phase) in self.phases.iter().enumerate() {
            let path = format!("phases[{index}]");
            text(
                &format!("{path}.id"),
                &phase.id,
                1,
                Some(MAX_EVIDENCE_PHASE_ID_BYTES),
            )?;
            if !ids.insert(&phase.id) {
                return Err(InputError::new(
                    format!("{path}.id"),
                    "phase identifiers must be unique",
                ));
            }
            for (field, value) in [
                ("wall_start", &phase.wall_start),
                ("wall_end", &phase.wall_end),
            ] {
                crate::native_evidence::timeline::wall_ms(value.as_deref()).map_err(|_| {
                    InputError::new(format!("{path}.{field}"), "phase wall time must be RFC3339")
                })?;
            }
            crate::native_evidence::timeline::validate_phases(std::slice::from_ref(phase))
                .map_err(|_| InputError::new(path, "phase ranges must be increasing"))?;
        }
        crate::native_evidence::timeline::validate_phases(&self.phases)
            .map_err(|_| InputError::new("phases", "phase ranges must not overlap"))?;
        for (index, artifact) in self.artifacts.iter().enumerate() {
            if let Some(options) = &artifact.options {
                let path = format!("artifacts[{index}].options");
                for (field, values, maximum) in [
                    (
                        "selected_metrics",
                        &options.selected_metrics,
                        MAX_EVIDENCE_SELECTED_METRICS,
                    ),
                    (
                        "group_fields",
                        &options.group_fields,
                        MAX_EVIDENCE_GROUP_FIELDS,
                    ),
                ] {
                    range(
                        &format!("{path}.{field}"),
                        values.len(),
                        None,
                        Some(maximum),
                    )?;
                    if values
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != values.len()
                    {
                        return Err(InputError::new(
                            format!("{path}.{field}"),
                            "field names must be unique",
                        ));
                    }
                }
                for (item, name) in options.group_fields.iter().enumerate() {
                    text(
                        &format!("{path}.group_fields[{item}]"),
                        name,
                        0,
                        Some(MAX_EVIDENCE_GROUP_FIELD_BYTES),
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn artifact_paths(
    request: &crate::native_evidence::model::NativeEvidenceRequest,
) -> Result<(), InputError> {
    let mut paths = std::collections::BTreeSet::new();
    for (index, artifact) in request.artifacts.iter().enumerate() {
        if !paths.insert(&artifact.path) {
            return Err(InputError::new(
                format!("artifacts[{index}].path"),
                "artifact paths must be unique",
            ));
        }
    }
    Ok(())
}

pub(crate) enum ToolRequest {
    ServerStatus(ServerStatusParams),
    ParseEnvironment(ParseEnvironmentParams),
    CheckFixtureSync(CheckFixtureSyncParams),
    MemorySummary(crate::memory_evidence::MemoryRequest),
    MemoryCompare(crate::memory_evidence::MemoryCompareRequest),
    NativeEvidenceSummary(crate::native_evidence::model::NativeEvidenceRequest),
    NativeEvidenceCompare(NativeEvidenceCompareParams),
    GetType(GetTypeParams),
    GetProc(GetProcParams),
    GetVar(GetVarParams),
    ListTypes(ListTypesParams),
    SearchSymbols(SearchSymbolsParams),
    SearchContext(SearchContextParams),
    CheckErrors(CheckErrorsParams),
    GetDefinition(GetDefinitionParams),
    DocumentSymbols(DocumentSymbolsParams),
    FindReferences(FindReferencesParams),
    FindImplementations(FindImplementationsParams),
    GenerateDocs(GenerateDocsParams),
    Compile(CompileParams),
    RiftCompile(RiftCompileParams),
    DmiInfo(DmiInfoParams),
    CompareDmiStates(CompareDmiStatesParams),
    FindDmiDuplicates(FindDmiDuplicatesParams),
    AuditIcons(AuditIconsParams),
    ExtractDmi(ExtractDmiParams),
    RenderMap(RenderMapParams),
    DiffMaps(DiffMapsParams),
    ListRenderPasses(ListRenderPassesParams),
    RenderMaps(RenderMapsParams),
    MapInfo(MapInfoParams),
    FindOnMap(FindOnMapParams),
    DebugLaunch(DebugLaunchParams),
    DebugMemory(DebugMemoryParams),
    DebugStop(DebugStopParams),
    DebugSetBreakpoints(DebugSetBreakpointsParams),
    DebugSetFunctionBreakpoints(DebugSetFunctionBreakpointsParams),
    DebugSetExceptionBreakpoints(DebugSetExceptionBreakpointsParams),
    DebugControl(DebugControlParams),
    DebugThreads(DebugThreadsParams),
    DebugStackTrace(DebugStackTraceParams),
    DebugScopes(DebugScopesParams),
    DebugVariables(DebugVariablesParams),
    DebugEvaluate(DebugEvaluateParams),
    DebugExceptionInfo(DebugExceptionInfoParams),
    DebugSource(DebugSourceParams),
    DebugWaitForEvent(DebugWaitForEventParams),
    Run(RunParams),
    WaitForOutput(WaitForOutputParams),
    Stop(StopParams),
    Status(StatusParams),
    Topic(TopicParams),
    TracyPrepare(TracyPrepareParams),
    TracyLaunch(TracyLaunchParams),
    TracyCapture(TracyCaptureParams),
    TracyStatus(TracyStatusParams),
    TracyStop(TracyStopParams),
    TracyHotspots(TracyHotspotsParams),
    TracyZone(TracyZoneParams),
    TracyFrameStats(TracyFrameStatsParams),
    TracyCompare(TracyCompareParams),
    TracyControlStats(TracyControlStatsParams),
}

impl ToolRequest {
    /// Wait limits of zero remain immediate probes; they never create a zero
    /// ingress budget. All useful work shares the original request start.
    pub(crate) fn total_budget_ms(&self, default_budget_ms: u64) -> u64 {
        match self {
            Self::ParseEnvironment(args) => args.timeout_ms.unwrap_or(600_000),
            Self::Compile(args) => args.timeout_ms.unwrap_or(600_000),
            Self::RiftCompile(args) => args.timeout_ms.unwrap_or(RIFT_DEFAULT_TIMEOUT_MS),
            Self::DebugLaunch(args) => args.startup_timeout_ms.unwrap_or(60_000),
            Self::Run(args) => args.startup_timeout_ms.unwrap_or(300_000),
            Self::TracyLaunch(args) => args
                .startup_timeout_ms
                .unwrap_or(60_000)
                .saturating_add(args.initialization_timeout_ms.unwrap_or(180_000)),
            Self::TracyCapture(args) => args.duration_ms.saturating_add(60_000),
            Self::Topic(args) => args.timeout_ms.unwrap_or(5_000),
            Self::GenerateDocs(_) => crate::limits::ServerLimits::default().max_docs_duration_ms,
            _ => default_budget_ms,
        }
    }

    pub(crate) fn needs_path_admission(&self) -> bool {
        use ToolRequest::*;
        matches!(
            self,
            ParseEnvironment(_)
                | CheckFixtureSync(_)
                | MemorySummary(_)
                | MemoryCompare(_)
                | NativeEvidenceSummary(_)
                | NativeEvidenceCompare(_)
                | DocumentSymbols(_)
                | DmiInfo(_)
                | CompareDmiStates(_)
                | FindDmiDuplicates(_)
                | AuditIcons(_)
                | ExtractDmi(_)
                | GenerateDocs(_)
                | RenderMap(_)
                | MapInfo(_)
                | FindOnMap(_)
                | DiffMaps(_)
                | Compile(_)
                | RiftCompile(_)
                | Run(_)
                | DebugLaunch(_)
                | DebugSetBreakpoints(_)
                | TracyPrepare(_)
                | TracyLaunch(_)
                | TracyHotspots(_)
                | TracyZone(_)
                | TracyFrameStats(_)
                | TracyCompare(_)
        )
    }

    pub(crate) fn snapshot_expectation(&self) -> Option<Option<&str>> {
        match self {
            Self::GetType(args) => Some(args.expected_snapshot.as_deref()),
            Self::GetProc(args) => Some(args.expected_snapshot.as_deref()),
            Self::GetVar(args) => Some(args.expected_snapshot.as_deref()),
            Self::ListTypes(args) => Some(args.expected_snapshot.as_deref()),
            Self::SearchSymbols(args) => Some(args.expected_snapshot.as_deref()),
            Self::SearchContext(args) => Some(args.expected_snapshot.as_deref()),
            Self::CheckErrors(args) => Some(args.expected_snapshot.as_deref()),
            Self::GetDefinition(args) => Some(args.expected_snapshot.as_deref()),
            Self::DocumentSymbols(args) => Some(args.expected_snapshot.as_deref()),
            Self::FindReferences(args) => Some(args.expected_snapshot.as_deref()),
            Self::FindImplementations(args) => Some(args.expected_snapshot.as_deref()),
            Self::GenerateDocs(args) => Some(args.expected_snapshot.as_deref()),
            Self::AuditIcons(args) => Some(args.expected_snapshot.as_deref()),
            Self::RenderMap(args) => Some(args.expected_snapshot.as_deref()),
            Self::RenderMaps(args) => Some(args.expected_snapshot.as_deref()),
            Self::DebugSetBreakpoints(args) => Some(args.expected_snapshot.as_deref()),
            Self::DebugSetFunctionBreakpoints(args) => args.expected_snapshot.as_deref().map(Some),
            Self::TracyHotspots(args) => args.expected_snapshot.as_deref().map(Some),
            Self::TracyZone(args) => args.expected_snapshot.as_deref().map(Some),
            Self::TracyFrameStats(args) => args.expected_snapshot.as_deref().map(Some),
            Self::TracyCompare(args) => args.expected_snapshot.as_deref().map(Some),
            Self::TracyControlStats(args) => args.expected_snapshot.as_deref().map(Some),
            _ => None,
        }
    }
    pub(crate) fn uses_optional_analysis(&self) -> bool {
        matches!(
            self,
            Self::TracyHotspots(_)
                | Self::TracyZone(_)
                | Self::TracyFrameStats(_)
                | Self::TracyCompare(_)
                | Self::TracyControlStats(_)
        )
    }
    pub(crate) fn runtime_expectation(&self) -> Option<(bool, Option<&str>)> {
        match self {
            Self::DebugStop(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugSetBreakpoints(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugSetFunctionBreakpoints(args) => {
                Some((true, args.expected_runtime.as_deref()))
            }
            Self::DebugSetExceptionBreakpoints(args) => {
                Some((true, args.expected_runtime.as_deref()))
            }
            Self::DebugControl(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugThreads(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugStackTrace(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugScopes(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugVariables(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugEvaluate(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugMemory(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugExceptionInfo(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugSource(args) => Some((true, args.expected_runtime.as_deref())),
            Self::DebugWaitForEvent(args) => Some((true, args.expected_runtime.as_deref())),
            Self::WaitForOutput(args) => Some((false, args.expected_runtime.as_deref())),
            Self::Stop(args) => Some((false, args.expected_runtime.as_deref())),
            Self::Status(args) => Some((false, args.expected_runtime.as_deref())),
            Self::Topic(args) => Some((false, args.expected_runtime.as_deref())),
            Self::TracyCapture(args) => Some((false, args.expected_runtime.as_deref())),
            Self::TracyStatus(args) => Some((false, args.expected_runtime.as_deref())),
            Self::TracyStop(args) => Some((false, args.expected_runtime.as_deref())),
            _ => None,
        }
    }

    /// Aliases become comparable only after the startup path policy canonicalizes
    /// each artifact. Check before any artifact parsing or blocking admission.
    pub(crate) fn validate_canonical_paths(&self) -> Result<(), InputError> {
        match self {
            Self::NativeEvidenceSummary(request) => artifact_paths(request),
            Self::NativeEvidenceCompare(request) => {
                for (index, run) in request.runs.iter().enumerate() {
                    artifact_paths(run).map_err(|error| {
                        InputError::new(format!("runs[{index}].{}", error.field), error.reason)
                    })?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

pub type RiftNetworkMode = RiftCompileNetworkMode;
pub const RIFT_DEFAULT_TIMEOUT_MS: u64 = 1_800_000;
pub const RIFT_MAX_TIMEOUT_MS: u64 = 1_800_000;
pub const RIFT_DEFAULT_IDLE_TIMEOUT_MS: u64 = 120_000;
pub const RIFT_MAX_IDLE_TIMEOUT_MS: u64 = 900_000;
pub const RIFT_MIN_TIMEOUT_MS: u64 = 1_000;
impl RiftCompileParams {
    pub fn network_mode(&self) -> RiftNetworkMode {
        self.network_mode.unwrap_or(RiftNetworkMode::Offline)
    }
    pub fn validated_timeouts(&self) -> Result<(u64, u64), &'static str> {
        Ok((
            self.timeout_ms.unwrap_or(RIFT_DEFAULT_TIMEOUT_MS),
            self.idle_timeout_ms.unwrap_or(RIFT_DEFAULT_IDLE_TIMEOUT_MS),
        ))
    }
}

impl TracyLaunchParams {
    pub(crate) fn workload(
        &self,
    ) -> Result<crate::tracy_experiment::WorkloadInput, crate::tracy_experiment::ExperimentError>
    {
        crate::tracy_experiment::validate_workload(crate::tracy_experiment::WorkloadInput {
            map: self.map.clone(),
            seed: self.seed.clone(),
            configuration_profile: self.configuration_profile.clone(),
            feature_set: self.feature_set.clone().unwrap_or_default(),
            scenario: self.scenario.clone(),
            external_run_id: self.external_run_id.clone(),
            annotations: self.annotations.clone().unwrap_or_default(),
        })
    }
}

impl TracyCaptureParams {
    pub(crate) fn workload(
        &self,
    ) -> Result<crate::tracy_experiment::WorkloadInput, crate::tracy_experiment::ExperimentError>
    {
        crate::tracy_experiment::validate_workload(crate::tracy_experiment::WorkloadInput {
            map: self.map.clone(),
            seed: self.seed.clone(),
            configuration_profile: self.configuration_profile.clone(),
            feature_set: self.feature_set.clone().unwrap_or_default(),
            scenario: self.scenario.clone(),
            external_run_id: self.external_run_id.clone(),
            annotations: self.annotations.clone().unwrap_or_default(),
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Coordinate(#[schemars(range(min = MIN_POSITIVE_INTEGER))] pub u64);
impl<'de> Deserialize<'de> for Coordinate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        if value == 0 {
            return Err(serde::de::Error::custom("coordinate must be positive"));
        }
        Ok(Self(value))
    }
}

fn render_coordinates(
    min: Option<&[Coordinate; 3]>,
    max: Option<&[Coordinate; 3]>,
) -> Result<(), InputError> {
    if let (Some(min), Some(max)) = (min, max) {
        if min[0].0 > max[0].0 || min[1].0 > max[1].0 || min[2].0 != max[2].0 {
            return Err(InputError::new(
                "max",
                "render bounds must increase in x/y and share one z level",
            ));
        }
    }
    Ok(())
}
