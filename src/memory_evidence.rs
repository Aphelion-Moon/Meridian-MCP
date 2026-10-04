use crate::process_metrics::{MemorySample, ProcessIdentity, RoleMemorySeries};
use crate::PathPolicy;
use anyhow::{bail, ensure, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SAMPLES: usize = 100_000;
pub(crate) const MAX_SAMPLE_LIMIT: usize = 100;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryRequest {
    /// Existing schema-2 Tracy capture sidecar or completed experiment JSON.
    pub evidence_path: PathBuf,
    /// Inclusive offset from experiment launch, in milliseconds.
    pub begin_ms: Option<u64>,
    /// Exclusive offset from experiment launch, in milliseconds.
    pub end_ms: Option<u64>,
    /// Maximum returned samples per metric, from 0 to 100. Statistics use all selected samples.
    #[serde(default = "default_sample_limit")]
    #[schemars(range(min = 0, max = MAX_SAMPLE_LIMIT))]
    pub sample_limit: usize,
}

fn default_sample_limit() -> usize {
    20
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryCompareRequest {
    pub baseline: MemoryRequest,
    pub current: MemoryRequest,
    /// Permit descriptive comparisons between different phase labels.
    #[serde(default)]
    pub allow_different_phases: bool,
}

#[derive(Deserialize)]
struct EvidenceDocument {
    schema: u32,
    #[serde(default)]
    experiment_identity: Value,
    #[serde(default)]
    meridian_mcp_build: Value,
    phase: Option<String>,
    memory_series: Vec<RoleMemorySeries>,
}

#[derive(Serialize)]
pub struct MetricSummary {
    pub metric_kind: String,
    pub unit: &'static str,
    pub sample_count: usize,
    pub first_offset_ms: u64,
    pub last_offset_ms: u64,
    pub observed_span_ms: u64,
    pub first_bytes: u64,
    pub last_bytes: u64,
    pub minimum_bytes: u64,
    pub peak_bytes: u64,
    pub peak_offset_ms: u64,
    pub net_change_bytes: i64,
    pub net_bytes_per_second: Option<f64>,
    pub largest_rise_bytes: u64,
    pub largest_fall_bytes: u64,
    pub maximum_interval_ms: u64,
    pub gap_count: usize,
    pub samples: Vec<MemorySample>,
    pub samples_truncated: bool,
}

#[derive(Serialize)]
pub struct SeriesSummary {
    pub identity: ProcessIdentity,
    pub operating_system: String,
    pub sampling_interval_ms: u64,
    pub missed_samples: u64,
    pub metrics: Vec<MetricSummary>,
}

#[derive(Serialize)]
pub struct MemorySummary {
    pub schema: u32,
    pub evidence_sha256: String,
    pub evidence_bytes: usize,
    pub identity_verification: &'static str,
    pub allocation_attribution: &'static str,
    pub phase: Option<String>,
    pub begin_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub series: Vec<SeriesSummary>,
    pub warnings: Vec<&'static str>,
    #[serde(skip)]
    recorded_identity: Value,
    #[serde(skip)]
    build_id: Value,
}

pub fn summarize(policy: &PathPolicy, request: MemoryRequest) -> Result<MemorySummary> {
    ensure!(
        request.sample_limit <= MAX_SAMPLE_LIMIT,
        "sample_limit must be between 0 and 100"
    );
    ensure!(
        request
            .end_ms
            .is_none_or(|end| end > request.begin_ms.unwrap_or(0)),
        "end_ms must be greater than begin_ms"
    );
    let path = policy.read_path(&request.evidence_path)?;
    ensure!(
        std::fs::metadata(&path)?.is_file(),
        "evidence must be a regular file"
    );
    let file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "evidence must be a regular file"
    );
    ensure!(
        file.metadata()?.len() <= MAX_DOCUMENT_BYTES,
        "memory evidence exceeds the 16 MiB limit"
    );
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_DOCUMENT_BYTES,
        "memory evidence exceeds the 16 MiB limit"
    );
    let document: EvidenceDocument = serde_json::from_slice(&bytes)?;
    ensure!(document.schema == 2, "memory evidence requires schema 2");
    ensure!(
        (1..=4).contains(&document.memory_series.len()),
        "memory evidence requires 1-4 process series"
    );
    ensure!(
        document
            .phase
            .as_ref()
            .is_none_or(|phase| !phase.is_empty() && phase.len() <= 128),
        "invalid phase label"
    );
    ensure!(
        document
            .memory_series
            .iter()
            .map(|series| series.samples.len())
            .sum::<usize>()
            <= MAX_SAMPLES,
        "memory evidence exceeds 100000 samples"
    );
    let mut roles = HashSet::new();
    let mut series = Vec::new();
    let mut warnings = vec![
        "process_totals_do_not_prove_leaks_or_retained_object_bytes",
        "sampled_peaks_may_miss_between_sample_spikes",
    ];
    for input in document.memory_series {
        ensure!(roles.insert(input.identity.role), "duplicate process role");
        ensure!(
            input.identity.pid != 0 && input.identity.started_at_identity != 0,
            "missing process identity"
        );
        ensure!(
            matches!(input.operating_system.as_str(), "windows" | "linux"),
            "unsupported memory operating system"
        );
        ensure!(
            (1..=60_000).contains(&input.sampling_interval_ms),
            "invalid sampling interval"
        );
        let mut grouped = BTreeMap::<String, Vec<MemorySample>>::new();
        let mut last_offsets = BTreeMap::new();
        for sample in input.samples {
            ensure!(
                sample.observed_value <= i64::MAX as u64,
                "memory value exceeds signed byte range"
            );
            let key = serde_json::to_value(sample.metric_kind)?
                .as_str()
                .unwrap()
                .to_owned();
            ensure!(
                match input.operating_system.as_str() {
                    "windows" => matches!(
                        key.as_str(),
                        "working_set_bytes" | "private_bytes" | "virtual_bytes"
                    ),
                    "linux" => matches!(key.as_str(), "rss_bytes" | "virtual_bytes"),
                    _ => false,
                },
                "metric does not match operating system"
            );
            if let Some(previous) = last_offsets.insert(key.clone(), sample.monotonic_offset_ms) {
                ensure!(
                    sample.monotonic_offset_ms > previous,
                    "metric timestamps must strictly increase"
                );
            }
            if sample.monotonic_offset_ms >= request.begin_ms.unwrap_or(0)
                && request
                    .end_ms
                    .is_none_or(|end| sample.monotonic_offset_ms < end)
            {
                grouped.entry(key).or_default().push(sample);
            }
        }
        let metrics = grouped
            .into_iter()
            .map(|(key, samples)| {
                summarize_metric(
                    key,
                    samples,
                    input.sampling_interval_ms,
                    request.sample_limit,
                )
            })
            .collect::<Vec<_>>();
        if (input.missed_samples > 0 || metrics.iter().any(|metric| metric.gap_count > 0))
            && !warnings.contains(&"incomplete_sampling")
        {
            warnings.push("incomplete_sampling");
        }
        if metrics.is_empty() && !warnings.contains(&"no_samples_in_selected_window") {
            warnings.push("no_samples_in_selected_window");
        }
        series.push(SeriesSummary {
            identity: input.identity,
            operating_system: input.operating_system,
            sampling_interval_ms: input.sampling_interval_ms,
            missed_samples: input.missed_samples,
            metrics,
        });
    }
    series.sort_by_key(|series| format!("{:?}", series.identity.role));
    Ok(MemorySummary {
        schema: 1,
        evidence_sha256: format!("{:x}", Sha256::digest(&bytes)),
        evidence_bytes: bytes.len(),
        identity_verification: "recorded_not_verified",
        allocation_attribution: "unavailable",
        phase: document.phase,
        begin_ms: request.begin_ms,
        end_ms: request.end_ms,
        series,
        warnings,
        recorded_identity: document.experiment_identity,
        build_id: document.meridian_mcp_build["build_id"].clone(),
    })
}

fn summarize_metric(
    key: String,
    samples: Vec<MemorySample>,
    interval: u64,
    limit: usize,
) -> MetricSummary {
    let first = &samples[0];
    let last = samples.last().unwrap();
    let peak = samples
        .iter()
        .max_by_key(|sample| sample.observed_value)
        .unwrap();
    let span = last.monotonic_offset_ms - first.monotonic_offset_ms;
    let delta = last.observed_value as i64 - first.observed_value as i64;
    let mut result = MetricSummary {
        metric_kind: key,
        unit: "bytes",
        sample_count: samples.len(),
        first_offset_ms: first.monotonic_offset_ms,
        last_offset_ms: last.monotonic_offset_ms,
        observed_span_ms: span,
        first_bytes: first.observed_value,
        last_bytes: last.observed_value,
        minimum_bytes: samples
            .iter()
            .map(|sample| sample.observed_value)
            .min()
            .unwrap(),
        peak_bytes: peak.observed_value,
        peak_offset_ms: peak.monotonic_offset_ms,
        net_change_bytes: delta,
        net_bytes_per_second: (span > 0).then(|| delta as f64 * 1000.0 / span as f64),
        largest_rise_bytes: 0,
        largest_fall_bytes: 0,
        maximum_interval_ms: 0,
        gap_count: 0,
        samples: samples.iter().take(limit).cloned().collect(),
        samples_truncated: samples.len() > limit,
    };
    for pair in samples.windows(2) {
        let gap = pair[1].monotonic_offset_ms - pair[0].monotonic_offset_ms;
        result.maximum_interval_ms = result.maximum_interval_ms.max(gap);
        result.gap_count += usize::from(gap > interval * 2);
        result.largest_rise_bytes = result.largest_rise_bytes.max(
            pair[1]
                .observed_value
                .saturating_sub(pair[0].observed_value),
        );
        result.largest_fall_bytes = result.largest_fall_bytes.max(
            pair[0]
                .observed_value
                .saturating_sub(pair[1].observed_value),
        );
    }
    result
}

pub fn compare(policy: &PathPolicy, request: MemoryCompareRequest) -> Result<Value> {
    let baseline = summarize(policy, request.baseline)?;
    let current = summarize(policy, request.current)?;
    for field in ["executable", "workload"] {
        let id = if field == "executable" {
            "executable_id"
        } else {
            "workload_id"
        };
        if baseline.recorded_identity[field][id]
            .as_str()
            .is_none_or(str::is_empty)
            || baseline.recorded_identity[field] != current.recorded_identity[field]
        {
            bail!("evidence_identity_mismatch: missing or different {field}");
        }
    }
    ensure!(
        baseline.build_id.as_str().is_some_and(|id| !id.is_empty())
            && baseline.build_id == current.build_id,
        "evidence_identity_mismatch: missing or different MCP build"
    );
    ensure!(
        request.allow_different_phases || baseline.phase == current.phase,
        "evidence_identity_mismatch: different phases require allow_different_phases"
    );
    ensure!(
        baseline.series.len() == current.series.len(),
        "evidence_identity_mismatch: process roles differ"
    );
    let mut comparisons = Vec::new();
    let mut warnings = Vec::new();
    if baseline.phase != current.phase {
        warnings.push("different_phases_descriptive_only");
    }
    for (left, right) in baseline.series.iter().zip(&current.series) {
        ensure!(
            left.identity.role == right.identity.role
                && left.operating_system == right.operating_system
                && left.sampling_interval_ms == right.sampling_interval_ms,
            "evidence_identity_mismatch: process role, OS or sampling interval differs"
        );
        ensure!(
            !left.metrics.is_empty() && left.metrics.len() == right.metrics.len(),
            "evidence_identity_mismatch: missing or different metrics"
        );
        for (a, b) in left.metrics.iter().zip(&right.metrics) {
            ensure!(
                a.metric_kind == b.metric_kind,
                "evidence_identity_mismatch: metrics differ"
            );
            if a.observed_span_ms != b.observed_span_ms
                && !warnings.contains(&"unequal_observed_spans")
            {
                warnings.push("unequal_observed_spans");
            }
            comparisons.push(json!({
                "role": left.identity.role, "operating_system": left.operating_system, "metric_kind": a.metric_kind,
                "peak_delta_bytes": b.peak_bytes as i64 - a.peak_bytes as i64,
                "last_delta_bytes": b.last_bytes as i64 - a.last_bytes as i64,
                "net_change_delta_bytes": (b.net_change_bytes as i128 - a.net_change_bytes as i128).to_string(),
                "net_rate_delta_bytes_per_second": a.net_bytes_per_second.zip(b.net_bytes_per_second).map(|(a,b)| b-a),
            }));
        }
    }
    Ok(
        json!({"schema": 1, "comparison_basis": "matching_recorded_identity_not_independently_verified", "baseline": baseline, "current": current, "comparisons": comparisons, "warnings": warnings}),
    )
}
