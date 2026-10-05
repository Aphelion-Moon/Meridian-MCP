use dmm_tools::{dmm, render_passes};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DmmError {
    #[error(transparent)]
    Parse(#[from] dreammaker::DMError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct MapBounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct ModelUseCount {
    pub model: String,
    pub count: usize,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
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
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct CoordinateDifference {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub left: Option<String>,
    pub right: Option<String>,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct MapDifference {
    pub coordinates: Vec<CoordinateDifference>,
    pub left_dimensions: [usize; 3],
    pub right_dimensions: [usize; 3],
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
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
    profile_loaded_map(path, &map, limit)
}

pub(crate) fn key_counts(map: &dmm::Map) -> BTreeMap<dmm::Key, usize> {
    let mut counts = BTreeMap::new();
    for key in &map.grid {
        *counts.entry(*key).or_default() += 1;
    }
    counts
}

pub(crate) fn profile_loaded_map(
    path: &Path,
    map: &dmm::Map,
    limit: usize,
) -> Result<MapProfile, DmmError> {
    let (x, y, z) = map.dim_xyz();
    let mut counts = BTreeMap::<String, usize>::new();
    for (key, count) in key_counts(map) {
        if let Some(model) = map.dictionary.get(&key) {
            *counts.entry(model_string(model)).or_default() += count;
        }
    }
    let unique_models = counts.len();
    let mut counts = counts
        .into_iter()
        .map(|(model, count)| ModelUseCount { model, count })
        .collect::<Vec<_>>();
    counts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.model.cmp(&b.model)));
    counts.truncate(limit);
    Ok(MapProfile {
        path: path.to_owned(),
        format: map_format(path)?.into(),
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
    // Intern parsed models once per dictionary key. Prefab equality ignores
    // variable insertion order while retaining the order of atoms in a tile.
    let mut models = HashMap::new();
    let left_ids = model_ids(&left, &mut models);
    let right_ids = model_ids(&right, &mut models);
    let mut left_cells = cells(&left).peekable();
    let mut right_cells = cells(&right).peekable();
    let mut differences = Vec::new();
    let mut truncated = false;
    loop {
        let coordinate = match (left_cells.peek(), right_cells.peek()) {
            (Some((left, _)), Some((right, _))) => *left.min(right),
            (Some((coordinate, _)), None) | (None, Some((coordinate, _))) => *coordinate,
            (None, None) => break,
        };
        let left_key = left_cells
            .next_if(|(at, _)| *at == coordinate)
            .map(|(_, key)| key);
        let right_key = right_cells
            .next_if(|(at, _)| *at == coordinate)
            .map(|(_, key)| key);
        // Zero retains the old missing-dictionary marker, distinct from a cell
        // outside this map. Only returned differences need rendered strings.
        let l = left_key.map(|key| left_ids.get(&key).copied().unwrap_or(0));
        let r = right_key.map(|key| right_ids.get(&key).copied().unwrap_or(0));
        if l != r {
            if differences.len() >= limit {
                truncated = true;
                break;
            }
            differences.push(CoordinateDifference {
                x: coordinate.x,
                y: coordinate.y,
                z: coordinate.z,
                left: cell_text(&left, left_key),
                right: cell_text(&right, right_key),
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

fn cells(map: &dmm::Map) -> impl Iterator<Item = (dmm::Coord3, dmm::Key)> + '_ {
    let (x, y, z) = map.dim_xyz();
    // Merge each map's coordinates in the established (x,y,z) ordering. A
    // combined bounding box could dwarf both maps when their shapes differ.
    (1..=x as i32).flat_map(move |x| {
        (1..=y as i32).flat_map(move |y| {
            (1..=z as i32).map(move |z| {
                let coordinate = dmm::Coord3::new(x, y, z);
                (coordinate, map[coordinate])
            })
        })
    })
}

fn model_ids<'a>(
    map: &'a dmm::Map,
    models: &mut HashMap<&'a [dmm::Prefab], usize>,
) -> BTreeMap<dmm::Key, usize> {
    map.dictionary
        .iter()
        .map(|(key, model)| {
            let next = models.len() + 1;
            (*key, *models.entry(model.as_slice()).or_insert(next))
        })
        .collect()
}

fn cell_text(map: &dmm::Map, key: Option<dmm::Key>) -> Option<String> {
    key.map(|key| {
        map.dictionary
            .get(&key)
            .map(|model| model_string(model))
            .unwrap_or_else(|| "<missing dictionary key>".into())
    })
}

fn map_format(path: &Path) -> Result<&'static str, std::io::Error> {
    const MARKER: &[u8] = b"//MAP CONVERTED BY dmm2tgm.py";
    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0_u8; 8192];
    let mut retained = 0;
    loop {
        let read = file.read(&mut buffer[retained..])?;
        if read == 0 {
            return Ok("DMM");
        }
        let end = retained + read;
        if buffer[..end]
            .windows(MARKER.len())
            .any(|window| window == MARKER)
        {
            return Ok("TGM");
        }
        retained = end.min(MARKER.len() - 1);
        buffer.copy_within(end - retained..end, 0);
    }
}
fn model_string(model: &[dmm::Prefab]) -> String {
    model
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
