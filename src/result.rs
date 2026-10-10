use crate::capabilities::SPACEMANDMM_REVISION;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct ToolMetadata {
    pub meridian_mcp_version: &'static str,
    pub meridian_mcp_build: crate::build_identity::BuildIdentity,
    pub spacemandmm_revision: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<crate::identity::AnalysisIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_generation: Option<u64>,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
}

impl ToolMetadata {
    pub fn complete(state_generation: Option<u64>) -> Self {
        Self {
            meridian_mcp_version: env!("CARGO_PKG_VERSION"),
            meridian_mcp_build: crate::build_identity::current().clone(),
            spacemandmm_revision: SPACEMANDMM_REVISION,
            state_generation,
            analysis: None,
            runtime_id: None,
            asset_generation: None,
            truncated: false,
            truncation_reasons: Vec::new(),
        }
    }

    pub fn for_snapshot(snapshot: &crate::analysis_snapshot::AnalysisSnapshot) -> Self {
        let mut metadata = Self::complete(Some(snapshot.generation));
        metadata.analysis = Some(snapshot.identity());
        metadata
    }
}

pub(crate) fn analysis_text<T: Serialize>(
    snapshot: &crate::analysis_snapshot::AnalysisSnapshot,
    payload: T,
) -> anyhow::Result<ToolResult> {
    Ok(projection(
        AnalysisOutput {
            analysis: snapshot.identity(),
            data: payload,
        },
        true,
        false,
    )
    .with_analysis(snapshot.identity()))
}

#[derive(Serialize, JsonSchema)]
pub struct AnalysisOutput<T> {
    pub analysis: crate::identity::AnalysisIdentity,
    #[serde(flatten)]
    pub data: T,
}

#[derive(Serialize, JsonSchema)]
pub struct Success<T> {
    #[serde(flatten)]
    pub data: T,
    #[serde(flatten)]
    pub metadata: ToolMetadata,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ToolFailure {
    pub code: String,
    pub message: String,
    pub recovery: Option<String>,
    pub details: std::collections::BTreeMap<String, Value>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct PlainFailure {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<MutationOutcome>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct OutputMutation {
    pub request_index: usize,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub cleanup_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_preview: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct MutationOutcome {
    pub operation_ran: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_succeeded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_count: Option<u64>,
    pub outputs: Vec<OutputMutation>,
    pub process_started: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub process_start_uncertain: bool,
    pub process_stopped: bool,
    pub cleanup_complete: bool,
    pub recovery_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub verified_request_indices: Vec<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unverified_request_indices: Vec<usize>,
    pub removed_breakpoints: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_request_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance_status: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct ReducedMutation {
    pub outcome: MutationOutcome,
    pub truncated: bool,
    pub response_omissions: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ToolOutput<T> {
    Success(OutcomeProjection<T>),
    Failure(ToolFailure),
    PlainFailure(PlainFailure),
    ReducedMutation(ReducedMutation),
}

#[derive(Serialize, JsonSchema)]
pub struct OutcomeProjection<T> {
    #[serde(flatten)]
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<MutationOutcome>,
}
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub(crate) struct OutcomeError {
    #[source]
    pub error: anyhow::Error,
    pub outcome: MutationOutcome,
}
pub(crate) fn path_preview(path: &std::path::Path) -> String {
    bounded_text(path.to_str().unwrap_or("<native path>"), 256, 256, false).to_owned()
}

pub(crate) fn output_schema<T: JsonSchema + 'static>() -> Value {
    // rmcp's generic helper uses Schemars' deserialization contract. Outputs
    // must instead describe omitted empty fields and serialization-only fields.
    let generated = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<ToolOutput<T>>();
    let Value::Object(mut schema) =
        serde_json::to_value(generated).expect("generated output schema")
    else {
        unreachable!("output schema is an object");
    };
    schema.remove("title");
    schema.remove("description");
    // Every branch is an object. The 2025 outputSchema dialect requires an
    // explicit object root even when Schemars represents the branches as anyOf.
    schema.insert("type".into(), Value::String("object".into()));
    Value::Object(schema)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCode {
    InvalidInput,
    PathOutsideWorkspace,
    ParseRequired,
    StaleGeneration,
    NotFound,
    AmbiguousSymbol,
    UnsupportedUpstream,
    LimitExceeded,
    TimedOut,
    InvalidCapture,
    CaptureNotReady,
    RecoveryRequired,
    WorkspaceIntegrityViolation,
    PartialEvidence,
    HelperFailure,
    HelperChecksumMismatch,
    ExternalToolFailure,
    ToolNotAvailable,
    Internal,
}

#[derive(Clone, Debug, Serialize)]
pub struct DomainToolResult {
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Box<ToolResultMetadata>>,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(skip)]
    pub outcome: Option<Box<MutationOutcome>>,
    #[serde(skip)]
    pub json_pretty: Option<bool>,
    pub content: Vec<DomainContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ToolResultMetadata {
    pub analysis: crate::identity::AnalysisIdentity,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub(crate) struct SemanticCallError {
    pub error: anyhow::Error,
    pub analysis: crate::identity::AnalysisIdentity,
}
pub(crate) fn tool_error(error: anyhow::Error) -> ToolResult {
    let semantic = error.downcast_ref::<SemanticCallError>();
    let contextual = semantic.map_or(&error, |value| &value.error);
    let operation = contextual.downcast_ref::<OutcomeError>();
    let source = operation.map_or(contextual, |value| &value.error);
    let mut result = ToolResult::error(source.to_string());
    let trace_error = source
        .chain()
        .find_map(|error| error.downcast_ref::<crate::tracy_artifact::TraceSetError>());
    let paired = trace_error.and_then(|error| match error {
        crate::tracy_artifact::TraceSetError::PartialPublication {
            trace,
            trace_retained,
            rollback_complete,
            ..
        } => Some((trace, *trace_retained, *rollback_complete)),
        _ => None,
    });
    // Transparent and boxed error sources do not necessarily expose the inner
    // concrete type through Error::source. Preserve typed publication facts.
    let atomic = source
        .chain()
        .find_map(|error| error.downcast_ref::<crate::atomic_output::AtomicOutputError>())
        .or_else(|| match trace_error {
            Some(crate::tracy_artifact::TraceSetError::Atomic(error)) => Some(error),
            Some(crate::tracy_artifact::TraceSetError::PartialPublication { source, .. }) => {
                Some(source.as_ref())
            }
            _ => None,
        });
    if let Some(crate::atomic_output::AtomicOutputError::Installed {
        artifact,
        cleanup_complete,
        backup,
        ..
    }) = atomic
    {
        let outcome = MutationOutcome::installed(
            usize::from(paired.is_some()),
            artifact,
            *cleanup_complete,
            backup.as_deref(),
        );
        result.structured_content = Some(
            serde_json::to_value(PlainFailure {
                message: source.to_string(),
                outcome: Some(outcome.clone()),
            })
            .expect("installed error projection"),
        );
        let mut outcome = outcome;
        outcome.operation_succeeded = Some(false);
        result = result.with_outcome(outcome);
    }
    if let Some((trace, retained, rollback_complete)) = paired {
        let mut outcome = result.outcome.take().map(|v| *v).unwrap_or_default();
        outcome.operation_ran = true;
        outcome.operation_succeeded = Some(false);
        outcome.cleanup_complete = rollback_complete && outcome.cleanup_complete;
        outcome.recovery_required |= !rollback_complete;
        outcome.outputs.push(OutputMutation {
            request_index: 0,
            installed: retained,
            path_preview: Some(path_preview(&trace.path)),
            sha256: Some(trace.sha256.clone()),
            cleanup_complete: rollback_complete,
            backup_preview: None,
        });
        result = result.with_outcome(outcome);
    }
    if let Some(operation) = operation {
        let mut outcome = operation.outcome.clone();
        outcome.operation_succeeded = Some(false);
        if let Some(atomic) = result.outcome.take() {
            outcome.operation_ran |= atomic.operation_ran;
            outcome.outputs.extend(atomic.outputs);
            outcome.cleanup_complete &= atomic.cleanup_complete;
            outcome.recovery_required |= atomic.recovery_required;
        }
        result = result.with_outcome(outcome);
    }
    if let Some(semantic) = semantic {
        result = result.with_analysis(semantic.analysis.clone());
    }
    result
}

impl MutationOutcome {
    pub fn installed(
        index: usize,
        artifact: &crate::atomic_output::OutputArtifact,
        cleanup_complete: bool,
        backup: Option<&std::path::Path>,
    ) -> Self {
        let preview = |path: &std::path::Path| {
            bounded_text(path.to_str().unwrap_or("<native path>"), 256, 256, false).to_owned()
        };
        Self {
            operation_ran: true,
            cleanup_complete,
            recovery_required: !cleanup_complete,
            outputs: vec![OutputMutation {
                request_index: index,
                installed: true,
                path_preview: Some(preview(&artifact.path)),
                sha256: Some(artifact.sha256.clone()),
                cleanup_complete,
                backup_preview: backup.map(preview),
            }],
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum DomainContent {
    #[serde(rename = "text")]
    Text { text: String },
}

impl DomainToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            meta: None,
            structured_content: None,
            outcome: None,
            json_pretty: None,
            content: vec![DomainContent::Text { text: text.into() }],
            is_error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            meta: None,
            structured_content: Some(
                serde_json::to_value(PlainFailure {
                    message: message.clone(),
                    outcome: None,
                })
                .expect("plain failure serialization"),
            ),
            outcome: None,
            json_pretty: None,
            content: vec![DomainContent::Text { text: message }],
            is_error: Some(true),
        }
    }

    pub fn structured_error(
        code: &str,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        structured_error_value(
            Value::String(code.to_owned()),
            message.into(),
            Some(recovery.into()),
            Map::new(),
        )
    }
    pub fn with_analysis(mut self, analysis: crate::identity::AnalysisIdentity) -> Self {
        self.meta = Some(Box::new(ToolResultMetadata { analysis }));
        self
    }
    pub fn with_outcome(mut self, outcome: MutationOutcome) -> Self {
        if let Some(value) = self.structured_content.as_mut() {
            value.as_object_mut().expect("object projection").insert(
                "outcome".into(),
                serde_json::to_value(&outcome).expect("bounded outcome serialization"),
            );
            let pretty = self.json_pretty.unwrap_or(false);
            let text = if pretty {
                serde_json::to_string_pretty(value)
            } else {
                serde_json::to_string(value)
            }
            .expect("JSON projection serialization");
            self.content = vec![DomainContent::Text { text }];
            self.json_pretty = Some(pretty);
        }
        self.outcome = Some(Box::new(outcome));
        self
    }
}

pub fn json_success<T: Serialize>(metadata: ToolMetadata, data: T) -> ToolResult {
    encoded_success(metadata, data, true)
}

pub fn json_success_compact<T: Serialize>(metadata: ToolMetadata, data: T) -> ToolResult {
    encoded_success(metadata, data, false)
}

fn encoded_success<T: Serialize>(metadata: ToolMetadata, data: T, pretty: bool) -> ToolResult {
    let analysis = metadata.analysis.clone();
    let result = projection(Success { metadata, data }, pretty, false);
    match analysis {
        Some(analysis) => result.with_analysis(analysis),
        None => result,
    }
}

pub fn projection<T: Serialize>(data: T, pretty: bool, error: bool) -> ToolResult {
    let value = match serde_json::to_value(data) {
        Ok(value) => value,
        Err(error) => {
            return structured_error(
                ToolErrorCode::Internal,
                "could not serialize tool result",
                None,
                json!({ "serialization_error": error.to_string() }),
            );
        }
    };
    let Value::Object(_) = value else {
        return structured_error(
            ToolErrorCode::Internal,
            "tool success payload must be a JSON object",
            None,
            json!({ "payload_type": json_type_name(&value) }),
        );
    };
    let text = if pretty {
        serde_json::to_string_pretty(&value)
    } else {
        serde_json::to_string(&value)
    }
    .expect("JSON value serialization cannot fail");
    ToolResult {
        meta: None,
        outcome: None,
        json_pretty: Some(pretty),
        structured_content: Some(value),
        content: vec![DomainContent::Text { text }],
        is_error: error.then_some(true),
    }
}

pub fn structured_error(
    code: ToolErrorCode,
    message: impl Into<String>,
    recovery: Option<String>,
    details: Value,
) -> ToolResult {
    let details = match details {
        Value::Object(details) => details,
        value => Map::from_iter([("value".to_owned(), value)]),
    };
    structured_error_value(
        serde_json::to_value(code).expect("ToolErrorCode serialization cannot fail"),
        message.into(),
        recovery,
        details,
    )
}

fn structured_error_value(
    code: Value,
    message: String,
    recovery: Option<String>,
    details: Map<String, Value>,
) -> ToolResult {
    projection(
        ToolFailure {
            code: code.as_str().expect("error code is a string").to_owned(),
            message,
            recovery,
            details: details.into_iter().collect(),
            path: None,
        },
        false,
        true,
    )
}

/// Measure actual Serde JSON encoding without allocating an oversized string.
pub fn encoded_bytes<T: Serialize>(value: &T, maximum: usize) -> Option<usize> {
    struct Counter {
        bytes: usize,
        maximum: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|&size| size <= self.maximum)
                .ok_or_else(|| std::io::Error::other("encoded response exceeds budget"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, maximum };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.bytes)
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub type ToolResult = DomainToolResult;
pub type ToolContent = DomainContent;

// serde_json uses these escapes for string contents. Count UTF-8 and JSON bytes
// separately, since a captured control byte can become six response bytes.
fn json_char_bytes(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
        '\0'..='\u{1f}' => 6,
        _ => ch.len_utf8(),
    }
}

pub fn bounded_text(text: &str, raw_limit: usize, json_limit: usize, tail: bool) -> &str {
    let mut raw_bytes = 0;
    let mut json_bytes = 2; // quotes
    let mut add = |ch: char| {
        if raw_bytes + ch.len_utf8() > raw_limit || json_bytes + json_char_bytes(ch) > json_limit {
            return false;
        }
        raw_bytes += ch.len_utf8();
        json_bytes += json_char_bytes(ch);
        true
    };
    if tail {
        for ch in text.chars().rev() {
            if !add(ch) {
                break;
            }
        }
        &text[text.len() - raw_bytes..]
    } else {
        for ch in text.chars() {
            if !add(ch) {
                break;
            }
        }
        &text[..raw_bytes]
    }
}

pub(crate) struct JsonBudget(pub usize);
impl std::io::Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("projection exceeds serialized byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_output::{AtomicOutputError, OutputArtifact};
    use crate::tracy_artifact::TraceSetError;

    #[test]
    fn paired_publication_errors_keep_both_installed_outputs() {
        let artifact = |path: &str| OutputArtifact {
            path: path.into(),
            bytes: 12,
            sha256: "a".repeat(64),
        };
        let error = TraceSetError::PartialPublication {
            source: Box::new(AtomicOutputError::Installed {
                artifact: Box::new(artifact("capture.json")),
                cleanup_complete: false,
                backup: Some("retained-backup.json".into()),
                message: "backup cleanup failed".into(),
            }),
            trace: artifact("capture.tracy"),
            trace_retained: true,
            rollback_complete: false,
        };
        let result = tool_error(error.into());
        assert_eq!(result.is_error, Some(true));
        let native = result.structured_content.as_ref().unwrap();
        let DomainContent::Text { text } = &result.content[0];
        let text: Value = serde_json::from_str(text).unwrap();
        assert_eq!(&text, native);
        let outcome = &native["outcome"];
        assert_eq!(outcome["outputs"].as_array().unwrap().len(), 2);
        assert_eq!(outcome["outputs"][0]["request_index"], 1);
        assert_eq!(outcome["outputs"][0]["installed"], true);
        assert_eq!(outcome["outputs"][1]["installed"], true);
        assert_eq!(outcome["cleanup_complete"], false);
        assert_eq!(outcome["recovery_required"], true);
    }
}
