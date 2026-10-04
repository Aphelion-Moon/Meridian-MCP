use crate::analysis_snapshot::AnalysisSnapshot;
use crate::build_provenance::ProvenanceStatus;
use crate::fixture_manifest::{FixtureInputRole, FixtureManifest, RequiredProcDocument};
use crate::mcp::ToolResult;
use crate::parameters::FixtureSyncParams;
use crate::state::ServerState;
use crate::tools::ToolExecutionContext;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

const ISSUE_JSON_BYTES: usize = 128 * 1024;
const ARGUMENT_JSON_BYTES: usize = 8 * 1024;

#[derive(Default, Serialize)]
struct FixtureIssue<'a> {
    code: &'static str,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_arguments: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_arguments: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_arguments_omitted: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_arguments_omitted: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_truncated: Option<bool>,
}

struct JsonBudget(usize);

impl Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| {
            std::io::Error::other("fixture detail exceeds its serialized byte budget")
        })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Issues {
    rows: Vec<Value>,
    total: usize,
    limit: usize,
    bytes: usize,
    details_omitted: bool,
    full: bool,
}

impl Issues {
    fn new(limit: usize) -> Self {
        Self {
            rows: Vec::new(),
            total: 0,
            limit,
            bytes: 2,
            details_omitted: false,
            full: false,
        }
    }

    fn push(&mut self, mut issue: FixtureIssue<'_>) {
        self.total += 1;
        if self.full || self.rows.len() >= self.limit {
            return;
        }
        // Check borrowed arguments before constructing retained JSON. Large
        // signatures must not allocate copies merely to discard them later.
        for (arguments, omitted) in [
            (
                &mut issue.expected_arguments,
                &mut issue.expected_arguments_omitted,
            ),
            (
                &mut issue.actual_arguments,
                &mut issue.actual_arguments_omitted,
            ),
        ] {
            if let Some(values) = *arguments {
                if serde_json::to_writer(JsonBudget(ARGUMENT_JSON_BYTES), values).is_err() {
                    *omitted = Some(values.len());
                    *arguments = None;
                }
            }
        }
        let mut budget = JsonBudget(ISSUE_JSON_BYTES.saturating_sub(self.bytes + 1));
        if serde_json::to_writer(&mut budget, &issue).is_err() {
            self.full = true;
            return;
        }
        self.bytes = ISSUE_JSON_BYTES - budget.0;
        self.details_omitted |= issue.expected_arguments_omitted.is_some()
            || issue.actual_arguments_omitted.is_some()
            || issue.message_truncated == Some(true);
        self.rows
            .push(serde_json::to_value(issue).expect("fixture issue serialization"));
    }

    fn respond(self, mut metadata: Value) -> Result<ToolResult> {
        super::build_response::bound_metadata(&mut metadata);
        metadata["truncated"] = json!(
            self.total != self.rows.len()
                || self.details_omitted
                || metadata.get("response_omissions").is_some()
        );
        metadata["issues_summary"] = json!({
            "total": self.total,
            "returned": self.rows.len(),
            "omitted": self.total - self.rows.len(),
        });
        metadata["issues"] = json!(self.rows);
        Ok(ToolResult::text(serde_json::to_string(&metadata)?))
    }
}

pub async fn check_sync(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let issue_limit = super::bounded_u64(&args, "issue_limit", 50, 0, 200)? as usize;
    let params: FixtureSyncParams = serde_json::from_value(args)
        .map_err(|error| anyhow!("invalid fixture sync arguments: {error}"))?;
    let mut issues = Issues::new(issue_limit);
    let fixture = match FixtureManifest::load(context.policy(), &params.fixture_manifest_path) {
        Ok(fixture) => fixture,
        Err(error) => {
            let message = error.to_string();
            let excerpt = super::build_response::bounded_text(&message, 4096, 4096, false);
            issues.push(FixtureIssue {
                code: "fixture_manifest_invalid",
                path: &params.fixture_manifest_path.to_string_lossy(),
                message: Some(excerpt),
                message_truncated: (excerpt.len() != message.len()).then_some(true),
                ..Default::default()
            });
            return issues.respond(json!({
                "classification": "invalid",
                "validation_complete": false,
            }));
        }
    };

    let snapshot = matching_or_fixture_snapshot(context, state, &fixture.dme_path).await?;
    for required in &fixture.required_procs {
        check_required_proc(&snapshot, required, &mut issues);
    }
    let present = required_tokens_present(&fixture.inputs, &fixture.required_tokens)?;
    for (token, present) in fixture.required_tokens.iter().zip(present) {
        if !present {
            issues.push(FixtureIssue {
                code: "required_token_missing",
                path: token,
                ..Default::default()
            });
        }
    }

    let provenance = context
        .build_provenance()
        .map(|store| store.evaluate_launch(&fixture.dmb_path, false))
        .transpose()?;
    let classification = if issues.total != 0 {
        "invalid"
    } else if provenance
        .as_ref()
        .is_some_and(|decision| decision.status == ProvenanceStatus::Stale)
    {
        "stale"
    } else {
        "verified"
    };

    issues.respond(json!({
        "classification": classification,
        "validation_complete": true,
        "fixture_id": fixture.fixture_id,
        "fixture_manifest_sha256": fixture.identity_sha256,
        "environment_path": fixture.dme_path,
        "dmb_path": fixture.dmb_path,
        "provenance_status": provenance.as_ref().map(|decision| decision.status).unwrap_or(ProvenanceStatus::Unverified),
        "build_record_id": provenance.as_ref().and_then(|decision| decision.record_id.as_deref()),
        "provenance_reasons": provenance.map(|decision| decision.reasons).unwrap_or_default(),
    }))
}

async fn matching_or_fixture_snapshot(
    context: &ToolExecutionContext,
    state: &ServerState,
    dme_path: &Path,
) -> Result<Arc<AnalysisSnapshot>> {
    if let Some(snapshot) = state.active_snapshot().await {
        let path = dme_path.to_owned();
        // Match the parser's freshness rules, including DME/configuration and
        // parsed inputs absent from the fixture manifest. Filesystem checks
        // belong on the blocking pool, just as they do for an explicit parse.
        let reusable = tokio::task::spawn_blocking(move || {
            super::parse::reusable_snapshot(Some(snapshot), &path)
        })
        .await?;
        if let Some(snapshot) = reusable {
            return Ok(snapshot);
        }
    }
    let temporary = ServerState::new();
    let parsed = super::parse::parse_environment_with_policy(
        &temporary,
        json!({"dme_path": dme_path.display().to_string()}),
        context.policy(),
    )
    .await?;
    if parsed.is_error == Some(true) {
        return Err(anyhow!("fixture DreamMaker parse failed"));
    }
    Ok(temporary.snapshot().await?)
}

fn check_required_proc(
    snapshot: &AnalysisSnapshot,
    required: &RequiredProcDocument,
    issues: &mut Issues,
) {
    let Some((owner, proc_name)) = split_proc_path(&required.path) else {
        issues.push(FixtureIssue {
            code: "required_proc_missing",
            path: &required.path,
            ..Default::default()
        });
        return;
    };
    let resolution = [owner, if owner.is_empty() { "/" } else { owner }]
        .into_iter()
        .find_map(|candidate| snapshot.proc_resolver().resolve(candidate, proc_name).ok());
    let Some(resolution) = resolution else {
        issues.push(FixtureIssue {
            code: "required_proc_missing",
            path: &required.path,
            ..Default::default()
        });
        return;
    };
    let actual = resolution
        .implementations
        .first()
        .map(|implementation| implementation.parameters.as_slice())
        .unwrap_or_default();
    if actual != required.arguments {
        issues.push(FixtureIssue {
            code: "required_proc_arguments_mismatch",
            path: &required.path,
            expected_arguments: Some(&required.arguments),
            actual_arguments: Some(actual),
            ..Default::default()
        });
    }
}

fn split_proc_path(path: &str) -> Option<(&str, &str)> {
    let (owner, proc_name) = path.rsplit_once("/proc/")?;
    (!proc_name.is_empty()).then_some((owner, proc_name))
}

fn required_tokens_present(
    inputs: &[crate::fixture_manifest::VerifiedFixtureInput],
    tokens: &[String],
) -> Result<Vec<bool>> {
    let normalized = tokens
        .iter()
        .map(|token| token.replace("\r\n", "\n"))
        .collect::<Vec<_>>();
    let mut unmatched = normalized
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for input in inputs.iter().filter(|input| {
        matches!(
            input.role,
            FixtureInputRole::Source
                | FixtureInputRole::GeneratedBinding
                | FixtureInputRole::Configuration
        )
    }) {
        if unmatched.is_empty() {
            break;
        }
        // Keep one file's decoded text at a time. Read errors still abort the
        // check: unreadable input is not evidence that a token is missing.
        let bytes = std::fs::read(&input.canonical_path)?;
        let text = crate::source::normalize_source_text(&bytes);
        unmatched.retain(|token| !text.contains(*token));
    }
    Ok(normalized
        .iter()
        .map(|token| !unmatched.contains(token.as_str()))
        .collect())
}
