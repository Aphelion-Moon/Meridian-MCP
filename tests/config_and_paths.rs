use meridian_mcp::{
    CapabilityMode, MeridianServer, PathPolicy, RiftBuildAccess, ServerConfig, ToolProfile,
    TracyAccess,
};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn startup_tool_profiles_are_strict_and_default_to_the_existing_catalog() {
    let root = fixture();
    let config = ServerConfig::from_values(None, vec![root.clone()], vec![]).unwrap();
    assert_eq!(config.tool_profile(), ToolProfile::All);
    let default_names = MeridianServer::new(config.clone()).unwrap().tool_names();
    let explicit_all = MeridianServer::new(config.clone().with_tool_profile(ToolProfile::All))
        .unwrap()
        .tool_names();
    assert_eq!(default_names, explicit_all);
    assert_eq!(default_names.len(), 26);
    for (value, profile, count) in [
        ("all", ToolProfile::All, 26),
        ("code", ToolProfile::Code, 14),
        ("assets", ToolProfile::Assets, 10),
        ("runtime", ToolProfile::Runtime, 6),
    ] {
        assert_eq!(value.parse::<ToolProfile>().unwrap(), profile);
        let selected = config.clone().with_tool_profile(profile);
        assert_eq!(selected.mode(), CapabilityMode::Analysis);
        let names = MeridianServer::new(selected).unwrap().tool_names();
        assert_eq!(names.len(), count, "{value}");
        assert!(names.iter().any(|name| name == "dm_server_status"));
        assert!(names.iter().any(|name| name == "dm_parse_environment"));
        let positions: Vec<_> = names
            .iter()
            .map(|name| default_names.iter().position(|item| item == name).unwrap())
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }
    for invalid in ["", "ALL", "asset", "code,runtime", " code", "runtime "] {
        assert!(
            invalid.parse::<ToolProfile>().is_err(),
            "accepted {invalid:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "meridian-mcp-policy-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn rift_build_access_is_disabled_by_default_and_strict_when_configured() {
    let root = fixture();
    let state = fixture();
    let compatible = ServerConfig::from_values_with_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        Some(state.clone()),
    )
    .unwrap();
    assert_eq!(compatible.rift_build_access(), RiftBuildAccess::Disabled);

    let offline = ServerConfig::from_values_with_rift_build_and_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        Some("offline"),
        Some(state.clone()),
    )
    .unwrap();
    assert_eq!(offline.rift_build_access(), RiftBuildAccess::Offline);

    let network = ServerConfig::from_values_with_rift_build_and_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        Some("network"),
        Some(state.clone()),
    )
    .unwrap();
    assert_eq!(network.rift_build_access(), RiftBuildAccess::Network);

    assert!(ServerConfig::from_values_with_rift_build_and_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        Some("internet"),
        Some(state.clone()),
    )
    .is_err());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn analysis_is_the_default_and_development_is_explicit() {
    let root = fixture();
    let state = fixture();
    let analysis = ServerConfig::from_values(None, vec![root.clone()], Vec::new()).unwrap();
    assert_eq!(analysis.mode(), CapabilityMode::Analysis);
    assert!(
        ServerConfig::from_values(Some("development"), vec![root.clone()], Vec::new()).is_err()
    );
    assert!(ServerConfig::from_values_with_state(
        Some("analysis"),
        vec![root.clone()],
        Vec::new(),
        Some(state.clone()),
    )
    .is_err());
    let development = ServerConfig::from_values_with_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        Some(state.clone()),
    )
    .unwrap();
    assert_eq!(development.mode(), CapabilityMode::Development);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn tracy_is_disabled_by_default_and_requires_development_with_a_manifest() {
    let root = fixture();
    let state = fixture();
    let default = ServerConfig::from_values_with_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        Some(state.clone()),
    )
    .unwrap();
    assert_eq!(default.tracy_access(), TracyAccess::Disabled);

    assert!(ServerConfig::from_values_with_features_and_state(
        Some("analysis"),
        vec![root.clone()],
        Vec::new(),
        None,
        Some("byond"),
        Some(root.join("manifest.json")),
        None,
    )
    .is_err());
    assert!(ServerConfig::from_values_with_features_and_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        None,
        Some("byond"),
        None,
        Some(state.clone()),
    )
    .is_err());
    assert!(ServerConfig::from_values_with_features_and_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        None,
        Some("remote"),
        Some(root.join("manifest.json")),
        Some(state.clone()),
    )
    .is_err());
    std::fs::write(root.join("manifest.json"), "{}").unwrap();
    let enabled = ServerConfig::from_values_with_features_and_state(
        Some("development"),
        vec![root.clone()],
        Vec::new(),
        None,
        Some("byond"),
        Some(root.join("manifest.json")),
        Some(state.clone()),
    )
    .unwrap();
    assert_eq!(enabled.tracy_access(), TracyAccess::Byond);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn traversal_and_unlisted_executables_are_rejected() {
    let root = fixture();
    let outside = root.parent().unwrap().join("outside.dm");
    std::fs::write(&outside, "outside").unwrap();
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    assert_eq!(
        policy.read_path(&outside).unwrap_err().code(),
        "path_outside_workspace"
    );
    assert_eq!(
        policy.executable(&outside).unwrap_err().code(),
        "executable_not_allowed"
    );
    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiler_allowlist_exposes_only_canonical_startup_configuration() {
    let root = fixture();
    let compiler = std::env::current_exe().unwrap();
    let policy = PathPolicy::new(vec![root.clone()], vec![compiler.clone()]).unwrap();

    assert_eq!(
        policy.compiler_allowlist(),
        &[compiler.canonicalize().unwrap()]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_failures_report_the_effective_immutable_policy() {
    let first = fixture();
    let second = fixture();
    let outside = first.parent().unwrap().join(format!(
        "meridian-mcp-outside-policy-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&outside, "outside").unwrap();
    let policy = PathPolicy::new(vec![first.clone(), second.clone()], Vec::new()).unwrap();

    let error = policy.read_path(&outside).unwrap_err();
    assert_eq!(error.code(), "path_outside_workspace");
    assert_eq!(error.context().containment_mode, "immutable_startup_roots");
    assert_eq!(
        error.context().policy_source,
        "server_startup_configuration"
    );
    assert_eq!(error.context().effective_roots.len(), 2);

    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(first).unwrap();
    std::fs::remove_dir_all(second).unwrap();
}

#[test]
fn outputs_require_containment_and_explicit_overwrite() {
    let root = fixture();
    let output = root.join("map.png");
    std::fs::write(&output, "existing").unwrap();
    let policy = PathPolicy::new(vec![root.clone()], Vec::new()).unwrap();
    assert_eq!(
        policy.output_path(&output, false).unwrap_err().code(),
        "output_exists"
    );
    assert_eq!(
        policy.output_path(&output, true).unwrap(),
        output.canonicalize().unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn parser_config_loader_rejects_junction_or_symlink_escape() {
    let root = fixture();
    let allowed = root.join("allowed");
    let external = root.join("external");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::create_dir(&external).unwrap();
    std::fs::write(
        external.join("SpacemanDMM.toml"),
        "[display]\nerror_level = \"off\"\n",
    )
    .unwrap();
    let link = allowed.join("linked");
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&external)
        .output()
        .unwrap()
        .status
        .success());
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &link).unwrap();
    let mut context = dreammaker::Context::default();
    context.set_read_policy(std::sync::Arc::new(
        PathPolicy::new(vec![allowed], vec![]).unwrap(),
    ));
    context.force_config(&link.join("SpacemanDMM.toml"));
    assert!(context.read_denied());
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
