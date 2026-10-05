use meridian_mcp::{
    all_contracts, contracts_for, contracts_for_configuration, render_tool_reference,
    CapabilityMode, RiftBuildAccess, ToolProfile,
};
use std::collections::HashSet;

#[test]
fn startup_profile_membership_keeps_shared_and_build_tools_in_their_domains() {
    let registry = all_contracts();
    for (profile, expected_count) in [
        (ToolProfile::All, 62),
        (ToolProfile::Code, 17),
        (ToolProfile::Assets, 13),
        (ToolProfile::Runtime, 38),
    ] {
        assert_eq!(
            registry
                .iter()
                .filter(|contract| contract.profiles.includes(profile))
                .count(),
            expected_count,
            "{profile:?}"
        );
    }
    for (name, expected) in [
        ("dm_server_status", [true, true, true]),
        ("dm_parse_environment", [true, true, true]),
        ("dm_get_proc", [true, false, false]),
        ("dm_check_fixture_sync", [true, false, false]),
        ("dm_generate_docs", [true, false, false]),
        ("dm_audit_icons", [false, true, false]),
        ("dm_render_maps", [false, true, false]),
        ("dm_compile", [true, false, true]),
        ("rift_compile", [true, false, true]),
        ("dm_memory_summary", [false, false, true]),
        ("dm_native_evidence_summary", [false, false, true]),
        ("dm_run", [false, false, true]),
        ("dm_debug_source", [false, false, true]),
        ("dm_tracy_compare", [false, false, true]),
    ] {
        let contract = registry
            .iter()
            .find(|contract| contract.name == name)
            .unwrap();
        let actual = [ToolProfile::Code, ToolProfile::Assets, ToolProfile::Runtime]
            .map(|profile| contract.profiles.includes(profile));
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn contracts_are_unique_bounded_and_analysis_is_read_only() {
    let contracts = all_contracts();
    let names: HashSet<_> = contracts.iter().map(|contract| contract.name).collect();
    assert_eq!(names.len(), contracts.len());
    assert!(contracts
        .iter()
        .all(|contract| !contract.summary.is_empty()));
    assert!(contracts
        .iter()
        .all(|contract| contract.max_output_bytes > 0));
    assert!(contracts_for(CapabilityMode::Analysis)
        .iter()
        .all(|contract| {
            !contract.effects.writes_files
                && !contract.effects.spawns_process
                && !contract.effects.network_loopback
                && !contract.effects.network_external
        }));
    let parse = all_contracts()
        .iter()
        .find(|contract| contract.name == "dm_parse_environment")
        .expect("dm_parse_environment must have a maximum contract");
    assert_eq!(parse.timeout_ms, Some(1_800_000));
}

#[test]
fn rift_compile_contract_respects_mode_access_and_platform() {
    let names = |mode, access| {
        contracts_for_configuration(mode, access)
            .into_iter()
            .map(|contract| contract.name)
            .collect::<HashSet<_>>()
    };

    assert!(!names(CapabilityMode::Analysis, RiftBuildAccess::Network).contains("rift_compile"));
    assert!(
        !names(CapabilityMode::Development, RiftBuildAccess::Disabled).contains("rift_compile")
    );

    #[cfg(windows)]
    {
        assert!(
            names(CapabilityMode::Development, RiftBuildAccess::Offline).contains("rift_compile")
        );
        assert!(
            names(CapabilityMode::Development, RiftBuildAccess::Network).contains("rift_compile")
        );
    }
    #[cfg(not(windows))]
    {
        assert!(
            !names(CapabilityMode::Development, RiftBuildAccess::Offline).contains("rift_compile")
        );
        assert!(
            !names(CapabilityMode::Development, RiftBuildAccess::Network).contains("rift_compile")
        );
    }

    let contract = all_contracts()
        .iter()
        .find(|contract| contract.name == "rift_compile")
        .expect("rift_compile must have a maximum contract");
    assert!(contract.effects.network_external);
    assert_eq!(contract.timeout_ms, Some(1_800_000));
}

#[test]
fn checked_in_reference_matches_contract_registry() {
    let expected = render_tool_reference(all_contracts());
    let actual = std::fs::read_to_string("docs/tool-contracts.md").unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn runtime_control_contracts_report_their_actual_effects() {
    let contract = |name| {
        all_contracts()
            .iter()
            .find(|contract| contract.name == name)
            .copied()
            .expect("runtime control contract should exist")
    };

    let runtime_stop = contract("dm_stop");
    assert!(runtime_stop.effects.destructive);
    assert!(runtime_stop.effects.writes_files);

    let tracy_status = contract("dm_tracy_status");
    assert!(!tracy_status.effects.destructive);
    assert!(tracy_status.effects.network_loopback);

    let tracy_stop = contract("dm_tracy_stop");
    assert!(tracy_stop.effects.destructive);
    assert!(tracy_stop.effects.writes_files);
    assert!(tracy_stop.effects.network_loopback);

    assert_eq!(contract("dm_tracy_launch").timeout_ms, Some(600_000));
}
