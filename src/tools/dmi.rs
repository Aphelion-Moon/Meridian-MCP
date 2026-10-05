use anyhow::{anyhow, Result};
use dmi::{Dir, Dirs};
use dmm_tools::dmi::render::{IconRenderer, RenderType};
use dmm_tools::dmi::Image;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::atomic_output::{write_atomic, AtomicOutputError};
use crate::limits::ServerLimits;
use crate::mcp::ToolResult;
use crate::outputs::*;
use crate::result::{json_success, ToolMetadata};
use crate::spaceman::dmi::{
    compare_states, discover_dmis_with_policy, profile_dmi, read_dmi, state_candidate_signatures,
    DecodedDmi, DmiError, IconReferenceResolution, MatchKind, StateLocator,
};
use crate::state::ServerState;
use crate::tools::ToolExecutionContext;

async fn load(
    context: &ToolExecutionContext,
    state: &ServerState,
    path: &Path,
) -> Result<DecodedDmi> {
    load_with_budget(
        context,
        state,
        path,
        usize::MAX,
        state.asset_limits().clone(),
    )
    .await
}

async fn load_with_budget(
    context: &ToolExecutionContext,
    state: &ServerState,
    path: &Path,
    remaining_decoded_bytes: usize,
    limits: ServerLimits,
) -> Result<DecodedDmi> {
    let path = path.to_owned();
    let policy = context.policy().clone();
    let cache = state.asset_cache();
    state
        .run_asset_job(move || {
            let checked = policy.read_path(&path)?;
            let input = read_dmi(&checked, &limits)?;
            Ok(cache
                .blocking_lock()
                .load_input(input, &limits, remaining_decoded_bytes)?)
        })
        .await
}

#[derive(Default)]
struct AssetScan {
    assets: BTreeMap<PathBuf, DecodedDmi>,
    decoded_bytes: usize,
    metadata_bytes: usize,
    states: usize,
    frames: usize,
}

impl AssetScan {
    async fn load(
        &mut self,
        context: &ToolExecutionContext,
        state: &ServerState,
        path: &Path,
        reasons: &mut Vec<String>,
    ) -> Result<Option<DecodedDmi>> {
        let path = context.policy().read_path(path)?;
        if let Some(asset) = self.assets.get(&path) {
            return Ok(Some(asset.clone()));
        }
        let remaining = state
            .asset_limits()
            .max_dmi_scan_decoded_bytes
            .saturating_sub(self.decoded_bytes);
        let mut limits = state.asset_limits().clone();
        limits.max_dmi_metadata_bytes = limits.max_dmi_metadata_bytes.min(
            limits
                .max_dmi_scan_metadata_bytes
                .saturating_sub(self.metadata_bytes),
        );
        limits.max_dmi_states = limits
            .max_dmi_states
            .min(limits.max_dmi_scan_states.saturating_sub(self.states));
        limits.max_dmi_frames = limits
            .max_dmi_frames
            .min(limits.max_dmi_scan_frames.saturating_sub(self.frames));
        let budget_limits = limits.clone();
        match load_with_budget(context, state, &path, remaining, limits).await {
            Ok(asset) => {
                self.decoded_bytes += asset.decoded_bytes();
                self.metadata_bytes += asset.metadata_bytes;
                self.states += asset.icon.metadata.states.len();
                self.frames += asset
                    .icon
                    .metadata
                    .states
                    .iter()
                    .map(|state| state.dirs.count() * state.frames.count())
                    .sum::<usize>();
                self.assets.insert(path, asset.clone());
                Ok(Some(asset))
            }
            Err(error) => {
                let reason = match error.downcast_ref::<DmiError>() {
                    Some(DmiError::Limit(reason)) => match reason.as_str() {
                        "max_dmi_metadata_bytes"
                            if budget_limits.max_dmi_metadata_bytes
                                < state.asset_limits().max_dmi_metadata_bytes =>
                        {
                            "max_dmi_scan_metadata_bytes".into()
                        }
                        "max_dmi_states"
                            if budget_limits.max_dmi_states
                                < state.asset_limits().max_dmi_states =>
                        {
                            "max_dmi_scan_states".into()
                        }
                        "max_dmi_frames"
                            if budget_limits.max_dmi_frames
                                < state.asset_limits().max_dmi_frames =>
                        {
                            "max_dmi_scan_frames".into()
                        }
                        _ => reason.clone(),
                    },
                    Some(_) => "dmi_load_failed".into(),
                    None => return Err(error),
                };
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
                Ok(None)
            }
        }
    }
}

pub async fn info(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::DmiInfoParams,
) -> Result<ToolResult> {
    let path = PathBuf::from(&args.dmi_path);
    let asset = load(context, state, &path).await?;
    let profile = profile_dmi(&asset, &ServerLimits::default())?;
    let mut metadata = ToolMetadata::complete(None);
    metadata.asset_generation = Some(asset.asset_generation);
    Ok(json_success(metadata, DmiInfoData { profile }))
}

pub async fn compare(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::CompareDmiStatesParams,
) -> Result<ToolResult> {
    let left = load(context, state, &PathBuf::from(&args.left_dmi_path)).await?;
    let right = load(context, state, &PathBuf::from(&args.right_dmi_path)).await?;
    let comparison = compare_states(
        &left,
        args.left_state.as_str(),
        args.left_duplicate_index.unwrap_or(0) as u32,
        &right,
        args.right_state.as_str(),
        args.right_duplicate_index.unwrap_or(0) as u32,
        args.minimum_similarity.unwrap_or(0.985) as f32,
    )?;
    let mut metadata = ToolMetadata::complete(None);
    metadata.asset_generation = Some(left.asset_generation.max(right.asset_generation));
    Ok(json_success(metadata, CompareDmiStatesData { comparison }))
}

fn cluster_comparisons(
    comparisons: Vec<crate::spaceman::dmi::StateComparison>,
) -> Vec<DuplicateCluster> {
    let mut members = comparisons
        .iter()
        .flat_map(|comparison| [comparison.left.clone(), comparison.right.clone()])
        .collect::<Vec<_>>();
    members.sort();
    members.dedup();
    let indexes = members
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, locator)| (locator, index))
        .collect::<BTreeMap<_, _>>();
    let mut parents = (0..members.len()).collect::<Vec<_>>();

    fn find(parents: &mut [usize], mut index: usize) -> usize {
        while parents[index] != index {
            parents[index] = parents[parents[index]];
            index = parents[index];
        }
        index
    }

    for comparison in &comparisons {
        let left = indexes[&comparison.left];
        let right = indexes[&comparison.right];
        let left_root = find(&mut parents, left);
        let right_root = find(&mut parents, right);
        if left_root != right_root {
            let (root, child) = if left_root < right_root {
                (left_root, right_root)
            } else {
                (right_root, left_root)
            };
            parents[child] = root;
        }
    }

    let mut groups = BTreeMap::<usize, Vec<StateLocator>>::new();
    for (index, member) in members.into_iter().enumerate() {
        let root = find(&mut parents, index);
        groups.entry(root).or_default().push(member);
    }
    let mut clusters = groups
        .into_values()
        .map(|members| {
            let mut pair_evidence = comparisons
                .iter()
                .filter(|comparison| {
                    members.binary_search(&comparison.left).is_ok()
                        && members.binary_search(&comparison.right).is_ok()
                })
                .cloned()
                .collect::<Vec<_>>();
            pair_evidence
                .sort_by(|left, right| (&left.left, &left.right).cmp(&(&right.left, &right.right)));
            let match_class = pair_evidence
                .iter()
                .map(|comparison| comparison.image_match)
                .max_by_key(match_kind_rank)
                .unwrap_or(MatchKind::Different);
            let confidence = match match_class {
                MatchKind::Exact => "exact",
                MatchKind::Transformed => "transformed",
                MatchKind::Padded => "padded",
                MatchKind::Palette => "palette",
                MatchKind::Near => "near",
                MatchKind::Different => "different",
            };
            let encoded = serde_json::to_vec(&(confidence, &members))
                .expect("duplicate cluster identities are serializable");
            DuplicateCluster {
                cluster_id: format!("{:x}", Sha256::digest(encoded)),
                confidence,
                members,
                pair_evidence,
            }
        })
        .collect::<Vec<_>>();
    clusters.sort_by(|left, right| {
        (match_confidence_rank(left.confidence), &left.members[0])
            .cmp(&(match_confidence_rank(right.confidence), &right.members[0]))
    });
    clusters
}

fn match_confidence_rank(confidence: &str) -> u8 {
    match confidence {
        "exact" => 0,
        "transformed" => 1,
        "padded" => 2,
        "palette" => 3,
        "near" => 4,
        _ => 5,
    }
}

fn match_kind_rank(kind: &MatchKind) -> u8 {
    match kind {
        MatchKind::Exact => 0,
        MatchKind::Transformed => 1,
        MatchKind::Padded => 2,
        MatchKind::Palette => 3,
        MatchKind::Near => 4,
        MatchKind::Different => 5,
    }
}

async fn duplicate_clusters(
    context: &ToolExecutionContext,
    state: &ServerState,
    root: &Path,
    glob: Option<&str>,
    minimum: f32,
    max_matches: usize,
    scan: &mut AssetScan,
) -> Result<(Vec<DuplicateCluster>, Vec<String>, usize)> {
    let limits = state.asset_limits();
    let (files, mut reasons) = discover_dmis_with_policy(context.policy(), root, glob, limits)?;
    let mut assets = Vec::new();
    for file in files {
        if let Some(asset) = scan.load(context, state, &file, &mut reasons).await? {
            assets.push(asset);
        }
    }
    let mut buckets = BTreeMap::<String, Vec<(usize, usize)>>::new();
    for (asset_index, asset) in assets.iter().enumerate() {
        for (state_index, state) in asset.icon.metadata.states.iter().enumerate() {
            for signature in state_candidate_signatures(asset, state) {
                buckets
                    .entry(signature)
                    .or_default()
                    .push((asset_index, state_index));
            }
        }
    }
    let mut candidate_pairs = BTreeSet::new();
    'candidate_buckets: for bucket in buckets.values() {
        for left in 0..bucket.len() {
            for right in left + 1..bucket.len() {
                candidate_pairs.insert((bucket[left], bucket[right]));
                if candidate_pairs.len() >= limits.max_dmi_candidates {
                    reasons.push("max_dmi_candidates".to_owned());
                    break 'candidate_buckets;
                }
            }
        }
    }
    let candidates = candidate_pairs.len();
    let mut matched_pairs = Vec::new();
    for ((left_asset, left_state), (right_asset, right_state)) in candidate_pairs {
        let left = &assets[left_asset];
        let right = &assets[right_asset];
        let left_state = &left.icon.metadata.states[left_state];
        let right_state = &right.icon.metadata.states[right_state];
        let comparison = compare_states(
            left,
            &left_state.name,
            left_state.duplicate_index,
            right,
            &right_state.name,
            right_state.duplicate_index,
            minimum,
        )?;
        if comparison.image_match != MatchKind::Different {
            matched_pairs.push(comparison);
        }
    }
    let mut clusters = cluster_comparisons(matched_pairs);
    let cluster_limit = max_matches.min(limits.max_dmi_matches);
    if clusters.len() > cluster_limit {
        clusters.truncate(cluster_limit);
        reasons.push("max_dmi_matches".to_owned());
    }
    Ok((clusters, reasons, candidates))
}

pub async fn find_duplicates(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::FindDmiDuplicatesParams,
) -> Result<ToolResult> {
    let root = match args.scope_path.as_deref() {
        Some(path) => PathBuf::from(path),
        None => state
            .snapshot()
            .await?
            .environment_path
            .parent()
            .ok_or_else(|| anyhow!("parsed environment has no root"))?
            .to_owned(),
    };
    let mut scan = AssetScan::default();
    let (clusters, reasons, candidates) = duplicate_clusters(
        context,
        state,
        &root,
        args.include_glob.as_deref(),
        args.minimum_similarity.unwrap_or(0.985) as f32,
        args.max_matches.unwrap_or(10_000) as usize,
        &mut scan,
    )
    .await?;
    let mut metadata = ToolMetadata::complete(None);
    metadata.truncated = !reasons.is_empty();
    metadata.truncation_reasons = reasons;
    Ok(json_success(
        metadata,
        FindDmiDuplicatesData {
            cluster_count: clusters.len(),
            candidate_comparisons: candidates,
            clusters,
        },
    ))
}

pub async fn audit_icons(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::AuditIconsParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let root = args
        .scope_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            snapshot
                .environment_path
                .parent()
                .unwrap_or(Path::new("."))
                .to_owned()
        });
    let mut scan = AssetScan::default();
    let (clusters, mut reasons, candidates) = duplicate_clusters(
        context,
        state,
        &root,
        args.include_glob.as_deref(),
        args.minimum_similarity.unwrap_or(0.985) as f32,
        args.max_matches.unwrap_or(10_000) as usize,
        &mut scan,
    )
    .await?;
    let mut dynamic_references = Vec::new();
    let mut missing_files = Vec::new();
    let mut missing_states = Vec::new();
    let mut referenced_states = BTreeSet::new();
    for reference in snapshot.icon_references.iter() {
        match &reference.resolution {
            IconReferenceResolution::Dynamic { .. } => dynamic_references.push(reference.clone()),
            IconReferenceResolution::Static {
                dmi_path,
                state: icon_state,
            } => {
                if !dmi_path.is_file() {
                    missing_files.push(MissingIconFile {
                        type_path: reference.type_path.clone(),
                        file: reference.file.clone(),
                        line: reference.line,
                        dmi_path: dmi_path.clone(),
                    });
                    continue;
                }
                if let Some(icon_state) = icon_state {
                    let Some(asset) = scan.load(context, state, dmi_path, &mut reasons).await?
                    else {
                        continue;
                    };
                    if !asset
                        .icon
                        .metadata
                        .states
                        .iter()
                        .any(|state| state.name == *icon_state)
                    {
                        missing_states.push(MissingIconState {
                            type_path: reference.type_path.clone(),
                            file: reference.file.clone(),
                            line: reference.line,
                            dmi_path: asset.identity.path.clone(),
                            state: icon_state.clone(),
                        });
                    } else {
                        referenced_states.insert((asset.identity.path.clone(), icon_state.clone()));
                    }
                }
            }
        }
    }
    let mut unused_states = Vec::new();
    if args.include_unused.unwrap_or(false) {
        let (files, unused_reasons) = discover_dmis_with_policy(
            context.policy(),
            &root,
            args.include_glob.as_deref(),
            state.asset_limits(),
        )?;
        for reason in unused_reasons {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
        for file in files {
            let Some(asset) = scan.load(context, state, &file, &mut reasons).await? else {
                continue;
            };
            for icon_state in &asset.icon.metadata.states {
                if !referenced_states
                    .contains(&(asset.identity.path.clone(), icon_state.name.clone()))
                {
                    unused_states.push(UnusedIconState {
                        dmi_path: asset.identity.path.clone(),
                        state: icon_state.name.clone(),
                        duplicate_index: icon_state.duplicate_index,
                        best_effort: true,
                    });
                }
            }
        }
    }
    let complete = dynamic_references.is_empty() && reasons.is_empty();
    let mut metadata = ToolMetadata::for_snapshot(&snapshot);
    metadata.truncated = !reasons.is_empty();
    metadata.truncation_reasons = reasons;
    Ok(json_success(
        metadata,
        AuditIconsData {
            complete,
            missing_files,
            missing_states,
            duplicates: clusters,
            unused_states,
            dynamic_references,
            candidate_comparisons: candidates,
            unused_evidence: "best_effort",
        },
    ))
}

pub async fn extract(
    context: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::ExtractDmiParams,
) -> Result<ToolResult> {
    let source = PathBuf::from(&args.dmi_path);
    let output = PathBuf::from(&args.output_path);
    let asset = load(context, state, &source).await?;
    let state_name = args.state.as_str();
    let duplicate = args.duplicate_index.unwrap_or(0) as u32;
    let icon_state = asset
        .icon
        .metadata
        .states
        .iter()
        .find(|value| value.name == state_name && value.duplicate_index == duplicate)
        .ok_or_else(|| DmiError::Invalid("requested state not found".into()))?;
    let kind = args
        .kind
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("auto");
    let renderer = IconRenderer::new(&asset.icon);
    let automatic = renderer.prepare_render_state(icon_state)?;
    let automatic_encoder = match automatic.render_type {
        RenderType::Png => "png",
        RenderType::Gif => "gif",
    };
    let actual = match kind {
        "auto" => automatic_encoder,
        "png" | "gif" if kind == automatic_encoder => kind,
        "png" | "gif" => {
            return Err(anyhow!(
                "requested {kind} output does not match state encoder {automatic_encoder}"
            ));
        }
        "contact_sheet" | "frame" => "png",
        _ => return Err(anyhow!("unknown extraction kind: {kind}")),
    };
    if output
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case(actual))
    {
        return Err(anyhow!("output extension must be .{actual}"));
    }
    let selected_direction = if kind == "frame" {
        let direction = parse_direction(
            args.direction
                .as_ref()
                .map(|value| value.as_str())
                .unwrap_or("south"),
        )?;
        validate_direction(icon_state.dirs, direction)?;
        Some(direction)
    } else {
        None
    };
    let selected_frame = args.frame.unwrap_or(0) as u32;
    if kind == "frame" && selected_frame as usize >= icon_state.frames.count() {
        return Err(anyhow!("frame is outside the selected state"));
    }
    let mut dimensions = extraction_dimensions(icon_state, &asset.icon, kind);
    let artifact = write_atomic(
        context.policy(),
        &output,
        args.overwrite.unwrap_or(false),
        |file| {
            match kind {
                "auto" | "png" | "gif" => automatic.render(file),
                "contact_sheet" => {
                    let images = renderer.render_to_images(&icon_state.get_state_name_index())?;
                    let width = images.iter().map(|image| image.width).max().unwrap_or(0);
                    let height = images.iter().map(|image| image.height).sum();
                    let mut sheet = Image::new_rgba(width, height);
                    let mut y = 0;
                    for image in &images {
                        sheet.composite(
                            image,
                            (0, y),
                            (0, 0, image.width, image.height),
                            [255, 255, 255, 255],
                        );
                        y += image.height;
                    }
                    dimensions = (width, height);
                    sheet.to_write(file)
                }
                "frame" => {
                    let direction = selected_direction.expect("frame direction was prevalidated");
                    let rect = asset
                        .icon
                        .rect_of_index(icon_state.index_of_frame(direction, selected_frame));
                    let mut image = Image::new_rgba(rect.2, rect.3);
                    image.composite(&asset.icon.image, (0, 0), rect, [255, 255, 255, 255]);
                    image.to_write(file)
                }
                _ => unreachable!(),
            }
            .map_err(|error| AtomicOutputError::writer(error.to_string()))
        },
    )?;
    let mut metadata = ToolMetadata::complete(None);
    metadata.asset_generation = Some(asset.asset_generation);
    let outcome = crate::result::MutationOutcome::installed(0, &artifact, true, None);
    Ok(json_success(
        metadata,
        ExtractDmiData {
            source_path: asset.identity.path,
            source_sha256: asset.identity.sha256,
            output: artifact,
            encoder: actual.into(),
            kind: kind.into(),
            dimensions: [dimensions.0, dimensions.1],
            state: state_name.into(),
            duplicate_index: duplicate,
        },
    )
    .with_outcome(outcome))
}

fn extraction_dimensions(
    state: &dmi::State,
    icon: &dmm_tools::dmi::IconFile,
    kind: &str,
) -> (u32, u32) {
    let direction_width = icon.metadata.width * state.dirs.count() as u32;
    match kind {
        "frame" => (icon.metadata.width, icon.metadata.height),
        "contact_sheet" => (
            direction_width,
            icon.metadata.height * state.frames.count() as u32,
        ),
        _ => (direction_width, icon.metadata.height),
    }
}

fn parse_direction(value: &str) -> Result<Dir> {
    match value.to_ascii_lowercase().as_str() {
        "north" => Ok(Dir::North),
        "south" => Ok(Dir::South),
        "east" => Ok(Dir::East),
        "west" => Ok(Dir::West),
        "northeast" => Ok(Dir::Northeast),
        "northwest" => Ok(Dir::Northwest),
        "southeast" => Ok(Dir::Southeast),
        "southwest" => Ok(Dir::Southwest),
        _ => Err(anyhow!("unknown DMI direction: {value}")),
    }
}

fn validate_direction(dirs: Dirs, direction: Dir) -> Result<()> {
    let supported = match dirs {
        Dirs::One => matches!(direction, Dir::South),
        Dirs::Four => matches!(direction, Dir::South | Dir::North | Dir::East | Dir::West),
        Dirs::Eight => true,
    };
    if supported {
        Ok(())
    } else {
        Err(anyhow!("direction is not present in the selected state"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaceman::dmi::StateComparison;

    fn locator(state: &str) -> StateLocator {
        StateLocator {
            dmi_path: PathBuf::from("technical.dmi"),
            state: state.to_owned(),
            duplicate_index: 0,
        }
    }

    fn comparison(left: &str, right: &str) -> StateComparison {
        StateComparison {
            left: locator(left),
            right: locator(right),
            image_match: MatchKind::Exact,
            metadata_differences: Vec::new(),
            frames: Vec::new(),
        }
    }

    #[test]
    fn connected_duplicate_pairs_form_one_stable_cluster() {
        let clusters = cluster_comparisons(vec![
            comparison("alpha", "beta"),
            comparison("beta", "gamma"),
        ]);

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].members.len(), 3);
        assert_eq!(clusters[0].pair_evidence.len(), 2);
        assert_eq!(clusters[0].members[0].state, "alpha");
        assert_eq!(clusters[0].members[2].state, "gamma");
    }
}
