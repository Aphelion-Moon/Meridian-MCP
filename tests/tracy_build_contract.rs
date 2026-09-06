use std::path::Path;

#[test]
fn tracy_builder_is_pinned_dual_arch_and_offline_with_owned_native_tests() {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/build-tracy-helpers.ps1");
    let script = std::fs::read_to_string(script_path).expect("Tracy build script should exist");

    for required in [
        "099df3de3dc37eca4712c06b8320fb9c53596edd",
        "d1ec404737b04b1ea73d6df4a1b477deacdb1900",
        "protocol_version = 82",
        "target_arch = 'x86_64'",
        "target_arch = 'x86'",
        "meridian-tracy-helper",
        "prof.dll",
        "libprof.so",
        "-R '^meridian_'",
        "byond-tracy-empty-queue.patch",
        "byond-tracy-health.patch",
        "tracy-clock-access.patch",
        "git apply --check",
        "--directory=$sourcePrefix",
        "patch_sha256",
        "queue_capacity",
        "queue_depth",
        "queue_high_water",
        "queue_tail_refresh_count",
        "queue_saturation_count",
        "queue_dropped_events",
        "produced_events",
        "consumed_events",
        "last_producer_progress",
        "prologue_validated",
        "module_relative_offset",
        "offset_table_identity",
    ] {
        assert!(
            script.contains(required),
            "missing build contract: {required}"
        );
    }
    for forbidden in ["git clone", "Invoke-WebRequest", "curl ", "wget "] {
        assert!(
            !script.contains(forbidden),
            "builder must not fetch sources: {forbidden}"
        );
    }
}

#[test]
fn owned_patch_files_are_forced_to_lf_with_unified_diff_whitespace_rules() {
    let attributes =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(".gitattributes"))
            .expect(".gitattributes should exist");
    let rule = attributes
        .lines()
        .find(|line| line.split_whitespace().next() == Some("*.patch"))
        .expect("owned patches require an attributes rule");
    let fields: Vec<_> = rule.split_whitespace().collect();
    assert!(
        fields.contains(&"text") && fields.contains(&"eol=lf"),
        "owned patches must remain LF"
    );
    let whitespace: Vec<_> = fields
        .iter()
        .find_map(|field| field.strip_prefix("whitespace="))
        .expect("unified-diff context requires explicit whitespace rules")
        .split(',')
        .collect();
    assert!(
        whitespace.contains(&"-space-before-tab"),
        "allow context markers before tab indentation"
    );
    assert!(
        whitespace.contains(&"-blank-at-eol"),
        "allow blank context lines containing one space"
    );
}

#[test]
fn byond_tracy_health_uses_the_protocol_82_plot_event_id() {
    let patch_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("helpers/tracy/byond-tracy-health.patch");
    let patch = std::fs::read_to_string(patch_path).expect("byond-tracy health patch should exist");

    assert!(
        patch.contains("plot_data_int = 55"),
        "Tracy protocol 82 PlotDataInt packets must use queue event id 55"
    );
    assert!(
        patch.contains("utracy.protocol.version != UTRACY_PROTOCOL_0_14_0"),
        "Meridian health plots must be gated to the protocol whose event id is verified"
    );
    assert!(
        patch.contains(".type = utracy.protocol.plot_data_int"),
        "health packets must use the negotiated protocol event id"
    );
    assert!(
        patch.contains("query_plot_name = 4"),
        "Tracy plot-name queries must be explicitly negotiated"
    );
    assert!(
        patch.contains("response_plot_name = 121"),
        "Tracy protocol 82 plot names must use the PlotName response event"
    );
    assert!(
        patch.contains("req.type == utracy.protocol.query_plot_name"),
        "byond-tracy must answer Tracy's dedicated plot-name query"
    );
    assert!(
        patch.contains("utracy.protocol.response_plot_name"),
        "plot-name queries must not be answered as generic strings"
    );
    assert!(
        patch.contains("evt.plot_data.timestamp - utracy.data.cur_thread.timestamp"),
        "plot timestamps must use Tracy's per-thread delta encoding"
    );
    for required in [
        "zone_begin32 = 20",
        "zone_begin16 = 21",
        "zone_end32 = 26",
        "zone_end16 = 27",
        "UTRACY_PROTOCOL_OFFSET_16BIT",
        "UTRACY_PROTOCOL_OFFSET_32BIT",
    ] {
        assert!(
            patch.contains(required),
            "protocol 82 zones must use Tracy's compact delta encoding: {required}"
        );
    }
}
