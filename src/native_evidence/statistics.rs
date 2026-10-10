use serde::Serialize;

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct NumericSummary {
    pub count: usize,
    pub missing_count: usize,
    pub minimum: f64,
    pub maximum: f64,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub sample_standard_deviation: Option<f64>,
}

impl NumericSummary {
    pub fn from_samples(samples: &[f64]) -> Option<Self> {
        Self::from_samples_with_total(samples, samples.len())
    }

    pub fn from_samples_with_total(samples: &[f64], total_count: usize) -> Option<Self> {
        if samples.is_empty() || samples.iter().any(|value| !value.is_finite()) {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let mean = compensated_sum(&sorted) / sorted.len() as f64;
        let sample_standard_deviation = (sorted.len() > 1).then(|| {
            let squared = sorted
                .iter()
                .map(|value| (value - mean).powi(2))
                .collect::<Vec<_>>();
            (compensated_sum(&squared) / (sorted.len() - 1) as f64).sqrt()
        });
        Some(Self {
            count: sorted.len(),
            missing_count: total_count.saturating_sub(sorted.len()),
            minimum: sorted[0],
            maximum: *sorted.last().unwrap(),
            mean,
            p50: percentile_type7(&sorted, 0.50).unwrap(),
            p95: percentile_type7(&sorted, 0.95).unwrap(),
            p99: percentile_type7(&sorted, 0.99).unwrap(),
            sample_standard_deviation,
        })
    }
}

pub fn percentile_type7(sorted: &[f64], probability: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let h = (sorted.len() - 1) as f64 * probability;
    let lower = h.floor() as usize;
    let upper = h.ceil() as usize;
    Some(sorted[lower] + (h - lower as f64) * (sorted[upper] - sorted[lower]))
}

pub fn coefficient_of_variation(summary: &NumericSummary, sample_count: usize) -> Option<f64> {
    (sample_count >= 3 && summary.mean != 0.0)
        .then_some(summary.sample_standard_deviation? / summary.mean.abs())
}

fn compensated_sum(values: &[f64]) -> f64 {
    let mut sum = 0.0;
    let mut correction = 0.0;
    for value in values {
        let adjusted = value - correction;
        let next = sum + adjusted;
        correction = (next - sum) - adjusted;
        sum = next;
    }
    sum
}
