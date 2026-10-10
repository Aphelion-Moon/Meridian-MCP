use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct DistributionSummary {
    pub sample_count: u64,
    pub minimum: f64,
    pub maximum: f64,
    pub mean: f64,
    pub median: f64,
    pub sample_stddev: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct NoiseEnvelope {
    pub frame_cv: f64,
    pub frame_range_ns: u64,
    pub cv_limit: f64,
    pub range_ratio_limit: f64,
    pub absolute_range_floor_ns: u64,
    pub noisy: bool,
    pub reasons: Vec<String>,
}

pub fn summarize_controls(values: &[u64]) -> Option<(DistributionSummary, NoiseEnvelope)> {
    if values.len() < 3 {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let count = sorted.len() as f64;
    let mean = sorted.iter().map(|value| *value as f64).sum::<f64>() / count;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[sorted.len() / 2 - 1] as f64 + sorted[sorted.len() / 2] as f64) / 2.0
    } else {
        sorted[sorted.len() / 2] as f64
    };
    let variance = sorted
        .iter()
        .map(|value| (*value as f64 - mean).powi(2))
        .sum::<f64>()
        / (count - 1.0);
    let sample_stddev = variance.sqrt();
    let cv = if mean == 0.0 {
        f64::INFINITY
    } else {
        sample_stddev / mean
    };
    let range = sorted.last().unwrap() - sorted.first().unwrap();
    let range_limit = (1_000_000_f64).max(0.20 * median);
    let mut reasons = Vec::new();
    if cv > 0.10 {
        reasons.push("coefficient_of_variation_exceeds_0_10".into());
    }
    if range as f64 > range_limit {
        reasons.push("absolute_range_exceeds_fixed_limit".into());
    }
    Some((
        DistributionSummary {
            sample_count: sorted.len() as u64,
            minimum: sorted[0] as f64,
            maximum: *sorted.last().unwrap() as f64,
            mean,
            median,
            sample_stddev,
        },
        NoiseEnvelope {
            frame_cv: cv,
            frame_range_ns: range,
            cv_limit: 0.10,
            range_ratio_limit: 0.20,
            absolute_range_floor_ns: 1_000_000,
            noisy: !reasons.is_empty(),
            reasons,
        },
    ))
}
