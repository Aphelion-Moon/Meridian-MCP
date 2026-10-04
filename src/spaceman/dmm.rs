use dmm_tools::{dmm, render_passes};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DmmError {
    #[error(transparent)]
    Parse(#[from] dreammaker::DMError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Serialize)]
pub struct MapBounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}
#[derive(Clone, Debug, Serialize)]
pub struct ModelUseCount {
    pub model: String,
    pub count: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct MapProfile {
    pub path: PathBuf,
    pub format: String,
    pub dimensions: [usize; 3],
    pub bounds: MapBounds,
    pub dictionary_entries: usize,
    pub unique_models: usize,
    pub model_use_counts: Vec<ModelUseCount>,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CoordinateDifference {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub left: Option<String>,
    pub right: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MapDifference {
    pub coordinates: Vec<CoordinateDifference>,
    pub left_dimensions: [usize; 3],
    pub right_dimensions: [usize; 3],
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct RenderPassRecord {
    pub name: String,
    pub description: String,
    pub default_enabled: bool,
}

pub fn load_map(path: &Path) -> Result<dmm::Map, dreammaker::DMError> {
    dmm::Map::from_file_with_cell_limit(path, crate::limits::ServerLimits::default().max_map_cells)
}

pub fn profile_map(path: &Path, limit: usize) -> Result<MapProfile, DmmError> {
    let map = load_map(path)?;
    let (x, y, z) = map.dim_xyz();
    let mut counts = BTreeMap::<String, usize>::new();
    for key in map.grid.iter() {
        if let Some(model) = map.dictionary.get(key) {
            *counts.entry(model_string(model)).or_default() += 1
        }
    }
    let unique_models = counts.len();
    let mut counts = counts
        .into_iter()
        .map(|(model, count)| ModelUseCount { model, count })
        .collect::<Vec<_>>();
    counts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.model.cmp(&b.model)));
    counts.truncate(limit);
    let content = std::fs::read_to_string(path)?;
    Ok(MapProfile {
        path: path.to_owned(),
        format: if content.contains("//MAP CONVERTED BY dmm2tgm.py") {
            "TGM".into()
        } else {
            "DMM".into()
        },
        dimensions: [x, y, z],
        bounds: MapBounds {
            min: [1, 1, 1],
            max: [x as i32, y as i32, z as i32],
        },
        dictionary_entries: map.dictionary.len(),
        unique_models,
        model_use_counts: counts,
        warnings: Vec::new(),
    })
}

pub fn diff_maps(left: &Path, right: &Path, limit: usize) -> Result<MapDifference, DmmError> {
    let left = load_map(left)?;
    let right = load_map(right)?;
    let ld = left.dim_xyz();
    let rd = right.dim_xyz();
    let left_cells = cells(&left);
    let right_cells = cells(&right);
    let coordinates = left_cells
        .keys()
        .chain(right_cells.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut differences = Vec::new();
    let mut truncated = false;
    for coordinate in coordinates {
        let l = left_cells.get(&coordinate);
        let r = right_cells.get(&coordinate);
        if l != r {
            if differences.len() >= limit {
                truncated = true;
                break;
            }
            differences.push(CoordinateDifference {
                x: coordinate.0,
                y: coordinate.1,
                z: coordinate.2,
                left: l.cloned(),
                right: r.cloned(),
            })
        }
    }
    Ok(MapDifference {
        coordinates: differences,
        left_dimensions: [ld.0, ld.1, ld.2],
        right_dimensions: [rd.0, rd.1, rd.2],
        truncated,
    })
}

pub fn render_pass_inventory() -> Vec<RenderPassRecord> {
    render_passes::RENDER_PASSES
        .iter()
        .map(|pass| RenderPassRecord {
            name: pass.name.to_owned(),
            description: pass.desc.to_owned(),
            default_enabled: pass.default,
        })
        .collect()
}

fn cells(map: &dmm::Map) -> BTreeMap<(i32, i32, i32), String> {
    let mut cells = BTreeMap::new();
    for (z, level) in map.iter_levels() {
        for (coordinate, key) in level.iter_top_down() {
            let model = map
                .dictionary
                .get(&key)
                .map(|value| model_string(value))
                .unwrap_or_else(|| "<missing dictionary key>".into());
            cells.insert((coordinate.x, coordinate.y, z), model);
        }
    }
    cells
}
fn model_string(model: &[dmm::Prefab]) -> String {
    model
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
