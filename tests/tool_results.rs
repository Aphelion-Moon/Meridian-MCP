use meridian_mcp::result::{
    json_success, structured_error, DomainContent, ToolErrorCode, ToolMetadata,
};
use serde_json::{json, Value};

fn payload(result: meridian_mcp::result::ToolResult) -> Value {
    let DomainContent::Text { text } = result.content.into_iter().next().unwrap();
    serde_json::from_str(&text).unwrap()
}

fn metadata() -> ToolMetadata {
    ToolMetadata {
        meridian_mcp_version: env!("CARGO_PKG_VERSION"),
        meridian_mcp_build: meridian_mcp::build_identity::current().clone(),
        spacemandmm_revision: "351ddc0ffb2439876d4565ce5130bb6b027ee605",
        state_generation: Some(7),
        analysis: None,
        runtime_id: None,
        asset_generation: None,
        truncated: false,
        truncation_reasons: Vec::new(),
    }
}

#[test]
fn error_codes_serialize_as_stable_snake_case() {
    let cases = [
        (ToolErrorCode::InvalidInput, "invalid_input"),
        (
            ToolErrorCode::PathOutsideWorkspace,
            "path_outside_workspace",
        ),
        (ToolErrorCode::ParseRequired, "parse_required"),
        (ToolErrorCode::StaleGeneration, "stale_generation"),
        (ToolErrorCode::NotFound, "not_found"),
        (ToolErrorCode::AmbiguousSymbol, "ambiguous_symbol"),
        (ToolErrorCode::UnsupportedUpstream, "unsupported_upstream"),
        (ToolErrorCode::LimitExceeded, "limit_exceeded"),
        (ToolErrorCode::TimedOut, "timed_out"),
        (ToolErrorCode::InvalidCapture, "invalid_capture"),
        (ToolErrorCode::CaptureNotReady, "capture_not_ready"),
        (ToolErrorCode::RecoveryRequired, "recovery_required"),
        (
            ToolErrorCode::WorkspaceIntegrityViolation,
            "workspace_integrity_violation",
        ),
        (ToolErrorCode::PartialEvidence, "partial_evidence"),
        (ToolErrorCode::HelperFailure, "helper_failure"),
        (
            ToolErrorCode::HelperChecksumMismatch,
            "helper_checksum_mismatch",
        ),
        (ToolErrorCode::ExternalToolFailure, "external_tool_failure"),
        (ToolErrorCode::ToolNotAvailable, "tool_not_available"),
        (ToolErrorCode::Internal, "internal"),
    ];

    for (code, expected) in cases {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(expected));
    }
}

#[test]
fn success_results_merge_trusted_metadata_with_existing_payload_fields() {
    let result = json_success(metadata(), json!({ "count": 2, "items": ["one", "two"] }));
    assert_eq!(result.is_error, None);
    let value = payload(result);

    assert_eq!(value["meridian_mcp_version"], "0.1.0");
    assert_eq!(value["meridian_mcp_build"]["schema"], 1);
    assert_eq!(
        value["meridian_mcp_build"]["executable_sha256"]
            .as_str()
            .map(str::len),
        Some(64)
    );
    assert_eq!(
        value["spacemandmm_revision"],
        "351ddc0ffb2439876d4565ce5130bb6b027ee605"
    );
    assert_eq!(value["state_generation"], 7);
    assert_eq!(value["truncated"], false);
    assert_eq!(value["truncation_reasons"], json!([]));
    assert_eq!(value["count"], 2);
    assert_eq!(value["items"], json!(["one", "two"]));
}

#[test]
fn structured_errors_always_include_recovery_and_object_details() {
    let result = structured_error(
        ToolErrorCode::PathOutsideWorkspace,
        "outside configured roots",
        Some("Choose a path beneath MERIDIAN_MCP_ROOTS.".to_owned()),
        json!({ "path": "C:/outside.dm" }),
    );
    assert_eq!(result.is_error, Some(true));
    let value = payload(result);

    assert_eq!(value["code"], "path_outside_workspace");
    assert_eq!(value["message"], "outside configured roots");
    assert_eq!(
        value["recovery"],
        "Choose a path beneath MERIDIAN_MCP_ROOTS."
    );
    assert_eq!(value["details"], json!({ "path": "C:/outside.dm" }));
}

#[test]
fn non_object_success_payloads_fail_closed() {
    let result = json_success(metadata(), json!(["not", "an", "object"]));
    assert_eq!(result.is_error, Some(true));
    let value = payload(result);

    assert_eq!(value["code"], "internal");
    assert!(value["details"].is_object());
}
