use meridian_mcp::native_evidence::model::{EvidenceRecord, PhaseInput};
use meridian_mcp::native_evidence::{redaction, timeline};
use std::collections::BTreeMap;

#[test]
fn phases_are_half_open_and_protected_text_is_redacted() {
    let phases = vec![PhaseInput {
        id: "game_start".into(),
        wall_start: Some("2026-01-01T00:00:00Z".into()),
        wall_end: Some("2026-01-01T00:00:01Z".into()),
        world_start_ds: None,
        world_end_ds: None,
    }];
    timeline::validate_phases(&phases).unwrap();
    let record = EvidenceRecord {
        wall_unix_ms: Some(1_767_225_600_000),
        world_deciseconds: None,
        sample_index: 0,
        metrics: BTreeMap::new(),
        groups: BTreeMap::new(),
    };
    assert_eq!(
        timeline::assign(&record, &phases).unwrap().as_deref(),
        Some("game_start")
    );
    let (text, count) = redaction::sanitize_text("ckey=example_player Discord-ID=private-discord PLAYER-ID=private-player my_player-id=visible monkey=visible");
    assert_eq!(count, 3);
    assert!(!text.contains("example_player"));
    assert!(!text.contains("private-discord"));
    assert!(!text.contains("private-player"));
    assert!(text.contains("my_player-id=visible monkey=visible"));
}
