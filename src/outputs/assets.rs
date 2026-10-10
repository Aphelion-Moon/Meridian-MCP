use crate::atomic_output::OutputArtifact;
use schemars::JsonSchema;
use serde::Serialize;
#[derive(Serialize, JsonSchema)]
pub struct PixelDimensions {
    pub width: usize,
    pub height: usize,
}
#[derive(Serialize, JsonSchema)]
pub struct RenderBounds {
    pub min: [usize; 3],
    pub max: [usize; 3],
}
#[derive(Serialize, JsonSchema)]
pub struct RenderMapData {
    pub analysis: crate::identity::AnalysisIdentity,
    pub success: bool,
    pub dmm_path: String,
    pub z_level: usize,
    pub output: OutputArtifact,
    pub output_path: String,
    pub dimensions_pixels: PixelDimensions,
    pub bounds: RenderBounds,
    pub applied_passes: Vec<String>,
    pub non_transparent_pixels: usize,
    pub warning: Option<&'static str>,
}
#[derive(Serialize, JsonSchema)]
pub struct RenderBatchItem {
    pub request_index: usize,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<RenderMapData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub outcome: crate::result::OutputMutation,
}
#[derive(Serialize, JsonSchema)]
pub struct RenderMapsData {
    pub completed: usize,
    pub failed: usize,
    pub files: Vec<RenderBatchItem>,
}
#[derive(Serialize, JsonSchema)]
pub struct MapDimensions {
    pub x: usize,
    pub y: usize,
    pub z: usize,
}
#[derive(Serialize, JsonSchema)]
pub struct MapInfoData {
    pub file: String,
    pub format: String,
    pub dimensions: MapDimensions,
    pub unique_tiles: usize,
    pub file_size_bytes: u64,
    pub top_types: Vec<(String, usize)>,
    pub top_areas: Vec<(String, usize)>,
    pub bounds: crate::spaceman::dmm::MapBounds,
    pub dictionary_entries: usize,
    pub unique_models: usize,
    pub model_use_counts: Vec<crate::spaceman::dmm::ModelUseCount>,
    pub warnings: Vec<String>,
    pub spacemandmm_revision: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct DiffMapsData {
    pub difference: crate::spaceman::dmm::MapDifference,
}
#[derive(Serialize, JsonSchema)]
pub struct RenderPassesData {
    pub passes: Vec<crate::spaceman::dmm::RenderPassRecord>,
}
#[derive(Serialize, JsonSchema)]
pub struct MapCoordinate {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub tile_key: String,
    pub matched_type: String,
}
#[derive(Serialize, JsonSchema)]
pub struct FindOnMapData {
    pub type_path: String,
    pub dmm_path: String,
    pub count: usize,
    pub matching_tile_keys: usize,
    pub keys: Vec<String>,
    pub coordinates: Vec<MapCoordinate>,
    pub truncated: bool,
}
pub type RenderMapOutput = RenderMapData;
pub type RenderMapsOutput = crate::result::Success<RenderMapsData>;
pub type MapInfoOutput = MapInfoData;
pub type DiffMapsOutput = crate::result::Success<DiffMapsData>;
pub type ListRenderPassesOutput = crate::result::Success<RenderPassesData>;
pub type FindOnMapOutput = FindOnMapData;
#[derive(Serialize, JsonSchema)]
pub struct DmiInfoData {
    pub profile: crate::spaceman::dmi::DmiProfile,
}
#[derive(Serialize, JsonSchema)]
pub struct CompareDmiStatesData {
    pub comparison: crate::spaceman::dmi::StateComparison,
}
#[derive(Serialize, JsonSchema)]
pub struct ExtractDmiData {
    pub source_path: std::path::PathBuf,
    pub source_sha256: String,
    pub output: OutputArtifact,
    pub encoder: String,
    pub kind: String,
    pub dimensions: [u32; 2],
    pub state: String,
    pub duplicate_index: u32,
}
pub type DmiInfoOutput = crate::result::Success<DmiInfoData>;
pub type CompareDmiStatesOutput = crate::result::Success<CompareDmiStatesData>;
pub type ExtractDmiOutput = crate::result::Success<ExtractDmiData>;

#[derive(Serialize, JsonSchema)]
pub struct DuplicateCluster {
    pub cluster_id: String,
    pub confidence: &'static str,
    pub members: Vec<crate::spaceman::dmi::StateLocator>,
    pub pair_evidence: Vec<crate::spaceman::dmi::StateComparison>,
}
#[derive(Serialize, JsonSchema)]
pub struct FindDmiDuplicatesData {
    pub cluster_count: usize,
    pub candidate_comparisons: usize,
    pub clusters: Vec<DuplicateCluster>,
}
#[derive(Serialize, JsonSchema)]
pub struct MissingIconFile {
    pub type_path: String,
    pub file: String,
    pub line: u32,
    pub dmi_path: std::path::PathBuf,
}
#[derive(Serialize, JsonSchema)]
pub struct MissingIconState {
    pub type_path: String,
    pub file: String,
    pub line: u32,
    pub dmi_path: std::path::PathBuf,
    pub state: String,
}
#[derive(Serialize, JsonSchema)]
pub struct UnusedIconState {
    pub dmi_path: std::path::PathBuf,
    pub state: String,
    pub duplicate_index: u32,
    pub best_effort: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct AuditIconsData {
    pub complete: bool,
    pub missing_files: Vec<MissingIconFile>,
    pub missing_states: Vec<MissingIconState>,
    pub duplicates: Vec<DuplicateCluster>,
    pub unused_states: Vec<UnusedIconState>,
    pub dynamic_references: Vec<crate::spaceman::dmi::IconReference>,
    pub candidate_comparisons: usize,
    pub unused_evidence: &'static str,
}
pub type FindDmiDuplicatesOutput = crate::result::Success<FindDmiDuplicatesData>;
pub type AuditIconsOutput = crate::result::Success<AuditIconsData>;
