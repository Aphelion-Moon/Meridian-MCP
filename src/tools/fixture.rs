use crate::analysis_snapshot::AnalysisSnapshot;
use crate::build_provenance::ProvenanceStatus;
use crate::fixture_manifest::{FixtureManifest, RequiredProcDocument, VerifiedFixtureManifest};
use crate::mcp::ToolResult;

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
    args: crate::parameters::CheckFixtureSyncParams,
) -> Result<ToolResult> {
    let issue_limit = args.issue_limit.unwrap_or(50) as usize;
    let params = args;
    let mut issues = Issues::new(issue_limit);
    let fixture = match load_manifest(
        context,
        state,
        std::path::Path::new(&params.fixture_manifest_path),
    )
    .await
    {
        Ok(fixture) => fixture,
        Err(error) => {
            let message = error.to_string();
            let excerpt = super::build_response::bounded_text(&message, 4096, 4096, false);
            issues.push(FixtureIssue {
                code: "fixture_manifest_invalid",
                path: &params.fixture_manifest_path,
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
    drop(snapshot);
    let policy = context.policy().clone();
    let limits = state.asset_limits().clone();
    let provenance_store = context.build_provenance_arc();
    let (fixture, present, provenance) = state
        .run_asset_job(move || {
            let present = required_tokens_present(
                &policy,
                &limits,
                &fixture.inputs,
                &fixture.required_tokens,
            )?;
            let provenance = provenance_store
                .map(|store| store.evaluate_launch(&fixture.dmb_path, false))
                .transpose()?;
            Ok((fixture, present, provenance))
        })
        .await?;
    for (token, present) in fixture.required_tokens.iter().zip(present) {
        if !present {
            issues.push(FixtureIssue {
                code: "required_token_missing",
                path: token,
                ..Default::default()
            });
        }
    }

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

pub(super) async fn load_manifest(
    context: &ToolExecutionContext,
    state: &ServerState,
    path: &Path,
) -> Result<VerifiedFixtureManifest> {
    let policy = context.policy().clone();
    let path = path.to_owned();
    let limits = state.asset_limits().clone();
    state
        .run_asset_job(move || Ok(FixtureManifest::load_with_limits(&policy, &path, &limits)?))
        .await
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
        let reusable = state
            .run_asset_job(move || Ok(super::parse::reusable_snapshot(Some(snapshot), &path)))
            .await?;
        if let Some(snapshot) = reusable {
            return Ok(snapshot);
        }
    }
    let temporary = state.isolated_analysis_state();
    let parsed = super::parse::parse_environment_with_policy(
        &temporary,
        crate::parameters::ParseEnvironmentParams {
            dme_path: dme_path.display().to_string(),
            force: None,
            timeout_ms: None,
        },
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
    policy: &crate::PathPolicy,
    limits: &crate::limits::ServerLimits,
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
    for input in inputs.iter().filter(|input| input.role.is_text()) {
        if unmatched.is_empty() {
            break;
        }
        // Keep one file's decoded text at a time. Read errors still abort the
        // check: unreadable input is not evidence that a token is missing.
        let bytes = input.read_verified_bytes(policy, limits)?;
        let text = crate::source::normalize_source_text(&bytes);
        unmatched.retain(|token| !text.contains(*token));
    }
    Ok(normalized
        .iter()
        .map(|token| !unmatched.contains(token.as_str()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture_manifest::{FixtureInputRole, VerifiedFixtureInput};
    use crate::limits::ServerLimits;
    use crate::{CapabilityMode, PathPolicy};
    use sha2::{Digest, Sha256};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn fixture_io_waits_for_blocking_job_admission() {
        let state = Arc::new(ServerState::with_limits(ServerLimits {
            max_blocking_jobs: 1,
            ..Default::default()
        }));
        let worker_state = Arc::clone(&state);
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let worker = tokio::spawn(async move {
            worker_state
                .run_asset_job(move || {
                    let _ = started.send(());
                    let _ = released.recv();
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![root.to_owned()], Vec::new()).unwrap(),
        );
        let args = json!({"fixture_manifest_path": root.join("Cargo.toml")});
        let premature = tokio::time::timeout(
            Duration::from_millis(50),
            check_sync(
                &context,
                &state,
                crate::parameters::decode(args.clone()).expect("valid fixture request"),
            ),
        )
        .await;
        drop(release);
        worker.await.unwrap().unwrap();
        assert!(
            premature.is_err(),
            "fixture manifest I/O bypassed the occupied blocking-job pool"
        );
        let result = check_sync(
            &context,
            &state,
            crate::parameters::decode(args).expect("valid fixture request"),
        )
        .await
        .unwrap();
        let crate::result::ToolContent::Text { text } = &result.content[0];
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap()["classification"],
            "invalid"
        );
    }

    #[test]
    fn fixture_token_scan_rejects_input_changed_since_hashing() {
        let root = std::env::temp_dir().join(format!(
            "meridian-fixture-token-identity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("input.dm");
        std::fs::write(&path, b"same").unwrap();
        let input = VerifiedFixtureInput {
            relative_path: "input.dm".into(),
            canonical_path: path.canonicalize().unwrap(),
            role: FixtureInputRole::Source,
            size: 4,
            sha256: format!("{:x}", Sha256::digest(b"same")),
        };
        let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
        let limits = ServerLimits::default();
        let mut outcomes = Vec::new();
        for contents in ["DIFF", "same-and-more", ""] {
            std::fs::write(&path, contents).unwrap();
            outcomes.push(required_tokens_present(
                &policy,
                &limits,
                std::slice::from_ref(&input),
                &["DIFF".into()],
            ));
        }
        std::fs::remove_dir_all(root).unwrap();
        for outcome in outcomes {
            assert!(
                outcome.is_err(),
                "token validation accepted bytes different from the recorded input identity"
            );
        }
    }

    #[tokio::test]
    async fn fixture_parse_uses_shared_admission_without_installing_its_snapshot() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provenance");
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
        );
        let state = ServerState::new();
        let permit = state.parse_permit().await;
        let dme = root.join("fixture.dme");
        let premature = tokio::time::timeout(
            Duration::from_millis(200),
            matching_or_fixture_snapshot(&context, &state, &dme),
        )
        .await;
        drop(permit);
        assert!(
            premature.is_err(),
            "fixture parsing bypassed the occupied server parse admission"
        );
        let snapshot = matching_or_fixture_snapshot(&context, &state, &dme)
            .await
            .unwrap();
        assert_eq!(
            snapshot.environment_path.canonicalize().unwrap(),
            dme.canonicalize().unwrap()
        );
        assert!(state.active_snapshot().await.is_none());
    }
}
