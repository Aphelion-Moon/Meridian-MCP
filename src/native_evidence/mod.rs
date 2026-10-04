pub mod byond;
pub mod csv;
pub mod jsonl;
pub mod model;
pub mod reader;
pub mod redaction;
pub mod statistics;
pub mod timeline;

use anyhow::{bail, Result};
use model::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone)]
pub struct NativeEvidenceContext {
    pub policy: crate::PathPolicy,
    pub provenance: Option<Arc<crate::BuildProvenanceStore>>,
}

pub fn validate_request(request: &NativeEvidenceRequest) -> Result<()> {
    crate::parameters::Request::validate(request).map_err(Into::into)
}

pub fn parse_artifact(
    context: &NativeEvidenceContext,
    descriptor: &ArtifactDescriptor,
    total: &mut u64,
    redacted: &mut u64,
) -> Result<ParsedArtifact> {
    let read = reader::read_artifact(&context.policy, descriptor, total)?;
    match descriptor.kind {
        ArtifactKind::ByondProcProfileJson | ArtifactKind::ByondSendmapsJson => {
            byond::parse(read, descriptor, redacted)
        }
        ArtifactKind::PerformanceCsv => csv::parse(read, descriptor, redacted),
        ArtifactKind::RuntimeJsonl | ArtifactKind::EventJsonl => {
            jsonl::parse(read, descriptor, redacted)
        }
    }
}

pub fn summarize_run(
    context: &NativeEvidenceContext,
    request: NativeEvidenceRequest,
) -> Result<NormalizedRun> {
    validate_request(&request)?;
    let mut total = 0;
    let mut redacted = 0;
    let mut canonical = BTreeSet::new();
    let mut parsed = Vec::new();
    for descriptor in &request.artifacts {
        let canonical_path = context.policy.read_path(&descriptor.path)?;
        if !canonical.insert(canonical_path) {
            bail!("evidence artifact paths must be unique");
        }
        parsed.push(parse_artifact(
            context,
            descriptor,
            &mut total,
            &mut redacted,
        )?);
    }
    let identity = run_identity(
        context,
        request.dmb_path.as_deref(),
        request.workload.clone(),
    )?;
    let artifacts = parsed
        .iter()
        .map(|artifact| artifact.identity.clone())
        .collect();
    let mut datasets = Vec::new();
    for (artifact_index, artifact) in parsed.into_iter().enumerate() {
        let assigned = artifact
            .records
            .iter()
            .map(|record| timeline::assign(record, &request.phases))
            .collect::<Result<Vec<_>>>()?;
        let game_start = request.phases.iter().find(|phase| phase.id == "game_start");
        let pre_game = artifact.semantics == EvidenceSemantics::CumulativeSnapshot
            && game_start.is_some_and(|phase| {
                let wall_start = timeline::wall_ms(phase.wall_start.as_deref())
                    .ok()
                    .flatten();
                let world_start = phase.world_start_ds;
                artifact.records.iter().all(|record| {
                    record
                        .wall_unix_ms
                        .zip(wall_start)
                        .is_some_and(|(value, start)| value < start)
                        || record
                            .world_deciseconds
                            .zip(world_start)
                            .is_some_and(|(value, start)| value < start)
                })
            });
        let full_records = artifact.records.iter().collect::<Vec<_>>();
        datasets.push(summarize_records(
            artifact_index,
            artifact.semantics,
            if pre_game {
                "pre_game_cumulative"
            } else if artifact.semantics == EvidenceSemantics::IntervalSeries {
                "full_interval"
            } else if artifact.semantics == EvidenceSemantics::EventStream {
                "full_event_stream"
            } else {
                "cumulative_snapshot"
            },
            None,
            &full_records,
            RecordCounts {
                rejected: artifact.rejected_records,
                assigned: assigned.iter().filter(|phase| phase.is_some()).count(),
                unassigned: assigned.iter().filter(|phase| phase.is_none()).count(),
            },
            artifact.unavailable_metrics.clone(),
        )?);
        if artifact.semantics != EvidenceSemantics::CumulativeSnapshot {
            for phase in &request.phases {
                let records = artifact
                    .records
                    .iter()
                    .zip(&assigned)
                    .filter_map(|(record, assigned)| {
                        (assigned.as_deref() == Some(phase.id.as_str())).then_some(record)
                    })
                    .collect::<Vec<_>>();
                if !records.is_empty() {
                    datasets.push(summarize_records(
                        artifact_index,
                        artifact.semantics,
                        "assigned",
                        Some(phase.id.clone()),
                        &records,
                        RecordCounts {
                            rejected: 0,
                            assigned: records.len(),
                            unassigned: 0,
                        },
                        artifact.unavailable_metrics.clone(),
                    )?);
                }
            }
        }
    }
    Ok(NormalizedRun {
        schema: 1,
        identity,
        artifacts,
        phases: request.phases,
        datasets,
        redaction: RedactionSummary {
            values_redacted: redacted,
            protected_fields: redaction::PROTECTED_FIELDS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        },
        warnings: Vec::new(),
    })
}

struct RecordCounts {
    rejected: usize,
    assigned: usize,
    unassigned: usize,
}

fn summarize_records(
    artifact: usize,
    semantics: EvidenceSemantics,
    classification: &str,
    assigned_phase: Option<String>,
    records: &[&EvidenceRecord],
    counts: RecordCounts,
    unavailable_metrics: Vec<String>,
) -> Result<DatasetSummary> {
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    let mut groups = BTreeMap::<String, u64>::new();
    for record in records {
        for (name, value) in &record.metrics {
            samples.entry(name.clone()).or_default().push(*value);
        }
        if !record.groups.is_empty() {
            let key = serde_json::to_string(&record.groups)?;
            *groups.entry(key).or_default() += 1;
            if groups.len() > crate::limits::MAX_EVIDENCE_GROUPS {
                bail!("evidence group limit exceeded");
            }
        }
    }
    let metrics = samples
        .into_iter()
        .filter_map(|(name, values)| {
            statistics::NumericSummary::from_samples_with_total(&values, records.len())
                .map(|summary| (name, summary))
        })
        .collect();
    let mut groups = groups
        .into_iter()
        .map(|(key, count)| GroupSummary { key, count })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| right.count.cmp(&left.count).then(left.key.cmp(&right.key)));
    groups.truncate(crate::limits::MAX_EVIDENCE_RETURNED_GROUPS);
    Ok(DatasetSummary {
        artifact,
        semantics,
        classification: classification.to_owned(),
        assigned_phase,
        raw_records: records.len() + counts.rejected,
        accepted_records: records.len(),
        rejected_records: counts.rejected,
        assigned_records: counts.assigned,
        unassigned_records: counts.unassigned,
        metrics,
        groups,
        unavailable_metrics,
    })
}

fn run_identity(
    context: &NativeEvidenceContext,
    dmb_path: Option<&std::path::Path>,
    workload: Option<WorkloadIdentityInput>,
) -> Result<NativeRunIdentity> {
    let Some(path) = dmb_path else {
        return Ok(NativeRunIdentity {
            identity_verification: "unavailable".to_owned(),
            build_record_id: None,
            dmb_sha256: None,
            workload,
        });
    };
    let file = crate::FileIdentity::capture(path)?;
    let decision = context
        .provenance
        .as_ref()
        .map(|store| store.evaluate_launch(path, false))
        .transpose()?;
    Ok(NativeRunIdentity {
        identity_verification: if decision
            .as_ref()
            .is_some_and(|decision| decision.status == crate::ProvenanceStatus::Verified)
        {
            "verified"
        } else {
            "unavailable"
        }
        .to_owned(),
        build_record_id: decision.and_then(|decision| decision.record_id),
        dmb_sha256: Some(file.sha256),
        workload,
    })
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ComparisonResult {
    pub schema: u32,
    pub build_record_id: String,
    pub run_count: usize,
    pub metrics: BTreeMap<String, ComparisonMetric>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct ComparisonMetric {
    pub values: Vec<f64>,
    pub absolute_delta: Option<f64>,
    pub percentage_delta: Option<f64>,
    pub coefficient_of_variation: Option<f64>,
    pub distribution: statistics::NumericSummary,
}

pub fn compare_runs(
    context: &NativeEvidenceContext,
    requests: Vec<NativeEvidenceRequest>,
) -> Result<ComparisonResult> {
    if requests.len() < 2 || requests.len() > crate::limits::MAX_EVIDENCE_COMPARISON_RUNS {
        bail!("comparison requires 2-20 runs");
    }
    let runs = requests
        .into_iter()
        .map(|request| summarize_run(context, request))
        .collect::<Result<Vec<_>>>()?;
    if runs
        .iter()
        .any(|run| run.identity.identity_verification != "verified")
    {
        bail!("evidence_identity_mismatch: every run requires verified build identity");
    }
    let record_id = runs[0].identity.build_record_id.clone().unwrap();
    if runs.iter().any(|run| {
        run.identity.build_record_id.as_deref() != Some(&record_id)
            || run.identity.workload != runs[0].identity.workload
            || run.identity.dmb_sha256 != runs[0].identity.dmb_sha256
            || run.phases != runs[0].phases
            || run
                .artifacts
                .iter()
                .map(|artifact| artifact.kind)
                .collect::<Vec<_>>()
                != runs[0]
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.kind)
                    .collect::<Vec<_>>()
    }) {
        bail!("evidence_identity_mismatch: build, workload, phase, or format identity differs");
    }
    let mut series = BTreeMap::<String, Vec<f64>>::new();
    for run in &runs {
        for dataset in &run.datasets {
            for (name, summary) in &dataset.metrics {
                let key = format!(
                    "{}|{:?}|{}|{}|{}",
                    dataset.artifact,
                    dataset.semantics,
                    dataset.classification,
                    dataset.assigned_phase.as_deref().unwrap_or("unassigned"),
                    name
                );
                series.entry(key).or_default().push(summary.mean);
            }
        }
    }
    let mut metrics = BTreeMap::new();
    for (key, values) in series {
        if values.len() != runs.len() {
            bail!("evidence_identity_mismatch: metric coverage differs between runs");
        }
        let distribution = statistics::NumericSummary::from_samples(&values).unwrap();
        let absolute_delta = (values.len() == 2).then(|| values[1] - values[0]);
        let percentage_delta = absolute_delta
            .filter(|_| values[0] != 0.0)
            .map(|delta| delta / values[0] * 100.0);
        let coefficient_of_variation =
            statistics::coefficient_of_variation(&distribution, values.len());
        metrics.insert(
            key,
            ComparisonMetric {
                values,
                absolute_delta,
                percentage_delta,
                coefficient_of_variation,
                distribution,
            },
        );
    }
    Ok(ComparisonResult {
        schema: 1,
        build_record_id: record_id,
        run_count: runs.len(),
        metrics,
    })
}
