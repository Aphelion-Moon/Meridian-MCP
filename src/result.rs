use crate::capabilities::SPACEMANDMM_REVISION;
use serde::Serialize;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

pub(crate) fn analysis_text(
    snapshot: &crate::analysis_snapshot::AnalysisSnapshot,
    mut payload: Value,
) -> anyhow::Result<ToolResult> {
    payload["analysis"] = serde_json::to_value(snapshot.identity())?;
    Ok(
        ToolResult::text(serde_json::to_string_pretty(&payload)?)
            .with_analysis(snapshot.identity()),
    )
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

#[derive(Debug, Serialize)]
pub struct DomainToolResult {
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Box<ToolResultMetadata>>,
    pub content: Vec<DomainContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
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
    match error.downcast_ref::<SemanticCallError>() {
        Some(semantic) => {
            ToolResult::error(semantic.error.to_string()).with_analysis(semantic.analysis.clone())
        }
        None => ToolResult::error(error.to_string()),
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum DomainContent {
    #[serde(rename = "text")]
    Text { text: String },
}

impl DomainToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            meta: None,
            content: vec![DomainContent::Text { text: text.into() }],
            is_error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            meta: None,
            content: vec![DomainContent::Text {
                text: message.into(),
            }],
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
}

pub fn json_success<T: Serialize>(metadata: ToolMetadata, data: T) -> ToolResult {
    encoded_success(metadata, data, true)
}

pub fn json_success_compact<T: Serialize>(metadata: ToolMetadata, data: T) -> ToolResult {
    encoded_success(metadata, data, false)
}

fn encoded_success<T: Serialize>(metadata: ToolMetadata, data: T, pretty: bool) -> ToolResult {
    let analysis = metadata.analysis.clone();
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
    let Value::Object(mut payload) = value else {
        return structured_error(
            ToolErrorCode::Internal,
            "tool success payload must be a JSON object",
            None,
            json!({ "payload_type": json_type_name(&value) }),
        );
    };
    let Value::Object(metadata) =
        serde_json::to_value(metadata).expect("ToolMetadata serialization cannot fail")
    else {
        unreachable!("ToolMetadata must serialize as an object");
    };
    for (key, value) in metadata {
        payload.insert(key, value);
    }
    let payload = Value::Object(payload);
    let result = ToolResult::text(
        if pretty {
            serde_json::to_string_pretty(&payload)
        } else {
            serde_json::to_string(&payload)
        }
        .expect("JSON value serialization cannot fail"),
    );
    match analysis {
        Some(analysis) => result.with_analysis(analysis),
        None => result,
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
    ToolResult::error(
        json!({
            "code": code,
            "message": message,
            "recovery": recovery,
            "details": details,
        })
        .to_string(),
    )
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
