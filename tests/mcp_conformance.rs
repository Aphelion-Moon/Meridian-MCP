use meridian_mcp::{CapabilityMode, MeridianServer, RiftBuildAccess, ServerConfig};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

static SERVER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn server(mode: &str, access: RiftBuildAccess) -> MeridianServer {
    let sequence = SERVER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-mode-{mode}-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let state = std::env::temp_dir().join(format!(
        "meridian-mcp-mode-state-{mode}-{}-{sequence}",
        std::process::id()
    ));
    if mode == "development" {
        std::fs::create_dir_all(&state).unwrap();
    }
    let access = match access {
        RiftBuildAccess::Disabled => None,
        RiftBuildAccess::Offline => Some("offline"),
        RiftBuildAccess::Network => Some("network"),
    };
    MeridianServer::new(
        ServerConfig::from_values_with_rift_build_and_state(
            Some(mode),
            vec![root],
            Vec::new(),
            access,
            (mode == "development").then_some(state),
        )
        .unwrap(),
    )
    .unwrap()
}

fn tracy_server() -> MeridianServer {
    let sequence = SERVER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "meridian-mcp-tracy-mode-{}-{sequence}",
        std::process::id()
    ));
    let state = std::env::temp_dir().join(format!(
        "meridian-mcp-tracy-state-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(root.join("helpers")).unwrap();
    let server_helper = root.join("helpers/server-helper.exe");
    let hook = root.join("helpers/prof.dll");
    std::fs::write(&server_helper, b"server helper").unwrap();
    std::fs::write(&hook, b"hook").unwrap();
    let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let manifest = root.join("manifest.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "helpers": [
                {"id":"tracy-server-helper","platform":std::env::consts::OS,"target_arch":std::env::consts::ARCH,"path":"helpers/server-helper.exe","sha256":hash(b"server helper"),"source_revision":"099df3de3dc37eca4712c06b8320fb9c53596edd","protocol_version":82},
                {"id":"byond-tracy","platform":std::env::consts::OS,"target_arch":"x86","path":"helpers/prof.dll","sha256":hash(b"hook"),"source_revision":"d1ec404737b04b1ea73d6df4a1b477deacdb1900","protocol_version":82,"byond_min_version":"516.1687","byond_max_version":"516.1687"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    MeridianServer::new(
        ServerConfig::from_values_with_features_and_state(
            Some("development"),
            vec![root],
            Vec::new(),
            None,
            Some("byond"),
            Some(manifest),
            Some(state),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn mode_inventories_are_exact_and_exclude_removed_protocol() {
    let analysis = server("analysis", RiftBuildAccess::Network).tool_names();
    let development = server("development", RiftBuildAccess::Disabled).tool_names();
    let development_offline = server("development", RiftBuildAccess::Offline).tool_names();
    let development_network = server("development", RiftBuildAccess::Network).tool_names();
    assert_eq!(analysis.len(), 26);
    assert_eq!(development.len(), 35);
    for tool in [
        "dm_document_symbols",
        "dm_find_references",
        "dm_find_implementations",
        "dm_dmi_info",
        "dm_compare_dmi_states",
        "dm_find_dmi_duplicates",
        "dm_audit_icons",
        "dm_diff_maps",
        "dm_list_render_passes",
        "dm_check_fixture_sync",
        "dm_native_evidence_summary",
        "dm_memory_summary",
        "dm_memory_compare",
        "dm_native_evidence_compare",
    ] {
        assert!(
            analysis.contains(&tool.to_owned()),
            "missing analysis tool {tool}"
        );
    }
    for tool in ["dm_extract_dmi", "dm_render_maps"] {
        assert!(!analysis.contains(&tool.to_owned()));
        assert!(
            development.contains(&tool.to_owned()),
            "missing development tool {tool}"
        );
    }
    assert!(analysis.contains(&"dm_parse_environment".to_owned()));
    assert!(analysis.contains(&"dm_server_status".to_owned()));
    assert!(development.contains(&"dm_server_status".to_owned()));
    assert!(!analysis.contains(&"dm_compile".to_owned()));
    assert!(development.contains(&"dm_compile".to_owned()));
    assert!(!development.contains(&"dm_connect_test".to_owned()));
    assert!(!analysis.contains(&"rift_compile".to_owned()));
    assert!(!development.contains(&"rift_compile".to_owned()));
    #[cfg(windows)]
    {
        assert!(development_offline.contains(&"rift_compile".to_owned()));
        assert!(development_network.contains(&"rift_compile".to_owned()));
    }
    #[cfg(not(windows))]
    {
        assert!(!development_offline.contains(&"rift_compile".to_owned()));
        assert!(!development_network.contains(&"rift_compile".to_owned()));
    }
    assert_eq!(CapabilityMode::Analysis, CapabilityMode::Analysis);
}

#[test]
fn rift_compile_schema_has_no_caller_controlled_paths_or_commands() {
    let definitions = meridian_mcp::tools::get_tool_definitions();
    let tool = definitions
        .iter()
        .find(|tool| tool.name == "rift_compile")
        .expect("rift_compile schema should be registered");
    let properties = tool.input_schema["properties"]
        .as_object()
        .expect("properties must be an object");
    let names = properties
        .keys()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        names,
        [
            "network_mode",
            "timeout_ms",
            "idle_timeout_ms",
            "capture_network",
            "force_rebuild",
            "fixture_manifest_path",
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        properties["network_mode"]["enum"],
        serde_json::json!(["offline", "allow"])
    );
    assert_eq!(properties["timeout_ms"]["minimum"], 1_000);
    assert_eq!(properties["timeout_ms"]["maximum"], 1_800_000);
    assert_eq!(properties["idle_timeout_ms"]["minimum"], 1_000);
    assert_eq!(properties["idle_timeout_ms"]["maximum"], 900_000);
}

#[test]
fn tracy_inventory_is_opt_in_and_exposes_fixed_command_tools_only() {
    let ordinary = server("development", RiftBuildAccess::Disabled).tool_names();
    assert!(!ordinary.iter().any(|name| name.starts_with("dm_tracy_")));

    let tracy = tracy_server().tool_names();
    let tracy_tools = tracy
        .iter()
        .filter(|name| name.starts_with("dm_tracy_"))
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        tracy_tools,
        [
            "dm_tracy_prepare",
            "dm_tracy_launch",
            "dm_tracy_capture",
            "dm_tracy_status",
            "dm_tracy_stop",
            "dm_tracy_hotspots",
            "dm_tracy_zone",
            "dm_tracy_frame_stats",
            "dm_tracy_compare",
            "dm_tracy_control_stats",
        ]
        .into_iter()
        .collect()
    );
    assert!(!tracy.iter().any(|name| name.contains("eval")));

    let definitions = meridian_mcp::tools::get_tool_definitions();
    for tool_name in ["dm_tracy_launch", "dm_tracy_capture"] {
        let properties = definitions
            .iter()
            .find(|tool| tool.name == tool_name)
            .unwrap()
            .input_schema["properties"]
            .as_object()
            .unwrap();
        for field in [
            "map",
            "seed",
            "configuration_profile",
            "feature_set",
            "scenario",
            "external_run_id",
            "annotations",
        ] {
            assert!(
                properties.contains_key(field),
                "{tool_name} omitted {field}"
            );
        }
        assert_eq!(properties["annotations"]["maxProperties"], 32);
    }
    let launch = definitions
        .iter()
        .find(|tool| tool.name == "dm_tracy_launch")
        .unwrap();
    assert!(launch.input_schema["properties"]["experiment_directory"].is_object());
    assert_eq!(
        launch.input_schema["properties"]["startup_timeout_ms"]["default"],
        60_000
    );
    assert_eq!(
        launch.input_schema["properties"]["config_directory"]["type"],
        "string"
    );
    assert_eq!(
        launch.input_schema["properties"]["wake_sleeping_world"]["type"],
        "boolean"
    );
    assert_eq!(
        launch.input_schema["properties"]["wake_sleeping_world"]["default"],
        true
    );
    assert_eq!(
        launch.input_schema["properties"]["initialization_timeout_ms"]["maximum"],
        300_000
    );
    assert!(launch.input_schema["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("experiment_directory")));
    let control = definitions
        .iter()
        .find(|tool| tool.name == "dm_tracy_control_stats")
        .unwrap();
    assert_eq!(
        control.input_schema["properties"]["trace_paths"]["minItems"],
        3
    );
    assert_eq!(
        control.input_schema["properties"]["trace_paths"]["maxItems"],
        20
    );
    assert_eq!(
        control.input_schema["properties"]["comparison_mode"]["enum"],
        serde_json::json!(["same_experiment_same_phase", "cross_experiment"])
    );
}

#[test]
fn all_runtime_launch_schemas_expose_the_same_provenance_override() {
    let definitions = meridian_mcp::tools::get_tool_definitions();
    for name in ["dm_run", "dm_debug_launch", "dm_tracy_launch"] {
        let tool = definitions
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(
            tool.input_schema["properties"]["require_verified_provenance"]["type"], "boolean",
            "{name}"
        );
        assert_eq!(
            tool.input_schema["properties"]["require_verified_provenance"]["default"], false,
            "{name}"
        );
    }
}
