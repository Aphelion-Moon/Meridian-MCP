use crate::limits::ServerLimits;
use dmi::{Dir, Dirs, State};
use dmm_tools::dmi::IconFile;
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[path = "dmi_decode.rs"]
mod decode;

#[derive(Debug, thiserror::Error)]
pub enum DmiError {
    #[error("DMI I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid DMI request: {0}")]
    Invalid(String),
    #[error("DMI resource limit exceeded: {0}")]
    Limit(String),
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct DmiAssetId {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    #[serde(skip)]
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug)]
pub struct DecodedDmi {
    pub identity: DmiAssetId,
    pub icon: Arc<IconFile>,
    pub asset_generation: u64,
    pub metadata_bytes: usize,
}

#[derive(Debug)]
pub struct PreparedDmi {
    identity: DmiAssetId,
    icon: IconFile,
    decoded_bytes: usize,
    metadata_bytes: usize,
}

pub(crate) struct DmiInput {
    identity: DmiAssetId,
    bytes: Vec<u8>,
}

impl DecodedDmi {
    pub fn decoded_bytes(&self) -> usize {
        self.icon.image.data.len() * 4
    }
}

#[derive(Debug)]
struct CacheEntry {
    asset: DecodedDmi,
    decoded_bytes: usize,
    last_use: u64,
    metadata_bytes: usize,
}

#[derive(Debug, Default)]
pub struct DmiCache {
    entries: HashMap<PathBuf, CacheEntry>,
    decoded_bytes: usize,
    next_generation: u64,
    clock: u64,
    decode_count: u64,
}

impl DmiCache {
    pub fn load(&mut self, path: &Path, limits: &ServerLimits) -> Result<DecodedDmi, DmiError> {
        self.load_input(read_dmi(path, limits)?, limits, usize::MAX)
    }

    pub(crate) fn load_input(
        &mut self,
        input: DmiInput,
        limits: &ServerLimits,
        remaining_decoded_bytes: usize,
    ) -> Result<DecodedDmi, DmiError> {
        self.load_input_checked(input, limits, remaining_decoded_bytes, || {
            Ok::<_, DmiError>(())
        })
    }

    pub(crate) fn load_input_checked<E: From<DmiError>>(
        &mut self,
        input: DmiInput,
        limits: &ServerLimits,
        remaining_decoded_bytes: usize,
        checkpoint: impl Fn() -> Result<(), E>,
    ) -> Result<DecodedDmi, E> {
        checkpoint()?;
        let (_, _, decoded_bytes) = decode::dimensions(&input.bytes, limits)?;
        if decoded_bytes > remaining_decoded_bytes {
            return Err(DmiError::Limit("max_dmi_scan_decoded_bytes".into()).into());
        }
        if let Some(entry) = self.entries.get_mut(&input.identity.path) {
            if entry.asset.identity.sha256 == input.identity.sha256 {
                decode::validate_metadata(
                    &entry.asset.icon.metadata,
                    entry.asset.icon.image.width,
                    entry.asset.icon.image.height,
                    limits,
                )?;
                if entry.metadata_bytes > limits.max_dmi_metadata_bytes {
                    return Err(DmiError::Limit("max_dmi_metadata_bytes".into()).into());
                }
                checkpoint()?;
                self.clock = self.clock.saturating_add(1);
                entry.last_use = self.clock;
                return Ok(entry.asset.clone());
            }
        }
        let prepared = prepare_input(input, limits, || self.decode_count += 1)?;
        checkpoint()?;
        Ok(self.install(prepared, limits))
    }

    pub fn decode_count(&self) -> u64 {
        self.decode_count
    }

    pub fn install(&mut self, prepared: PreparedDmi, limits: &ServerLimits) -> DecodedDmi {
        let PreparedDmi {
            identity,
            icon,
            decoded_bytes,
            metadata_bytes,
        } = prepared;
        let path = identity.path.clone();
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(&path) {
            if entry.asset.identity.sha256 == identity.sha256 {
                entry.last_use = self.clock;
                return entry.asset.clone();
            }
        }
        self.next_generation = self.next_generation.saturating_add(1);
        let asset = DecodedDmi {
            identity,
            icon: Arc::new(icon),
            asset_generation: self.next_generation,
            metadata_bytes,
        };
        if let Some(old) = self.entries.insert(
            path,
            CacheEntry {
                asset: asset.clone(),
                decoded_bytes,
                last_use: self.clock,
                metadata_bytes,
            },
        ) {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(old.decoded_bytes);
        }
        self.decoded_bytes = self.decoded_bytes.saturating_add(decoded_bytes);
        while self.entries.len() > limits.max_dmi_cache_entries
            || self.decoded_bytes > limits.max_dmi_cache_bytes
        {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(path, entry)| (entry.last_use, *path))
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(old) = self.entries.remove(&victim) {
                self.decoded_bytes = self.decoded_bytes.saturating_sub(old.decoded_bytes);
            }
        }
        asset
    }
}

pub fn prepare_dmi(path: &Path, limits: &ServerLimits) -> Result<PreparedDmi, DmiError> {
    prepare_input(read_dmi(path, limits)?, limits, || {})
}

pub(crate) fn read_dmi(path: &Path, limits: &ServerLimits) -> Result<DmiInput, DmiError> {
    let path = std::fs::canonicalize(path)?;
    let bytes = read_dmi_bytes(std::fs::File::open(&path)?, limits.max_dmi_file_bytes)?;
    let sha256 = hex_sha256(&bytes);
    let modified = std::fs::metadata(&path)
        .ok()
        .and_then(|value| value.modified().ok());
    Ok(DmiInput {
        identity: DmiAssetId {
            path,
            sha256,
            size: bytes.len() as u64,
            modified,
        },
        bytes,
    })
}

fn prepare_input(
    input: DmiInput,
    limits: &ServerLimits,
    before_decode: impl FnOnce(),
) -> Result<PreparedDmi, DmiError> {
    let (icon, metadata_bytes) = decode::decode(&input.bytes, limits, before_decode)?;
    let decoded_bytes = icon.image.data.len() * 4;
    Ok(PreparedDmi {
        identity: input.identity,
        icon,
        decoded_bytes,
        metadata_bytes,
    })
}

fn read_dmi_bytes(mut reader: impl std::io::Read, limit: u64) -> Result<Vec<u8>, DmiError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let remaining = limit.saturating_add(1).saturating_sub(bytes.len() as u64);
        let count = reader.read(&mut buffer[..remaining.min(8192) as usize])?;
        if count == 0 {
            return Ok(bytes);
        }
        let next_len = bytes
            .len()
            .checked_add(count)
            .ok_or_else(|| DmiError::Limit("max_dmi_file_bytes".into()))?;
        if next_len as u64 > limit {
            return Err(DmiError::Limit("max_dmi_file_bytes".into()));
        }
        if next_len > bytes.capacity() {
            let capacity = bytes
                .capacity()
                .saturating_mul(2)
                .max(next_len)
                .min(usize::try_from(limit).unwrap_or(usize::MAX));
            bytes.reserve_exact(capacity - bytes.len());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct PixelCounts {
    pub opaque: u64,
    pub translucent: u64,
    pub transparent: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct AlphaBounds {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct DmiFrameProfile {
    pub direction: i32,
    pub frame: u32,
    pub rect: (u32, u32, u32, u32),
    pub sha256: String,
    pub pixel_counts: PixelCounts,
    pub alpha_bounds: Option<AlphaBounds>,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct DmiStateProfile {
    pub name: String,
    pub duplicate_index: u32,
    pub directions: usize,
    pub frame_count: usize,
    pub delays: Vec<f32>,
    pub movement: bool,
    pub loop_count: u32,
    pub rewind: bool,
    pub frames: Vec<DmiFrameProfile>,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct DmiProfile {
    pub identity: DmiAssetId,
    pub asset_generation: u64,
    pub sheet_width: u32,
    pub sheet_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub total_frames: usize,
    pub states: Vec<DmiStateProfile>,
    pub warnings: Vec<DmiWarning>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct DmiWarning {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IconReferenceResolution {
    Static {
        dmi_path: PathBuf,
        state: Option<String>,
    },
    Dynamic {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct IconReference {
    pub type_path: String,
    pub file: String,
    pub line: u32,
    pub resolution: IconReferenceResolution,
}

pub fn profile_dmi(asset: &DecodedDmi, limits: &ServerLimits) -> Result<DmiProfile, DmiError> {
    let icon = &asset.icon;
    let mut total = 0usize;
    let mut states = Vec::new();
    for state in &icon.metadata.states {
        let mut frames = Vec::new();
        for frame in 0..state.frames.count() as u32 {
            for dir in ordered_dirs(state.dirs) {
                total += 1;
                if total > limits.max_dmi_frames {
                    return Err(DmiError::Limit("max_dmi_frames".to_owned()));
                }
                let rect = icon.rect_of_index(state.index_of_frame(dir, frame));
                let normalized = frame_pixels(icon, rect);
                let (counts, bounds) = pixel_stats(&normalized, rect.2, rect.3);
                frames.push(DmiFrameProfile {
                    direction: dir.to_int(),
                    frame,
                    rect,
                    sha256: hash_frame(rect.2, rect.3, &normalized),
                    pixel_counts: counts,
                    alpha_bounds: bounds,
                });
            }
        }
        states.push(DmiStateProfile {
            name: state.name.clone(),
            duplicate_index: state.duplicate_index,
            directions: state.dirs.count(),
            frame_count: state.frames.count(),
            delays: (0..state.frames.count())
                .map(|index| state.frames.delay(index))
                .collect(),
            movement: state.movement,
            loop_count: state.loop_,
            rewind: state.rewind,
            frames,
        });
    }
    Ok(DmiProfile {
        identity: asset.identity.clone(),
        asset_generation: asset.asset_generation,
        sheet_width: icon.image.width,
        sheet_height: icon.image.height,
        cell_width: icon.metadata.width,
        cell_height: icon.metadata.height,
        total_frames: total,
        states,
        warnings: vec![DmiWarning {
            code: "hotspot_unsupported",
            message: "The pinned DMI parser does not expose complete hotspot semantics.",
        }],
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeometricTransform {
    Identity,
    MirrorHorizontal,
    MirrorVertical,
    Rotate90,
    Rotate180,
    Rotate270,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
    pub alpha_bounds: Option<AlphaBounds>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    Exact,
    Transformed,
    Padded,
    Palette,
    Near,
    Different,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct FrameComparison {
    pub kind: MatchKind,
    pub transform: GeometricTransform,
    pub offset: (i32, i32),
    pub similarity: f32,
    pub changed_pixels: u64,
    pub max_channel_delta: u8,
}

pub fn normalize_frame(
    width: u32,
    height: u32,
    pixels: impl IntoIterator<Item = [u8; 4]>,
) -> NormalizedFrame {
    let pixels = pixels
        .into_iter()
        .map(|mut pixel| {
            if pixel[3] == 0 {
                pixel = [0, 0, 0, 0];
            }
            pixel
        })
        .collect::<Vec<_>>();
    let (_, alpha_bounds) = pixel_stats(&pixels, width, height);
    NormalizedFrame {
        width,
        height,
        pixels,
        alpha_bounds,
    }
}

pub fn transform_direction(dir: Dir, transform: GeometricTransform) -> Dir {
    match transform {
        GeometricTransform::Identity => dir,
        GeometricTransform::MirrorHorizontal => dir.flip_ew(),
        GeometricTransform::MirrorVertical => dir.flip_ns(),
        GeometricTransform::Rotate90 => match dir {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            Dir::Northeast => Dir::Southeast,
            Dir::Southeast => Dir::Southwest,
            Dir::Southwest => Dir::Northwest,
            Dir::Northwest => Dir::Northeast,
        },
        GeometricTransform::Rotate180 => dir.flip(),
        GeometricTransform::Rotate270 => match dir {
            Dir::North => Dir::West,
            Dir::West => Dir::South,
            Dir::South => Dir::East,
            Dir::East => Dir::North,
            Dir::Northeast => Dir::Northwest,
            Dir::Northwest => Dir::Southwest,
            Dir::Southwest => Dir::Southeast,
            Dir::Southeast => Dir::Northeast,
        },
    }
}

pub fn compare_frames(
    left: &NormalizedFrame,
    right: &NormalizedFrame,
    minimum_similarity: f32,
) -> FrameComparison {
    let threshold = minimum_similarity.clamp(0.90, 1.0);
    let mut best = compare_aligned(left, right, GeometricTransform::Identity);
    for transform in [
        GeometricTransform::MirrorHorizontal,
        GeometricTransform::MirrorVertical,
        GeometricTransform::Rotate90,
        GeometricTransform::Rotate180,
        GeometricTransform::Rotate270,
    ] {
        let transformed = transform_frame(right, transform);
        let candidate = compare_aligned(left, &transformed, transform);
        if candidate.similarity > best.similarity {
            best = candidate;
        }
    }
    if best.similarity >= threshold && best.kind == MatchKind::Different {
        best.kind = MatchKind::Near;
    }
    best
}

fn compare_aligned(
    left: &NormalizedFrame,
    right: &NormalizedFrame,
    transform: GeometricTransform,
) -> FrameComparison {
    let (Some(lb), Some(rb)) = (left.alpha_bounds, right.alpha_bounds) else {
        let same = left.alpha_bounds.is_none() && right.alpha_bounds.is_none();
        return FrameComparison {
            kind: if same {
                MatchKind::Exact
            } else {
                MatchKind::Different
            },
            transform,
            offset: (0, 0),
            similarity: if same { 1.0 } else { 0.0 },
            changed_pixels: if same { 0 } else { 1 },
            max_channel_delta: if same { 0 } else { 255 },
        };
    };
    let lw = lb.max_x - lb.min_x + 1;
    let lh = lb.max_y - lb.min_y + 1;
    let rw = rb.max_x - rb.min_x + 1;
    let rh = rb.max_y - rb.min_y + 1;
    if lw != rw || lh != rh {
        return FrameComparison {
            kind: MatchKind::Different,
            transform,
            offset: (
                rb.min_x as i32 - lb.min_x as i32,
                rb.min_y as i32 - lb.min_y as i32,
            ),
            similarity: 0.0,
            changed_pixels: u64::from(lw) * u64::from(lh),
            max_channel_delta: 255,
        };
    }
    let mut delta = 0u64;
    let mut changed = 0;
    let mut max_delta = 0;
    let mut palette_left = BTreeMap::new();
    let mut palette_right = BTreeMap::new();
    let mut palette_equal = true;
    for y in 0..lh {
        for x in 0..lw {
            let a = left.pixels[((lb.min_y + y) * left.width + lb.min_x + x) as usize];
            let b = right.pixels[((rb.min_y + y) * right.width + rb.min_x + x) as usize];
            if a != b {
                changed += 1;
            }
            for channel in 0..4 {
                let d = a[channel].abs_diff(b[channel]);
                delta += u64::from(d);
                max_delta = max_delta.max(d);
            }
            let next_left = palette_left.len();
            let li = *palette_left.entry(a).or_insert(next_left);
            let next_right = palette_right.len();
            let ri = *palette_right.entry(b).or_insert(next_right);
            if li != ri || a[3] != b[3] {
                palette_equal = false;
            }
        }
    }
    let similarity = 1.0 - delta as f32 / (lw as f32 * lh as f32 * 4.0 * 255.0);
    let padded = lb.min_x != rb.min_x
        || lb.min_y != rb.min_y
        || left.width != right.width
        || left.height != right.height;
    let kind = if changed == 0 {
        if transform != GeometricTransform::Identity {
            MatchKind::Transformed
        } else if padded {
            MatchKind::Padded
        } else {
            MatchKind::Exact
        }
    } else if palette_equal {
        MatchKind::Palette
    } else {
        MatchKind::Different
    };
    FrameComparison {
        kind,
        transform,
        offset: (
            rb.min_x as i32 - lb.min_x as i32,
            rb.min_y as i32 - lb.min_y as i32,
        ),
        similarity,
        changed_pixels: changed,
        max_channel_delta: max_delta,
    }
}

fn transform_frame(frame: &NormalizedFrame, transform: GeometricTransform) -> NormalizedFrame {
    let (width, height) = match transform {
        GeometricTransform::Rotate90 | GeometricTransform::Rotate270 => (frame.height, frame.width),
        _ => (frame.width, frame.height),
    };
    let mut out = vec![[0; 4]; (width * height) as usize];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let (nx, ny) = match transform {
                GeometricTransform::Identity => (x, y),
                GeometricTransform::MirrorHorizontal => (frame.width - 1 - x, y),
                GeometricTransform::MirrorVertical => (x, frame.height - 1 - y),
                GeometricTransform::Rotate90 => (frame.height - 1 - y, x),
                GeometricTransform::Rotate180 => (frame.width - 1 - x, frame.height - 1 - y),
                GeometricTransform::Rotate270 => (y, frame.width - 1 - x),
            };
            out[(ny * width + nx) as usize] = frame.pixels[(y * frame.width + x) as usize];
        }
    }
    normalize_frame(width, height, out)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema)]
pub struct StateLocator {
    pub dmi_path: PathBuf,
    pub state: String,
    pub duplicate_index: u32,
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct StateComparison {
    pub left: StateLocator,
    pub right: StateLocator,
    pub image_match: MatchKind,
    pub metadata_differences: Vec<String>,
    pub frames: Vec<FrameComparison>,
}

pub fn compare_states(
    left: &DecodedDmi,
    left_name: &str,
    left_duplicate: u32,
    right: &DecodedDmi,
    right_name: &str,
    right_duplicate: u32,
    minimum: f32,
) -> Result<StateComparison, DmiError> {
    let ls = find_state(&left.icon, left_name, left_duplicate)?;
    let rs = find_state(&right.icon, right_name, right_duplicate)?;
    if ls.dirs.count() != rs.dirs.count() || ls.frames.count() != rs.frames.count() {
        return Ok(StateComparison {
            left: locator(left, ls),
            right: locator(right, rs),
            image_match: MatchKind::Different,
            metadata_differences: metadata_differences(ls, rs),
            frames: Vec::new(),
        });
    }
    let threshold = minimum.clamp(0.90, 1.0);
    let mut best: Option<(u8, f32, Vec<FrameComparison>)> = None;
    for transform in [
        GeometricTransform::Identity,
        GeometricTransform::MirrorHorizontal,
        GeometricTransform::MirrorVertical,
        GeometricTransform::Rotate90,
        GeometricTransform::Rotate180,
        GeometricTransform::Rotate270,
    ] {
        let mut comparisons = Vec::new();
        let right_dirs = ordered_dirs(rs.dirs);
        let mut valid = true;
        for frame in 0..ls.frames.count() as u32 {
            for left_dir in ordered_dirs(ls.dirs) {
                let Some(right_dir) = right_dirs
                    .iter()
                    .copied()
                    .find(|right_dir| transform_direction(*right_dir, transform) == left_dir)
                else {
                    valid = false;
                    break;
                };
                let left_frame = normalized_icon_frame(&left.icon, ls, left_dir, frame);
                let right_frame = normalized_icon_frame(&right.icon, rs, right_dir, frame);
                let transformed = transform_frame(&right_frame, transform);
                let mut comparison = compare_aligned(&left_frame, &transformed, transform);
                if comparison.kind == MatchKind::Different && comparison.similarity >= threshold {
                    comparison.kind = MatchKind::Near;
                }
                comparisons.push(comparison);
            }
            if !valid {
                break;
            }
        }
        if !valid {
            continue;
        }
        let rank = comparisons
            .iter()
            .map(|comparison| comparison.kind)
            .max_by_key(match_rank)
            .map(|kind| match_rank(&kind))
            .unwrap_or_else(|| match_rank(&MatchKind::Different));
        let similarity = comparisons
            .iter()
            .map(|comparison| comparison.similarity)
            .sum::<f32>()
            / comparisons.len().max(1) as f32;
        if best.as_ref().is_none_or(|(best_rank, best_similarity, _)| {
            rank < *best_rank || (rank == *best_rank && similarity > *best_similarity)
        }) {
            best = Some((rank, similarity, comparisons));
        }
    }
    let (_, _, comparisons) =
        best.unwrap_or_else(|| (match_rank(&MatchKind::Different), 0.0, Vec::new()));
    let class = comparisons
        .iter()
        .map(|value| value.kind)
        .max_by_key(match_rank)
        .unwrap_or(MatchKind::Different);
    Ok(StateComparison {
        left: locator(left, ls),
        right: locator(right, rs),
        image_match: class,
        metadata_differences: metadata_differences(ls, rs),
        frames: comparisons,
    })
}

pub fn state_candidate_signatures(asset: &DecodedDmi, state: &State) -> Vec<String> {
    let mut signatures = Vec::new();
    for transform in [
        GeometricTransform::Identity,
        GeometricTransform::MirrorHorizontal,
        GeometricTransform::MirrorVertical,
        GeometricTransform::Rotate90,
        GeometricTransform::Rotate180,
        GeometricTransform::Rotate270,
    ] {
        let source_dirs = ordered_dirs(state.dirs);
        let mut exact = Sha256::new();
        let mut cropped = Sha256::new();
        let mut palette = Sha256::new();
        let mut perceptual = 0u64;
        let mut palette_indexes = BTreeMap::<[u8; 4], u32>::new();
        let mut valid = true;
        for frame in 0..state.frames.count() as u32 {
            for canonical_dir in ordered_dirs(state.dirs) {
                let Some(source_dir) = source_dirs.iter().copied().find(|source_dir| {
                    transform_direction(*source_dir, transform) == canonical_dir
                }) else {
                    valid = false;
                    break;
                };
                let source = normalized_icon_frame(&asset.icon, state, source_dir, frame);
                let transformed = transform_frame(&source, transform);
                exact.update(transformed.width.to_le_bytes());
                exact.update(transformed.height.to_le_bytes());
                for pixel in &transformed.pixels {
                    exact.update(pixel);
                }

                if let Some(bounds) = transformed.alpha_bounds {
                    let width = bounds.max_x - bounds.min_x + 1;
                    let height = bounds.max_y - bounds.min_y + 1;
                    cropped.update(width.to_le_bytes());
                    cropped.update(height.to_le_bytes());
                    palette.update(width.to_le_bytes());
                    palette.update(height.to_le_bytes());
                    for y in bounds.min_y..=bounds.max_y {
                        for x in bounds.min_x..=bounds.max_x {
                            let pixel = transformed.pixels[(y * transformed.width + x) as usize];
                            cropped.update(pixel);
                            let next = palette_indexes.len() as u32;
                            let index = *palette_indexes.entry(pixel).or_insert(next);
                            palette.update(index.to_le_bytes());
                            palette.update([pixel[3]]);
                        }
                    }
                } else {
                    cropped.update(0u32.to_le_bytes());
                    palette.update(0u32.to_le_bytes());
                }
                perceptual = perceptual.rotate_left(7) ^ perceptual_signature(&transformed);
            }
            if !valid {
                break;
            }
        }
        if !valid {
            continue;
        }
        signatures.push(format!("exact:{:x}", exact.finalize()));
        signatures.push(format!("cropped:{:x}", cropped.finalize()));
        signatures.push(format!("palette:{:x}", palette.finalize()));
        for band in 0..4 {
            let segment = (perceptual >> (band * 16)) as u16;
            signatures.push(format!("perceptual:{band}:{segment:04x}"));
        }
    }
    signatures.sort();
    signatures.dedup();
    signatures
}

fn perceptual_signature(frame: &NormalizedFrame) -> u64 {
    let mut signature = 0u64;
    for sample_y in 0..8 {
        for sample_x in 0..8 {
            let x = (sample_x * frame.width / 8).min(frame.width.saturating_sub(1));
            let y = (sample_y * frame.height / 8).min(frame.height.saturating_sub(1));
            let pixel = frame.pixels[(y * frame.width + x) as usize];
            let luminance =
                u32::from(pixel[0]) * 54 + u32::from(pixel[1]) * 183 + u32::from(pixel[2]) * 19;
            let visible = pixel[3] != 0 && luminance != 0;
            if visible {
                signature |= 1 << (sample_y * 8 + sample_x);
            }
        }
    }
    signature
}

fn match_rank(kind: &MatchKind) -> u8 {
    match kind {
        MatchKind::Exact => 0,
        MatchKind::Transformed => 1,
        MatchKind::Padded => 2,
        MatchKind::Palette => 3,
        MatchKind::Near => 4,
        MatchKind::Different => 5,
    }
}
fn metadata_differences(a: &State, b: &State) -> Vec<String> {
    let mut out = Vec::new();
    if a.dirs != b.dirs {
        out.push("directions".into())
    }
    if a.frames != b.frames {
        out.push("frames_or_delays".into())
    }
    if a.movement != b.movement {
        out.push("movement".into())
    }
    if a.loop_ != b.loop_ {
        out.push("loop".into())
    }
    if a.rewind != b.rewind {
        out.push("rewind".into())
    }
    out
}
fn find_state<'a>(icon: &'a IconFile, name: &str, duplicate: u32) -> Result<&'a State, DmiError> {
    icon.metadata
        .states
        .iter()
        .find(|state| state.name == name && state.duplicate_index == duplicate)
        .ok_or_else(|| DmiError::Invalid(format!("state {name} duplicate {duplicate} not found")))
}
fn locator(asset: &DecodedDmi, state: &State) -> StateLocator {
    StateLocator {
        dmi_path: asset.identity.path.clone(),
        state: state.name.clone(),
        duplicate_index: state.duplicate_index,
    }
}
fn normalized_icon_frame(icon: &IconFile, state: &State, dir: Dir, frame: u32) -> NormalizedFrame {
    let rect = icon.rect_of_index(state.index_of_frame(dir, frame));
    normalize_frame(rect.2, rect.3, frame_pixels(icon, rect))
}

pub fn discover_dmis(
    root: &Path,
    include_glob: Option<&str>,
    limits: &ServerLimits,
) -> Result<(Vec<PathBuf>, Vec<String>), DmiError> {
    let policy = crate::PathPolicy::new(vec![root.to_owned()], vec![])
        .map_err(|error| DmiError::Invalid(error.to_string()))?;
    discover_dmis_with_policy(&policy, root, include_glob, limits)
}

pub fn discover_dmis_with_policy(
    policy: &crate::PathPolicy,
    root: &Path,
    include_glob: Option<&str>,
    limits: &ServerLimits,
) -> Result<(Vec<PathBuf>, Vec<String>), DmiError> {
    let matcher = glob_regex(include_glob.unwrap_or("**/*.dmi"))?;
    let root = std::fs::canonicalize(root)?;
    let mut stack = vec![root.clone()];
    let mut files = Vec::new();
    let mut bytes = 0u64;
    let mut reasons = Vec::new();
    while let Some(dir) = stack.pop() {
        let checked = policy
            .read_path(&dir)
            .map_err(|error| DmiError::Invalid(error.to_string()))?;
        for entry in std::fs::read_dir(checked)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_symlink() {
                continue;
            }
            if ty.is_dir() {
                if matches!(
                    entry.file_name().to_string_lossy().as_ref(),
                    ".git" | "target" | ".meridian-mcp-cache"
                ) {
                    continue;
                }
                stack.push(path)
            } else if ty.is_file() {
                let relative = path.strip_prefix(&root).unwrap_or(&path);
                if matcher.is_match(&relative.to_string_lossy().replace('\\', "/")) {
                    bytes = bytes.saturating_add(entry.metadata()?.len());
                    if files.len() >= limits.max_dmi_files {
                        reasons.push("max_dmi_files".into());
                        break;
                    }
                    if bytes > limits.max_dmi_input_bytes {
                        reasons.push("max_dmi_input_bytes".into());
                        break;
                    }
                    files.push(path)
                }
            }
        }
    }
    files.sort();
    Ok((files, reasons))
}

fn glob_regex(pattern: &str) -> Result<Regex, DmiError> {
    let mut expression = String::from("^");
    let chars = pattern.replace('\\', "/").chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '*' if chars.get(index + 1) == Some(&'*') && chars.get(index + 2) == Some(&'/') => {
                expression.push_str("(?:.*/)?");
                index += 2;
            }
            '*' if chars.get(index + 1) == Some(&'*') => {
                expression.push_str(".*");
                index += 1;
            }
            '*' => expression.push_str("[^/]*"),
            '?' => expression.push_str("[^/]"),
            character => expression.push_str(&regex::escape(&character.to_string())),
        }
        index += 1;
    }
    expression.push('$');
    Regex::new(&expression).map_err(|error| DmiError::Invalid(error.to_string()))
}

fn ordered_dirs(dirs: Dirs) -> Vec<Dir> {
    match dirs {
        Dirs::One => vec![Dir::South],
        Dirs::Four => vec![Dir::South, Dir::North, Dir::East, Dir::West],
        Dirs::Eight => vec![
            Dir::South,
            Dir::North,
            Dir::East,
            Dir::West,
            Dir::Southeast,
            Dir::Southwest,
            Dir::Northeast,
            Dir::Northwest,
        ],
    }
}
fn frame_pixels(icon: &IconFile, rect: (u32, u32, u32, u32)) -> Vec<[u8; 4]> {
    let data = icon.image.data.as_slice().expect("contiguous DMI pixels");
    let mut out = Vec::with_capacity((rect.2 * rect.3) as usize);
    for y in rect.1..rect.1 + rect.3 {
        for x in rect.0..rect.0 + rect.2 {
            let pixel = data[(y * icon.image.width + x) as usize];
            let mut rgba = [pixel.r, pixel.g, pixel.b, pixel.a];
            if rgba[3] == 0 {
                rgba = [0, 0, 0, 0]
            }
            out.push(rgba)
        }
    }
    out
}
fn pixel_stats(pixels: &[[u8; 4]], width: u32, height: u32) -> (PixelCounts, Option<AlphaBounds>) {
    let mut counts = PixelCounts::default();
    let mut bounds: Option<AlphaBounds> = None;
    for y in 0..height {
        for x in 0..width {
            let alpha = pixels[(y * width + x) as usize][3];
            match alpha {
                0 => counts.transparent += 1,
                255 => counts.opaque += 1,
                _ => counts.translucent += 1,
            }
            if alpha != 0 {
                bounds = Some(match bounds {
                    None => AlphaBounds {
                        min_x: x,
                        min_y: y,
                        max_x: x,
                        max_y: y,
                    },
                    Some(old) => AlphaBounds {
                        min_x: old.min_x.min(x),
                        min_y: old.min_y.min(y),
                        max_x: old.max_x.max(x),
                        max_y: old.max_y.max(y),
                    },
                })
            }
        }
    }
    (counts, bounds)
}
fn hash_frame(width: u32, height: u32, pixels: &[[u8; 4]]) -> String {
    let mut hash = Sha256::new();
    hash.update(width.to_le_bytes());
    hash.update(height.to_le_bytes());
    for pixel in pixels {
        hash.update(pixel)
    }
    format!("{:x}", hash.finalize())
}
fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_decode_does_not_install_or_advance_cache_generation() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.add_text_chunk("Description".into(), "# BEGIN DMI\nversion = 4.0\n\twidth = 1\n\theight = 1\nstate = \"late\"\n\tdirs = 1\n\tframes = 1\n# END DMI\n".into()).unwrap();
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[1, 2, 3, 255])
                .unwrap();
        }
        let input = || DmiInput {
            identity: DmiAssetId {
                path: "late.dmi".into(),
                sha256: hex_sha256(&bytes),
                size: bytes.len() as u64,
                modified: None,
            },
            bytes: bytes.clone(),
        };
        let mut cache = DmiCache::default();
        let checks = std::cell::Cell::new(0);
        let result =
            cache.load_input_checked(input(), &ServerLimits::default(), usize::MAX, || {
                checks.set(checks.get() + 1);
                anyhow::ensure!(checks.get() < 2, "request_cancelled");
                Ok::<_, anyhow::Error>(())
            });
        assert!(result.is_err());
        assert_eq!(cache.decode_count, 1);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.next_generation, 0);
        let installed = cache
            .load_input(input(), &ServerLimits::default(), usize::MAX)
            .unwrap();
        assert_eq!(installed.asset_generation, 1);
        assert_eq!(cache.decode_count, 2);
    }

    #[test]
    fn bounded_reader_never_consumes_past_detection_byte() {
        let input = vec![0_u8; 4096];
        let mut reader = std::io::Cursor::new(input);
        assert!(read_dmi_bytes(&mut reader, 16).is_err());
        assert_eq!(reader.position(), 17);
    }

    fn frame(width: u32, height: u32, pixels: &[[u8; 4]]) -> NormalizedFrame {
        normalize_frame(width, height, pixels.iter().copied())
    }

    #[test]
    fn transparent_rgb_is_ignored_and_padding_is_classified() {
        let left = frame(2, 1, &[[200, 1, 2, 0], [255, 0, 0, 255]]);
        let right = frame(3, 1, &[[4, 5, 6, 0], [7, 8, 9, 0], [255, 0, 0, 255]]);
        let comparison = compare_frames(&left, &right, 0.985);
        assert_eq!(comparison.kind, MatchKind::Padded);
        assert_eq!(comparison.changed_pixels, 0);
    }

    #[test]
    fn mirror_palette_and_near_changes_are_distinguished() {
        let left = frame(2, 1, &[[255, 0, 0, 255], [0, 255, 0, 255]]);
        let mirror = frame(2, 1, &[[0, 255, 0, 255], [255, 0, 0, 255]]);
        assert_eq!(
            compare_frames(&left, &mirror, 0.985).kind,
            MatchKind::Transformed
        );
        let palette = frame(2, 1, &[[0, 0, 255, 255], [255, 255, 0, 255]]);
        assert_eq!(
            compare_frames(&left, &palette, 0.985).kind,
            MatchKind::Palette
        );
        let near_left = frame(
            3,
            1,
            &[[255, 0, 0, 255], [0, 255, 0, 255], [255, 0, 0, 255]],
        );
        let near = frame(
            3,
            1,
            &[[250, 0, 0, 255], [0, 255, 0, 255], [255, 0, 0, 255]],
        );
        assert_eq!(
            compare_frames(&near_left, &near, 0.985).kind,
            MatchKind::Near
        );
    }

    #[test]
    fn all_direction_transforms_have_stable_inverses() {
        for direction in Dir::ALL {
            assert_eq!(
                transform_direction(
                    transform_direction(*direction, GeometricTransform::MirrorHorizontal),
                    GeometricTransform::MirrorHorizontal
                ),
                *direction
            );
            assert_eq!(
                transform_direction(
                    transform_direction(*direction, GeometricTransform::Rotate90),
                    GeometricTransform::Rotate270
                ),
                *direction
            );
        }
    }

    #[test]
    fn default_dmi_glob_matches_root_and_nested_files() {
        let matcher = glob_regex("**/*.dmi").unwrap();
        assert!(matcher.is_match("icons.dmi"));
        assert!(matcher.is_match("icons/items.dmi"));
        assert!(!matcher.is_match("icons/items.png"));
    }
}
