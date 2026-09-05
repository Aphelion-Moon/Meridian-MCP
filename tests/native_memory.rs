#[path = "../helpers/auxtools-memory/accounting.rs"]
mod accounting;

use meridian_mcp::native_memory::MemoryControl;
use serde_json::json;

#[test]
fn memory_control_rejects_unbounded_and_unknown_arguments() {
    for value in [
        json!({"action":"start","duration_ms":60001}),
        json!({"action":"start","max_records":0}),
        json!({"action":"stop","row_limit":1001}),
        json!({"action":"start","path":"elsewhere"}),
        json!({"action":"exec"}),
    ] {
        assert!(MemoryControl::parse(value).is_err());
    }
    let value = MemoryControl::parse(json!({"action":"start"}))
        .unwrap()
        .command()
        .unwrap();
    assert!(value.starts_with("#meridian_memory_v1 "));
    let request: serde_json::Value =
        serde_json::from_str(value.strip_prefix("#meridian_memory_v1 ").unwrap()).unwrap();
    assert_eq!(request["duration_ms"], 10000);
    assert_eq!(request["max_records"], 20000);
}

#[test]
fn helper_result_requires_the_versioned_success_envelope() {
    use meridian_mcp::native_memory::parse_response;
    assert!(parse_response("Memory profiler enabled").is_err());
    assert!(parse_response(r#"{"protocol_version":2,"evidence":{"ok":true}}"#).is_err());
    assert!(parse_response(
        r#"{"protocol_version":1,"evidence":{"ok":false,"error":"unsupported allocator"}}"#
    )
    .unwrap_err()
    .to_string()
    .contains("unsupported allocator"));
    assert!(parse_response(
        r#"{"protocol_version":1,"evidence":{"ok":true,"result":{"recording":true}}}"#
    )
    .is_ok());
}

#[cfg(windows)]
#[test]
fn helper_requires_exact_overlay_and_artifact_hashes() {
    use sha2::{Digest, Sha256};
    let root = std::env::temp_dir().join(format!(
        "meridian-native-manifest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let manifest = root.join("manifest.json");
    std::fs::write(root.join("debug_server.dll"), b"fixture-only").unwrap();
    let patches: Vec<_> = ["accounting.rs", "mem_profiler.rs", "protocol.patch"]
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(format!("helpers/auxtools-memory/{name}")).unwrap();
            json!({"name":name,"patch_sha256":format!("{:x}",Sha256::digest(bytes))})
        })
        .collect();
    let mut value = json!({"schema_version":2,"helpers":[{
        "id":"auxtools-memory","platform":"windows","target_arch":"x86","path":"debug_server.dll",
        "sha256":format!("{:x}",Sha256::digest(b"fixture-only")),"source_revision":meridian_mcp::native_memory::SOURCE_REVISION,
        "protocol_version":1,"byond_min_version":"516.1687","byond_max_version":"516.1687","patches":patches
    }]});
    std::fs::write(&manifest, value.to_string()).unwrap();
    assert!(meridian_mcp::native_memory::verified_memory_helper(&manifest).is_ok());
    value["helpers"][0]["patches"][0]["patch_sha256"] = json!("0".repeat(64));
    std::fs::write(&manifest, value.to_string()).unwrap();
    assert!(
        meridian_mcp::native_memory::verified_memory_helper(&manifest)
            .unwrap_err()
            .to_string()
            .contains("overlay")
    );
    std::fs::write(root.join("debug_server.dll"), b"changed").unwrap();
    assert!(
        meridian_mcp::native_memory::verified_memory_helper(&manifest)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
}
