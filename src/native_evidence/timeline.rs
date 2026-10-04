use crate::native_evidence::model::{EvidenceRecord, PhaseInput};
use anyhow::{bail, Result};

pub fn validate_phases(phases: &[PhaseInput]) -> Result<()> {
    for phase in phases {
        if phase.id.is_empty() || phase.id.len() > crate::limits::MAX_EVIDENCE_PHASE_ID_BYTES {
            bail!("phase identifiers must contain 1-256 bytes");
        }
        if let (Some(start), Some(end)) = (
            wall_ms(phase.wall_start.as_deref())?,
            wall_ms(phase.wall_end.as_deref())?,
        ) {
            if start >= end {
                bail!("phase wall ranges must be half-open and increasing");
            }
        }
        if let (Some(start), Some(end)) = (phase.world_start_ds, phase.world_end_ds) {
            if start >= end {
                bail!("phase world ranges must be half-open and increasing");
            }
        }
    }
    for (index, left) in phases.iter().enumerate() {
        for right in &phases[index + 1..] {
            if overlaps(
                left.world_start_ds,
                left.world_end_ds,
                right.world_start_ds,
                right.world_end_ds,
            ) || overlaps(
                wall_ms(left.wall_start.as_deref())?,
                wall_ms(left.wall_end.as_deref())?,
                wall_ms(right.wall_start.as_deref())?,
                wall_ms(right.wall_end.as_deref())?,
            ) {
                bail!("phase ranges overlap");
            }
        }
    }
    Ok(())
}

pub fn assign(record: &EvidenceRecord, phases: &[PhaseInput]) -> Result<Option<String>> {
    let wall = phases
        .iter()
        .filter(|phase| {
            contains(
                wall_ms(phase.wall_start.as_deref()).ok().flatten(),
                wall_ms(phase.wall_end.as_deref()).ok().flatten(),
                record.wall_unix_ms,
            )
        })
        .map(|phase| phase.id.as_str())
        .collect::<Vec<_>>();
    let world = phases
        .iter()
        .filter(|phase| {
            contains(
                phase.world_start_ds,
                phase.world_end_ds,
                record.world_deciseconds,
            )
        })
        .map(|phase| phase.id.as_str())
        .collect::<Vec<_>>();
    if wall.len() > 1 || world.len() > 1 {
        bail!("record matches multiple phases");
    }
    match (wall.first(), world.first()) {
        (Some(left), Some(right)) if left != right => {
            bail!("wall and world clocks assign different phases")
        }
        (Some(value), _) | (_, Some(value)) => Ok(Some((*value).to_owned())),
        _ => Ok(None),
    }
}

pub fn wall_ms(value: Option<&str>) -> Result<Option<i128>> {
    value
        .map(|value| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .map(|time| time.unix_timestamp_nanos() / 1_000_000)
                .map_err(anyhow::Error::from)
        })
        .transpose()
}
fn contains<T: Ord + Copy>(start: Option<T>, end: Option<T>, value: Option<T>) -> bool {
    matches!((start, end, value), (Some(start), Some(end), Some(value)) if start <= value && value < end)
}
fn overlaps<T: Ord + Copy>(
    a_start: Option<T>,
    a_end: Option<T>,
    b_start: Option<T>,
    b_end: Option<T>,
) -> bool {
    matches!((a_start, a_end, b_start, b_end), (Some(a), Some(b), Some(c), Some(d)) if a < d && c < b)
}
