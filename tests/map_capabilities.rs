use meridian_mcp::spaceman::dmm::{diff_maps, profile_map};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn write_map(path: &std::path::Path, dictionary: &str, levels: &[&str]) {
    let mut text = dictionary.to_owned();
    for (z, rows) in levels.iter().enumerate() {
        text.push_str(&format!("\n(1,1,{}) = {{\"\n{rows}\n\"}}\n", z + 1));
    }
    std::fs::write(path, text).unwrap();
}

#[test]
fn map_diff_ignores_dictionary_keys_and_variable_order_but_preserves_atom_order() {
    let (root, _, left) = fixture();
    let right = root.join("right.dmm");
    write_map(
        &left,
        "\"a\" = (/obj/test {a = 1; b = 2},/turf,/area)\n",
        &["a"],
    );
    write_map(
        &right,
        "\"z\" = (/obj/test {b = 2; a = 1},/turf,/area)\n",
        &["z"],
    );
    let diff = diff_maps(&left, &right, 0).unwrap();
    assert!(
        diff.coordinates.is_empty() && !diff.truncated,
        "variable order is not a map change"
    );
    for model in [
        "/obj/test {b = 3; a = 1},/turf,/area",
        "/turf,/obj/test {a = 1; b = 2},/area",
    ] {
        write_map(&right, &format!("\"z\" = ({model})\n"), &["z"]);
        let diff = diff_maps(&left, &right, 1).unwrap();
        assert_eq!(diff.coordinates.len(), 1);
        assert!(!diff.truncated);
        assert_ne!(diff.coordinates[0].left, diff.coordinates[0].right);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn map_diff_preserves_coordinate_order_dimensions_and_exact_limit_boundaries() {
    let (root, _, left) = fixture();
    let right = root.join("right.dmm");
    let dictionary = "\"a\" = (/turf/a,/area)\n\"b\" = (/turf/b,/area)\n";
    write_map(&left, dictionary, &["aa\naa", "aa\naa"]);
    write_map(&right, dictionary, &["ba\nab", "ab\nba"]);
    let diff = diff_maps(&left, &right, 4).unwrap();
    let coordinates: Vec<_> = diff
        .coordinates
        .iter()
        .map(|item| (item.x, item.y, item.z))
        .collect();
    assert_eq!(coordinates, [(1, 1, 2), (1, 2, 1), (2, 1, 1), (2, 2, 2)]);
    assert!(!diff.truncated);
    let short = diff_maps(&left, &right, 3).unwrap();
    assert!(short.truncated);
    assert_eq!(
        short
            .coordinates
            .iter()
            .map(|item| (item.x, item.y, item.z))
            .collect::<Vec<_>>(),
        coordinates[..3]
    );
    assert!(diff_maps(&left, &right, 0).unwrap().truncated);
    write_map(&left, dictionary, &["aaa"]);
    write_map(&right, dictionary, &["a\na\na", "a\na\na"]);
    let asymmetric = diff_maps(&left, &right, 10).unwrap();
    assert_eq!(asymmetric.left_dimensions, [3, 1, 1]);
    assert_eq!(asymmetric.right_dimensions, [1, 3, 2]);
    assert_eq!(asymmetric.coordinates.len(), 7);
    assert_eq!(
        asymmetric
            .coordinates
            .iter()
            .map(|item| (item.x, item.y, item.z))
            .collect::<Vec<_>>(),
        [
            (1, 1, 2),
            (1, 2, 1),
            (1, 2, 2),
            (1, 3, 1),
            (1, 3, 2),
            (2, 1, 1),
            (3, 1, 1)
        ]
    );
    assert!(asymmetric.coordinates[..5]
        .iter()
        .all(|item| item.left.is_none()));
    assert!(asymmetric.coordinates[5..]
        .iter()
        .all(|item| item.right.is_none()));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn map_info_profiles_one_loaded_map_and_accepts_latin1() {
    let (root, _, map) = fixture();
    write_map(&map, "\"a\" = (/obj/item,/obj/item,/turf,/area/shared)\n\"b\" = (/obj/item,/obj/item,/turf,/area/shared)\n\"c\" = (/turf,/area/other)\n\"d\" = (/obj/unused,/turf,/area)\n", &["aac\nbbc"]);
    let mut bytes = std::fs::read(&map).unwrap();
    bytes.extend_from_slice(b"// caf\xe9\n//");
    assert!(bytes.len() < 8190);
    bytes.resize(8190, b'x');
    // Start the marker at byte 8191, across the format scanner's read boundary.
    bytes.extend_from_slice(b"\n//MAP CONVERTED BY dmm2tgm.py\n");
    std::fs::write(&map, bytes).unwrap();
    let profile = profile_map(&map, 1).expect("valid Latin-1 maps must be inspectable");
    assert_eq!(profile.format, "TGM");
    assert_eq!(profile.unique_models, 2);
    assert_eq!(profile.dictionary_entries, 4);
    assert_eq!(profile.model_use_counts.len(), 1);
    assert_eq!(profile.model_use_counts[0].count, 4);
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let result = call_tool(
        &context,
        &ServerState::new(),
        "dm_map_info",
        json!({"dmm_path":map}),
    )
    .await
    .unwrap();
    let meridian_mcp::result::ToolContent::Text { text } = &result.content[0];
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(value["unique_models"], 2);
    assert_eq!(value["model_use_counts"][0]["count"], 4);
    assert_eq!(
        value["top_types"],
        json!([["/obj", 8], ["/area", 6], ["/turf", 6]])
    );
    assert_eq!(
        value["top_areas"],
        json!([["/area/shared", 4], ["/area/other", 2]])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn map_grid_limits_reject_sparse_and_overflowing_coordinates() {
    let (root, _, path) = fixture();
    assert!(dmm_tools::dmm::Map::from_file_with_cell_limit(&path, 1).is_ok());
    assert!(dmm_tools::dmm::Map::from_file_with_cell_limit(&path, 0).is_err());
    for coordinate in [
        "1000000,1000000,1",
        "184467440737095516160,1,1",
        "0,1,1",
        "1,1",
    ] {
        std::fs::write(
            &path,
            format!("\"a\" = (/turf,/area)\n\n({coordinate}) = {{\"\na\n\"}}\n"),
        )
        .unwrap();
        assert!(
            meridian_mcp::spaceman::dmm::load_map(&path).is_err(),
            "accepted {coordinate}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn multi_block_maps_retain_the_tallest_block() {
    let (root, _, path) = fixture();
    std::fs::write(&path, "\"a\" = (/turf,/area)\n\n(1,1,1) = {\"\na\na\n\"}\n(2,1,1) = {\"\na\na\n\"}\n(3,1,1) = {\"\na\n\"}\n(3,2,1) = {\"\na\n\"}\n").unwrap();
    let map = dmm_tools::dmm::Map::from_file_with_cell_limit(&path, 6).unwrap();
    assert_eq!(map.dim_xyz(), (3, 2, 1));
    assert!(dmm_tools::dmm::Map::from_file_with_cell_limit(&path, 5).is_err());
    std::fs::write(
        &path,
        "\"a\" = (/turf,/area)\n\n(1,1,1) = {\"\na\na\n\"}\n(1,1,2) = {\"\na\n\"}\n",
    )
    .unwrap();
    assert!(
        dmm_tools::dmm::Map::from_file_with_cell_limit(&path, 4).is_err(),
        "a later short block must not hide a missing row"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-map-capabilities-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let dme = root.join("fixture.dme");
    let map = root.join("fixture.dmm");
    std::fs::write(&dme, "/turf\n/area\n").unwrap();
    std::fs::write(
        &map,
        r#""a" = (/turf,/area)

(1,1,1) = {"
a
"}
"#,
    )
    .unwrap();
    (root, dme, map)
}

#[tokio::test]
async fn batch_preflight_rejects_late_invalid_chunks_before_any_write() {
    let (root, dme, map) = fixture();
    let output = root.join("first.png");
    let context = ToolExecutionContext::new(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::new();
    call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path": dme}),
    )
    .await
    .unwrap();
    let result = call_tool(
        &context,
        &state,
        "dm_render_maps",
        json!({
            "files": [{
                "dmm_path": map,
                "chunks": [
                    {"output_path": output, "min": [1,1,1], "max": [1,1,1]},
                    {"output_path": root.join("invalid.png"), "min": [2,1,1], "max": [2,1,1]}
                ]
            }]
        }),
    )
    .await;

    assert!(result.is_err(), "invalid batch must fail during preflight");
    assert!(!output.exists(), "preflight failure wrote an earlier chunk");
    std::fs::remove_dir_all(root).unwrap();
}
