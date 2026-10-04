use meridian_mcp::capabilities::{BYOND_TRACY_REVISION, TRACY_PROTOCOL_VERSION, TRACY_REVISION};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::tracy::TracyInstallation;
use meridian_mcp::{CapabilityMode, PathPolicy, RiftBuildAccess};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn initialize_owner() {
    #[cfg(windows)]
    meridian_mcp::process::initialize_runtime_owner().unwrap();
    #[cfg(unix)]
    meridian_mcp::process::initialize_runtime_owner_with_executable(std::path::Path::new(env!(
        "CARGO_BIN_EXE_meridian-mcp"
    )))
    .unwrap();
}

async fn owned_collector(
    mode: &str,
) -> (
    std::path::PathBuf,
    std::sync::Arc<meridian_mcp::tracy_collector::TracyCollector>,
) {
    initialize_owner();
    use meridian_mcp::tracy_collector::{TracyCollector, TracyCollectorSpec};
    let root = std::env::temp_dir().join(format!(
        "meridian-collector-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let helper = root.join(format!("collector{}", std::env::consts::EXE_SUFFIX));
    assert!(std::process::Command::new("rustup")
        .args([
            "run",
            "1.95.0",
            "rustc",
            "--edition=2021",
            "tests/fixtures/tracy/blocked_collector.rs",
            "-o"
        ])
        .arg(&helper)
        .status()
        .unwrap()
        .success());
    let collector = TracyCollector::spawn(TracyCollectorSpec {
        helper,
        working_directory: root.clone(),
        environment: vec![("COLLECTOR_MODE".into(), mode.into())],
        request_timeout: std::time::Duration::from_secs(5),
    })
    .await
    .unwrap();
    (root, std::sync::Arc::new(collector))
}

#[tokio::test]
async fn collector_stop_closes_stdin_after_session_stop_response() {
    let (root, collector) = owned_collector("respond").await;
    collector
        .stop(std::time::Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!collector.is_running().await);
    assert_eq!(collector.exit_code().await, Some(37));
    drop(collector);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancelled_stop_still_terminates_the_owned_child() {
    use std::time::Duration;
    let (root, collector) = owned_collector("blocked").await;
    let stopping = tokio::spawn({
        let collector = collector.clone();
        async move { collector.stop(Duration::from_millis(100)).await }
    });
    tokio::task::yield_now().await;
    stopping.abort();
    let _ = stopping.await;
    tokio::time::timeout(Duration::from_millis(500), async {
        while collector.is_running().await {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("armed stop survives caller cancellation");
    drop(collector);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn collector_stderr_is_utf8_safe_and_keeps_a_bounded_tail() {
    use std::time::Duration;
    let (root, collector) = owned_collector("stderr").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let tail = collector.stderr_tail().await;
            assert!(tail.len() <= 64);
            assert!(tail
                .iter()
                .all(|line| line.len() <= 4096 + "... [truncated]".len()));
            if tail.last().is_some_and(|line| line == "line69") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("stderr reader survived split UTF-8 and oversized line");
    let _ = collector.stop(Duration::from_millis(100)).await;
    assert!(!collector.is_running().await);
    drop(collector);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn collector_stop_terminates_owned_child_with_blocked_protocol_writer() {
    initialize_owner();
    use meridian_mcp::tracy_collector::{TracyCollector, TracyCollectorSpec};
    use std::time::Duration;
    let root = std::env::temp_dir().join(format!("meridian-collector-stop-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let helper = root.join(format!("blocked-collector{}", std::env::consts::EXE_SUFFIX));
    assert!(std::process::Command::new("rustup")
        .args([
            "run",
            "1.95.0",
            "rustc",
            "--edition=2021",
            "tests/fixtures/tracy/blocked_collector.rs",
            "-o"
        ])
        .arg(&helper)
        .status()
        .unwrap()
        .success());
    let collector = std::sync::Arc::new(
        TracyCollector::spawn(TracyCollectorSpec {
            helper,
            working_directory: root.clone(),
            environment: vec![],
            request_timeout: Duration::from_secs(30),
        })
        .await
        .unwrap(),
    );
    let mut captures = Vec::new();
    for _ in 0..8 {
        let collector = collector.clone();
        captures.push(tokio::spawn(async move {
            collector
                .capture_window(
                    1,
                    64,
                    std::path::Path::new(&"x".repeat(32768)),
                    "fixture",
                    1,
                )
                .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        collector.stop(Duration::from_millis(50)),
    )
    .await;
    for capture in captures {
        capture.abort();
        let _ = capture.await;
    }
    assert!(
        result.is_ok(),
        "stop must not await the blocked protocol writer"
    );
    if !collector.cleanup_confirmed() {
        assert!(
            result.as_ref().unwrap().is_err(),
            "unconfirmed cleanup cannot report success"
        );
    }
    let exited = tokio::time::timeout(Duration::from_millis(500), async {
        while collector.is_running().await {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    drop(collector);
    assert!(
        exited.is_ok(),
        "owned child must exit after the independent kill request"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> (std::path::PathBuf, ToolExecutionContext) {
    let root = std::env::temp_dir().join(format!(
        "meridian-tracy-tools-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("helpers")).unwrap();
    let helper = root.join("helpers/meridian-tracy-helper.exe");
    let hook = root.join("helpers/prof.dll");
    std::fs::write(&helper, b"helper").unwrap();
    std::fs::write(&hook, b"verified hook").unwrap();
    let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let manifest = root.join("manifest.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({"schema_version":2,"helpers":[
            {"id":"tracy-server-helper","platform":std::env::consts::OS,"target_arch":std::env::consts::ARCH,"path":"helpers/meridian-tracy-helper.exe","sha256":hash(b"helper"),"source_revision":TRACY_REVISION,"protocol_version":TRACY_PROTOCOL_VERSION},
            {"id":"byond-tracy","platform":std::env::consts::OS,"target_arch":"x86","path":"helpers/prof.dll","sha256":hash(b"verified hook"),"source_revision":BYOND_TRACY_REVISION,"protocol_version":TRACY_PROTOCOL_VERSION,"byond_min_version":"516.1685","byond_max_version":"516.1687"}
        ]})).unwrap(),
    ).unwrap();
    let installation = TracyInstallation::validate(&manifest).unwrap();
    let context = ToolExecutionContext::with_features(
        CapabilityMode::Development,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
        RiftBuildAccess::Disabled,
        None,
        None,
        Some(installation),
    );
    (root, context)
}

#[test]
fn actual_byond_version_is_checked_against_the_verified_hook_range() {
    let (root, _) = fixture();
    let installation = TracyInstallation::validate(&root.join("manifest.json")).unwrap();

    assert_eq!(
        installation.validate_byond_version("516.1685").unwrap(),
        "516.1685"
    );
    assert_eq!(
        installation.validate_byond_version("1685").unwrap(),
        "516.1685"
    );
    assert_eq!(
        installation.validate_byond_version("516.1687").unwrap(),
        "516.1687"
    );
    assert!(installation.validate_byond_version("516.1684").is_err());
    assert!(installation.validate_byond_version("516.1688").is_err());
    assert!(installation.validate_byond_version("unknown").is_err());
}

#[tokio::test]
async fn prepare_is_hash_verified_atomic_and_idempotent() {
    let (root, context) = fixture();
    let dmb = root.join("game.dmb");
    std::fs::write(&dmb, b"dmb").unwrap();

    let first = call_tool(
        &context,
        &ServerState::new(),
        "dm_tracy_prepare",
        serde_json::json!({"dmb_path":dmb}),
    )
    .await
    .unwrap();
    assert_ne!(first.is_error, Some(true));
    assert_eq!(
        std::fs::read(root.join("prof.dll")).unwrap(),
        b"verified hook"
    );

    let second = call_tool(
        &context,
        &ServerState::new(),
        "dm_tracy_prepare",
        serde_json::json!({"dmb_path":dmb}),
    )
    .await
    .unwrap();
    assert_ne!(second.is_error, Some(true));

    std::fs::write(root.join("prof.dll"), b"different").unwrap();
    assert!(call_tool(
        &context,
        &ServerState::new(),
        "dm_tracy_prepare",
        serde_json::json!({"dmb_path":dmb}),
    )
    .await
    .is_err());

    call_tool(
        &context,
        &ServerState::new(),
        "dm_tracy_prepare",
        serde_json::json!({"dmb_path":dmb,"overwrite":true}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(root.join("prof.dll")).unwrap(),
        b"verified hook"
    );
    std::fs::remove_dir_all(root).unwrap();
}
