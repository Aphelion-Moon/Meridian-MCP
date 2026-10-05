use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    ByondProcProfileJson,
    ByondSendmapsJson,
    PerformanceCsv,
    RuntimeJsonl,
    EventJsonl,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactOptions {
    #[serde(default)]
    #[schemars(length(max = crate::limits::MAX_EVIDENCE_SELECTED_METRICS))]
    #[schemars(extend("uniqueItems" = true))]
    pub selected_metrics: Vec<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub wall_time_field: Option<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub world_time_field: Option<String>,
    #[serde(default)]
    #[schemars(length(max = crate::limits::MAX_EVIDENCE_GROUP_FIELDS), inner(length(max = crate::limits::MAX_EVIDENCE_GROUP_FIELD_BYTES)), extend("uniqueItems" = true, "x-itemMaxUtf8Bytes" = crate::limits::MAX_EVIDENCE_GROUP_FIELD_BYTES))]
    pub group_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "ArtifactOptions", skip_serializing_if = "Option::is_none")]
    pub options: Option<ArtifactOptions>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseInput {
    /// Unique phase identifier, limited to 256 UTF-8 bytes.
    #[schemars(length(min = 1, max = crate::limits::MAX_EVIDENCE_PHASE_ID_BYTES), extend("x-maxUtf8Bytes" = crate::limits::MAX_EVIDENCE_PHASE_ID_BYTES))]
    pub id: String,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub wall_start: Option<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub wall_end: Option<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "i64", skip_serializing_if = "Option::is_none")]
    pub world_start_ds: Option<i64>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "i64", skip_serializing_if = "Option::is_none")]
    pub world_end_ds: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkloadIdentityInput {
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub configuration_profile: Option<String>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "String", skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeEvidenceRequest {
    #[schemars(length(min = 1, max = crate::limits::MAX_EVIDENCE_ARTIFACTS))]
    pub artifacts: Vec<ArtifactDescriptor>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(with = "PathBuf", skip_serializing_if = "Option::is_none")]
    pub dmb_path: Option<PathBuf>,
    #[serde(default, deserialize_with = "crate::parameters::optional")]
    #[schemars(
        with = "WorkloadIdentityInput",
        skip_serializing_if = "Option::is_none"
    )]
    pub workload: Option<WorkloadIdentityInput>,
    #[serde(default)]
    #[schemars(length(max = crate::limits::MAX_EVIDENCE_PHASES))]
    pub phases: Vec<PhaseInput>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSemantics {
    CumulativeSnapshot,
    IntervalSeries,
    EventStream,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct ArtifactIdentity {
    pub relative_path: String,
    pub kind: ArtifactKind,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct EvidenceRecord {
    pub wall_unix_ms: Option<i128>,
    pub world_deciseconds: Option<i64>,
    pub sample_index: u64,
    pub metrics: BTreeMap<String, f64>,
    pub groups: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ParsedArtifact {
    pub identity: ArtifactIdentity,
    pub semantics: EvidenceSemantics,
    pub records: Vec<EvidenceRecord>,
    pub accepted_records: usize,
    pub rejected_records: usize,
    pub unavailable_metrics: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct EvidenceWarning {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, schemars::JsonSchema)]
pub struct RedactionSummary {
    pub values_redacted: u64,
    pub protected_fields: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct DatasetSummary {
    pub artifact: usize,
    pub semantics: EvidenceSemantics,
    pub classification: String,
    pub assigned_phase: Option<String>,
    pub raw_records: usize,
    pub accepted_records: usize,
    pub rejected_records: usize,
    pub assigned_records: usize,
    pub unassigned_records: usize,
    pub metrics: BTreeMap<String, crate::native_evidence::statistics::NumericSummary>,
    pub groups: Vec<GroupSummary>,
    pub unavailable_metrics: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct GroupSummary {
    pub key: String,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct NativeRunIdentity {
    pub identity_verification: String,
    pub build_record_id: Option<String>,
    pub dmb_sha256: Option<String>,
    pub workload: Option<WorkloadIdentityInput>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct NormalizedRun {
    pub schema: u32,
    pub identity: NativeRunIdentity,
    pub artifacts: Vec<ArtifactIdentity>,
    pub phases: Vec<PhaseInput>,
    pub datasets: Vec<DatasetSummary>,
    pub redaction: RedactionSummary,
    pub warnings: Vec<EvidenceWarning>,
}
