use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
