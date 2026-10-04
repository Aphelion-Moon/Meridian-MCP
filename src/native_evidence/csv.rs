use super::model::{ArtifactDescriptor, EvidenceRecord, EvidenceSemantics, ParsedArtifact};
use super::reader::ReadArtifact;
use anyhow::{bail, Result};

pub fn parse(
    read: ReadArtifact,
    descriptor: &ArtifactDescriptor,
    redacted: &mut u64,
) -> Result<ParsedArtifact> {
    let options = descriptor.options.as_ref();
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .trim(csv::Trim::All)
        .from_reader(read.bytes.as_slice());
    let headers = reader.headers()?.clone();
    if headers.len() > crate::limits::MAX_EVIDENCE_COLUMNS {
        bail!("evidence column limit exceeded");
    }
    let unique = headers.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != headers.len() {
        bail!("CSV headers must be unique");
    }
    let mut records = Vec::new();
    let mut rejected = 0;
    for (index, row) in reader.records().enumerate() {
        if index >= crate::limits::MAX_EVIDENCE_ROWS {
            bail!("evidence row limit exceeded");
        }
        let row = match row {
            Ok(row) => row,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        let mut object = serde_json::Map::new();
        for (header, cell) in headers.iter().zip(row.iter()) {
            if crate::native_evidence::redaction::protected(header) {
                *redacted += 1;
                continue;
            }
            if let Ok(number) = cell.parse::<i64>() {
                object.insert(header.to_owned(), serde_json::json!(number));
            } else if let Ok(number) = cell.parse::<f64>() {
                object.insert(header.to_owned(), serde_json::json!(number));
            } else {
                object.insert(header.to_owned(), serde_json::json!(cell));
            }
        }
        let generic = super::reader::scalar_record(
            &serde_json::Value::Object(object),
            index as u64,
            options,
            redacted,
        );
        records.push(EvidenceRecord {
            metrics: generic.metrics,
            groups: generic.groups,
            wall_unix_ms: generic.wall_unix_ms,
            world_deciseconds: generic.world_deciseconds,
            sample_index: index as u64,
        });
    }
    Ok(ParsedArtifact {
        identity: read.identity,
        semantics: EvidenceSemantics::IntervalSeries,
        accepted_records: records.len(),
        rejected_records: rejected,
        records,
        unavailable_metrics: Vec::new(),
    })
}
