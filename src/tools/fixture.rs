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
use std::path::Path;
use std::sync::Arc;

#[derive(Serialize)]
struct FixtureIssue {
    code: &'static str,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_arguments: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_arguments: Option<Vec<String>>,
}

pub async fn check_sync(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: Value,
) -> Result<ToolResult> {
    let params: FixtureSyncParams = serde_json::from_value(args)
        .map_err(|error| anyhow!("invalid fixture sync arguments: {error}"))?;
    let fixture = match FixtureManifest::load(context.policy(), &params.fixture_manifest_path) {
        Ok(fixture) => fixture,
        Err(error) => {
            return Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
                "classification": "invalid",
                "issues": [{"code": "fixture_manifest_invalid", "path": params.fixture_manifest_path, "message": error.to_string()}]
            }))?));
        }
    };

    let snapshot = matching_or_fixture_snapshot(context, state, &fixture.dme_path).await?;
    let mut issues = Vec::new();
    for required in &fixture.required_procs {
        check_required_proc(&snapshot, required, &mut issues);
    }
    for token in &fixture.required_tokens {
        if !required_token_present(&fixture.inputs, token)? {
            issues.push(FixtureIssue {
                code: "required_token_missing",
                path: token.clone(),
                expected_arguments: None,
                actual_arguments: None,
            });
        }
    }

    let provenance = context
        .build_provenance()
        .map(|store| store.evaluate_launch(&fixture.dmb_path, false))
        .transpose()?;
    let classification = if !issues.is_empty() {
        "invalid"
    } else if provenance
        .as_ref()
        .is_some_and(|decision| decision.status == ProvenanceStatus::Stale)
    {
        "stale"
    } else {
        "verified"
    };

    Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
        "classification": classification,
        "fixture_id": fixture.fixture_id,
        "fixture_manifest_sha256": fixture.identity_sha256,
        "environment_path": fixture.dme_path,
        "dmb_path": fixture.dmb_path,
        "issues": issues,
        "provenance_status": provenance.as_ref().map(|decision| decision.status).unwrap_or(ProvenanceStatus::Unverified),
        "build_record_id": provenance.as_ref().and_then(|decision| decision.record_id.as_deref()),
        "provenance_reasons": provenance.map(|decision| decision.reasons).unwrap_or_default(),
    }))?))
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
    issues: &mut Vec<FixtureIssue>,
) {
    let Some((owner, proc_name)) = split_proc_path(&required.path) else {
        issues.push(FixtureIssue {
            code: "required_proc_missing",
            path: required.path.clone(),
            expected_arguments: None,
            actual_arguments: None,
        });
        return;
    };
    let resolution = [owner, if owner.is_empty() { "/" } else { owner }]
        .into_iter()
        .find_map(|candidate| snapshot.proc_resolver().resolve(candidate, proc_name).ok());
    let Some(resolution) = resolution else {
        issues.push(FixtureIssue {
            code: "required_proc_missing",
            path: required.path.clone(),
            expected_arguments: None,
            actual_arguments: None,
        });
        return;
    };
    let actual = resolution
        .implementations
        .first()
        .map(|implementation| implementation.parameters.clone())
        .unwrap_or_default();
    if actual != required.arguments {
        issues.push(FixtureIssue {
            code: "required_proc_arguments_mismatch",
            path: required.path.clone(),
            expected_arguments: Some(required.arguments.clone()),
            actual_arguments: Some(actual),
        });
    }
}

fn split_proc_path(path: &str) -> Option<(&str, &str)> {
    let (owner, proc_name) = path.rsplit_once("/proc/")?;
    (!proc_name.is_empty()).then_some((owner, proc_name))
}

fn required_token_present(
    inputs: &[crate::fixture_manifest::VerifiedFixtureInput],
    token: &str,
) -> Result<bool> {
    let token = token.replace("\r\n", "\n");
    for input in inputs.iter().filter(|input| {
        matches!(
            input.role,
            FixtureInputRole::Source
                | FixtureInputRole::GeneratedBinding
                | FixtureInputRole::Configuration
        )
    }) {
        let text = std::fs::read_to_string(&input.canonical_path)?.replace("\r\n", "\n");
        if text.contains(&token) {
            return Ok(true);
        }
    }
    Ok(false)
}
