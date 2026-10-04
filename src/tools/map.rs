use anyhow::{anyhow, Result};
use dmm_tools::{minimap, render_passes, IconCache};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::info;

use crate::atomic_output::{write_atomic, AtomicOutputError};
use crate::limits::ServerLimits;
use crate::mcp::{ToolContent, ToolResult};
use crate::result::{json_success, ToolMetadata};
use crate::spaceman::dmm::{
    diff_maps as calculate_diff, key_counts, load_map, profile_loaded_map, render_pass_inventory,
};
use crate::state::ServerState;
use crate::tools::ToolExecutionContext;

pub(crate) fn pass_selection<'a>(
    enabled: &'a [String],
    disabled: &'a [String],
) -> Result<(Vec<&'a str>, Vec<&'a str>)> {
    let enabled = enabled.iter().map(String::as_str).collect::<Vec<_>>();
    let disabled = disabled.iter().map(String::as_str).collect::<Vec<_>>();
    let known = render_pass_inventory()
        .into_iter()
        .map(|pass| pass.name)
        .collect::<std::collections::BTreeSet<_>>();
    for name in enabled.iter().chain(disabled.iter()) {
        if !known.contains(*name) {
            return Err(anyhow!("unknown render pass: {name}"));
        }
    }
    if enabled.iter().any(|name| disabled.contains(name)) {
        return Err(anyhow!("a render pass cannot be both enabled and disabled"));
    }
    Ok((enabled, disabled))
}

fn validated_render_bounds(
    args: &crate::parameters::RenderMapParams,
    dimensions: (usize, usize, usize),
) -> Result<(usize, [usize; 3], [usize; 3])> {
    let (dim_x, dim_y, dim_z) = dimensions;
    let z_level = args.z_level.unwrap_or(1) as usize;
    if z_level == 0 || z_level > dim_z {
        return Err(anyhow!(
            "Z-level {z_level} is outside the map range 1..={dim_z}"
        ));
    }
    let min = args
        .min
        .map(|values| values.map(|value| value.0 as usize))
        .unwrap_or([1, 1, z_level]);
    let max = args
        .max
        .map(|values| values.map(|value| value.0 as usize))
        .unwrap_or([dim_x, dim_y, z_level]);
    if min.contains(&0)
        || min[0] > max[0]
        || min[1] > max[1]
        || min[2] != max[2]
        || max[0] > dim_x
        || max[1] > dim_y
        || max[2] > dim_z
    {
        return Err(anyhow!("render bounds are outside the map"));
    }
    let pixel_count = (max[0] - min[0] + 1) as u64 * (max[1] - min[1] + 1) as u64 * 32 * 32;
    if pixel_count > ServerLimits::default().max_render_pixels {
        return Err(anyhow!("render exceeds max_render_pixels"));
    }
    Ok((z_level, min, max))
}

/// Render a map to PNG.
pub async fn render_map(
    execution: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::RenderMapParams,
) -> Result<ToolResult> {
    let dmm_path = args.dmm_path.as_str();
    let output_path = args
        .output_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(dmm_path).with_extension("png"));
    let path = PathBuf::from(dmm_path);
    if !path.exists() {
        return Ok(ToolResult::error(format!("File not found: {dmm_path}")));
    }

    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;
    let environment_root = snapshot
        .environment_path
        .parent()
        .ok_or_else(|| anyhow!("Parsed environment has no parent directory"))?;

    let map = load_map(&execution.policy().read_path(&path)?)?;
    let (z_level, min, max) = validated_render_bounds(&args, map.dim_xyz())?;
    info!("Rendering map: {dmm_path} z-level {z_level} to {output_path:?}");

    let mut icon_cache =
        IconCache::with_read_policy(std::sync::Arc::new(execution.policy().clone()));
    icon_cache.set_icons_root(environment_root);
    let (enabled, disabled) = pass_selection(
        args.enable_passes.as_deref().unwrap_or_default(),
        args.disable_passes.as_deref().unwrap_or_default(),
    )?;
    let render_passes =
        render_passes::configure_list(&context.config.map_renderer, &enabled, &disabled);
    let errors: RwLock<_> = Default::default();
    let bump = Default::default();
    let image = minimap::generate(
        minimap::Context {
            objtree,
            map: &map,
            level: map.z_level(min[2] - 1),
            min: (min[0] - 1, min[1] - 1),
            max: (max[0] - 1, max[1] - 1),
            render_passes: &render_passes,
            errors: &errors,
            bump: &bump,
            print_errors: false,
        },
        &icon_cache,
    )
    .map_err(|()| anyhow!("SpacemanDMM could not render the requested map"))?;
    if icon_cache.read_denied() {
        return Err(anyhow!(
            "path_outside_workspace: map resource read denied by startup policy"
        ));
    }
    let non_transparent_pixels = image.data.iter().filter(|pixel| pixel.a > 0).count();

    let mut encoded = Vec::new();
    image
        .to_write(&mut encoded)
        .map_err(|error| anyhow!(error.to_string()))?;
    if encoded.len() as u64 > ServerLimits::default().max_render_output_bytes {
        return Err(anyhow!("render exceeds max_render_output_bytes"));
    }
    let artifact = write_atomic(
        execution.policy(),
        &output_path,
        args.overwrite.unwrap_or(false),
        |file| file.write_all(&encoded).map_err(AtomicOutputError::from),
    )?;

    Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
        "success": true,
        "dmm_path": dmm_path,
        "z_level": z_level,
        "output": artifact,
        "output_path": output_path.display().to_string(),
        "dimensions_pixels": {"width": (max[0]-min[0]+1) * 32, "height": (max[1]-min[1]+1) * 32},
        "bounds": {"min":min,"max":max},
        "applied_passes": enabled,
        "non_transparent_pixels": non_transparent_pixels,
        "warning": if non_transparent_pixels == 0 {
            Some("The renderer produced a fully transparent image. This can be expected when render passes hide every atom on the selected z-level.")
        } else {
            None
        }
    }))?))
}

/// Get map dimensions and atom instance statistics.
pub async fn map_info(args: crate::parameters::MapInfoParams) -> Result<ToolResult> {
    let dmm_path = args.dmm_path.as_str();
    let path = PathBuf::from(dmm_path);
    if !path.exists() {
        return Ok(ToolResult::error(format!("File not found: {dmm_path}")));
    }

    info!("Getting map info: {dmm_path}");
    let map = load_map(&path)?;
    let (dim_x, dim_y, dim_z) = map.dim_xyz();
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    let mut area_counts: HashMap<String, usize> = HashMap::new();

    for (key, count) in key_counts(&map) {
        let prefabs = map
            .dictionary
            .get(&key)
            .ok_or_else(|| anyhow!("Map grid references a missing dictionary key"))?;
        for prefab in prefabs {
            if let Some(base_type) = prefab
                .path
                .split('/')
                .nth(1)
                .filter(|segment| !segment.is_empty())
                .map(|segment| format!("/{segment}"))
            {
                *type_counts.entry(base_type).or_default() += count;
            }
            if prefab.path == "/area" || prefab.path.starts_with("/area/") {
                *area_counts.entry(prefab.path.clone()).or_default() += count;
            }
        }
    }

    let mut sorted_types: Vec<_> = type_counts.into_iter().collect();
    sorted_types.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let mut sorted_areas: Vec<_> = area_counts.into_iter().collect();
    sorted_areas.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let profile = profile_loaded_map(&path, &map, 10_000)?;
    Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
        "file": dmm_path,
        "format": profile.format,
        "dimensions": {"x": dim_x, "y": dim_y, "z": dim_z},
        "unique_tiles": map.dictionary.len(),
        "file_size_bytes": std::fs::metadata(&path)?.len(),
        "top_types": sorted_types.into_iter().take(20).collect::<Vec<_>>(),
        "top_areas": sorted_areas.into_iter().take(20).collect::<Vec<_>>(),
        "bounds": profile.bounds,
        "dictionary_entries": profile.dictionary_entries,
        "unique_models": profile.unique_models,
        "model_use_counts": profile.model_use_counts,
        "warnings": profile.warnings,
        "spacemandmm_revision": crate::capabilities::SPACEMANDMM_REVISION
    }))?))
}

pub async fn diff_maps(args: crate::parameters::DiffMapsParams) -> Result<ToolResult> {
    let left = PathBuf::from(args.left_dmm_path.as_str());
    let right = PathBuf::from(args.right_dmm_path.as_str());
    let maximum = ServerLimits::default().max_map_differences;
    let limit = args.limit.unwrap_or(maximum as u64) as usize;
    let difference = calculate_diff(&left, &right, limit.min(maximum))?;
    let mut metadata = ToolMetadata::complete(None);
    metadata.truncated = difference.truncated;
    if difference.truncated {
        metadata
            .truncation_reasons
            .push("map_difference_limit".into())
    }
    Ok(json_success(metadata, json!({"difference":difference})))
}

pub async fn list_render_passes() -> Result<ToolResult> {
    Ok(json_success(
        ToolMetadata::complete(None),
        json!({"passes":render_pass_inventory()}),
    ))
}

pub async fn render_maps(
    execution: &ToolExecutionContext,
    state: &ServerState,
    args: crate::parameters::RenderMapsParams,
) -> Result<ToolResult> {
    let files = &args.files;
    let limits = ServerLimits::default();
    if files.len() > limits.max_render_files {
        return Err(anyhow!("batch exceeds max_render_files"));
    }
    let overwrite = args.overwrite.unwrap_or(false);
    let _ = pass_selection(
        args.enable_passes.as_deref().unwrap_or_default(),
        args.disable_passes.as_deref().unwrap_or_default(),
    )?;
    let mut requests = Vec::new();
    for file in files {
        let dmm = file.dmm_path.as_str();
        let dmm = execution.policy().read_path(dmm)?;
        let map = load_map(&dmm)?;
        let chunks = &file.chunks;
        if requests.len() + chunks.len() > limits.max_render_chunks {
            return Err(anyhow!("batch exceeds max_render_chunks"));
        }
        for chunk in chunks {
            let output = chunk.output_path.as_str();
            let output = execution.policy().output_path(output, overwrite)?;
            if output
                .extension()
                .and_then(|extension| extension.to_str())
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("png"))
            {
                return Err(anyhow!("render output extension must be .png"));
            }
            let request = crate::parameters::RenderMapParams {
                dmm_path: dmm.display().to_string(),
                output_path: Some(output.display().to_string()),
                overwrite: Some(overwrite),
                z_level: chunk.z_level,
                min: chunk.min,
                max: chunk.max,
                enable_passes: args.enable_passes.clone(),
                disable_passes: args.disable_passes.clone(),
            };
            let _ = validated_render_bounds(&request, map.dim_xyz())?;
            requests.push(request)
        }
    }
    let mut results = Vec::new();
    let mut completed = 0;
    let mut failed = 0;
    for request in requests {
        match render_map(execution, state, request.clone()).await {
            Ok(result) if result.is_error != Some(true) => {
                completed += 1;
                results.push(json!({"success":true,"result":tool_result_payload(&result)}));
            }
            Ok(result) => {
                failed += 1;
                results.push(json!({"success":false,"result":tool_result_payload(&result)}));
            }
            Err(error) => {
                failed += 1;
                results.push(json!({"success":false,"error":error.to_string(),"request":request}));
            }
        }
    }
    Ok(json_success(
        ToolMetadata::complete(
            state
                .active_snapshot()
                .await
                .map(|snapshot| snapshot.generation),
        ),
        json!({"completed":completed,"failed":failed,"files":results}),
    ))
}

fn tool_result_payload(result: &ToolResult) -> Value {
    match result.content.first() {
        Some(ToolContent::Text { text }) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        None => Value::Null,
    }
}

/// Find exact type and subtype instances on a map.
pub async fn find_on_map(args: crate::parameters::FindOnMapParams) -> Result<ToolResult> {
    let dmm_path = args.dmm_path.as_str();
    let type_path = args.type_path.as_str();
    let path = PathBuf::from(dmm_path);
    if !path.exists() {
        return Ok(ToolResult::error(format!("File not found: {dmm_path}")));
    }

    info!("Finding {type_path} on map {dmm_path}");
    let map = load_map(&path)?;
    let descendant_prefix = format!("{}/", type_path.trim_end_matches('/'));
    let mut matching_tiles: BTreeMap<_, Vec<&str>> = BTreeMap::new();
    for (key, prefabs) in &map.dictionary {
        let matches: Vec<_> = prefabs
            .iter()
            .filter(|prefab| {
                prefab.path == type_path || prefab.path.starts_with(&descendant_prefix)
            })
            .map(|prefab| prefab.path.as_str())
            .collect();
        if !matches.is_empty() {
            matching_tiles.insert(*key, matches);
        }
    }

    let keys: Vec<_> = matching_tiles
        .keys()
        .map(|key| map.format_key(*key).to_string())
        .collect();
    let mut coordinates = Vec::new();
    for (z, level) in map.iter_levels() {
        for (coordinate, key) in level.iter_top_down() {
            if let Some(matches) = matching_tiles.get(&key) {
                for matched_type in matches {
                    coordinates.push(json!({
                        "x": coordinate.x,
                        "y": coordinate.y,
                        "z": z,
                        "tile_key": map.format_key(key).to_string(),
                        "matched_type": matched_type
                    }));
                }
            }
        }
    }

    Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
        "type_path": type_path,
        "dmm_path": dmm_path,
        "count": coordinates.len(),
        "matching_tile_keys": keys.len(),
        "keys": keys,
        "coordinates": coordinates
    }))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::ToolContent;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn fixture_directory() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "meridian-mcp-map-{}-{unique}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn result_json(result: &ToolResult) -> Value {
        let ToolContent::Text { text } = &result.content[0];
        serde_json::from_str(text).expect("tool result should be JSON")
    }

    fn write_map(directory: &Path) -> PathBuf {
        let path = directory.join("fixture.dmm");
        std::fs::write(
            &path,
            r#"//MAP CONVERTED BY dmm2tgm.py THIS HEADER COMMENT MUST NOT COUNT AS A TYPE
"aa" = (/obj/item/test,/turf/open/space,/area/space)
"ab" = (/turf/open/space,/area/space)

(1,1,1) = {"
aa
ab
"}
(2,1,1) = {"
ab
aa
"}
"#,
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn map_info_uses_parsed_grid_dimensions_and_instance_counts() {
        let directory = fixture_directory();
        let path = write_map(&directory);
        let result = map_info(
            crate::parameters::decode(json!({"dmm_path": path})).expect("valid fixture request"),
        )
        .await
        .unwrap();
        let payload = result_json(&result);

        assert_eq!(payload["dimensions"], json!({"x": 2, "y": 2, "z": 1}));
        assert_eq!(payload["unique_tiles"], 2);
        assert_eq!(payload["top_types"][0], json!(["/area", 4]));
        assert!(payload["top_types"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry[0] != "/"));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn find_on_map_returns_exact_byond_coordinates() {
        let directory = fixture_directory();
        let path = write_map(&directory);
        let result = find_on_map(
            crate::parameters::decode(json!({
                "dmm_path": path,
                "type_path": "/obj/item/test"
            }))
            .expect("valid fixture request"),
        )
        .await
        .unwrap();
        let payload = result_json(&result);

        assert_eq!(payload["count"], 2);
        assert_eq!(
            payload["coordinates"],
            json!([
                {"x": 1, "y": 2, "z": 1, "tile_key": "aa", "matched_type": "/obj/item/test"},
                {"x": 2, "y": 1, "z": 1, "tile_key": "aa", "matched_type": "/obj/item/test"}
            ])
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn render_map_writes_a_real_png() {
        let directory = fixture_directory();
        let dme_path = directory.join("fixture.dme");
        std::fs::write(&dme_path, "// map render fixture\n").unwrap();
        let map_path = directory.join("render.dmm");
        std::fs::write(
            &map_path,
            r#""a" = (/turf,/area)

(1,1,1) = {"
a
"}
"#,
        )
        .unwrap();
        let output_path = directory.join("render.png");
        let state = ServerState::new();
        let parsed = crate::tools::parse::parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        assert_eq!(parsed.is_error, None, "parse result: {parsed:?}");
        let execution = ToolExecutionContext::new(
            crate::CapabilityMode::Development,
            crate::PathPolicy::new(vec![directory.clone()], Vec::new()).unwrap(),
        );

        let result = render_map(
            &execution,
            &state,
            crate::parameters::decode(json!({
                "dmm_path": map_path,
                "output_path": output_path,
                "z_level": 1
            }))
            .expect("valid fixture request"),
        )
        .await
        .unwrap();
        assert_eq!(result.is_error, None, "render result: {result:?}");
        let payload = result_json(&result);
        assert_eq!(payload["non_transparent_pixels"], 0);
        assert!(payload["warning"]
            .as_str()
            .unwrap()
            .contains("fully transparent"));
        let bytes = std::fs::read(&output_path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");

        std::fs::remove_dir_all(directory).unwrap();
    }
}
