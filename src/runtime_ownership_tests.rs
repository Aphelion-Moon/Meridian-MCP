use crate::process_metrics::{process_identity, ProcessIdentity, ProcessRole};
use crate::state::RuntimeState;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn initialize_owner() {
    #[cfg(windows)]
    crate::process::initialize_runtime_owner().unwrap();
    #[cfg(unix)]
    crate::process::initialize_runtime_owner_with_executable(
        &std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("meridian-mcp"),
    )
    .unwrap();
}

struct FixtureChild(std::process::Child);

impl std::ops::Deref for FixtureChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for FixtureChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixture_provenance() -> crate::LaunchProvenance {
    crate::LaunchProvenance {
        status: crate::ProvenanceStatus::Unverified,
        build_record_id: None,
        dmb_sha256: "00".repeat(32),
        warnings: Vec::new(),
    }
}

#[tokio::test]
async fn output_wait_keeps_status_and_stop_responsive() {
    use crate::tools::runtime;
    use serde_json::json;
    initialize_owner();
    let state = crate::state::ServerState::new();
    let (child, containment) = crate::process::spawn_runtime_process(
        &mut tokio::process::Command::from(fixture_command("leaf", std::path::Path::new("unused"))),
    )
    .unwrap();
    {
        let mut runtime = state.runtime().await;
        runtime.set_game_process(child, 1337, fixture_provenance());
        runtime.containment = Some(containment);
    }
    let mut wait = Box::pin(runtime::wait_for_output(
        &state,
        json!({"pattern":"MISSING", "timeout_ms":300000}),
    ));
    tokio::select! {
        result = &mut wait => panic!("wait finished before stop: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    tokio::time::timeout(Duration::from_secs(1), runtime::status(&state, json!({})))
        .await
        .expect("status blocked by output wait")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), runtime::stop(&state, json!({})))
        .await
        .expect("stop blocked by output wait")
        .unwrap();
    let original_exit = state.runtime().await.last_exit_code;
    let (child, containment) = crate::process::spawn_runtime_process(
        &mut tokio::process::Command::from(fixture_command("leaf", std::path::Path::new("unused"))),
    )
    .unwrap();
    {
        let mut runtime = state.runtime().await;
        runtime.clear_runtime_diagnostics();
        runtime.set_game_process(child, 1337, fixture_provenance());
        runtime.containment = Some(containment);
        crate::state::push_output_line(
            &runtime.output_log,
            "MISSING from the replacement".to_owned(),
        );
    }
    let result = tokio::time::timeout(Duration::from_secs(1), wait)
        .await
        .expect("wait did not observe stop")
        .unwrap();
    let crate::mcp::ToolContent::Text { text } = &result.content[0];
    let result: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(result["process_exited"], true);
    assert_eq!(result["matched"], false);
    assert_eq!(result["last_exit_code"], json!(original_exit));
    assert_eq!(result["recent_output"], json!([]));
    runtime::stop(&state, json!({})).await.unwrap();
}

#[tokio::test]
async fn launch_readiness_is_stoppable_and_cancellation_releases_ownership() {
    use crate::tools::runtime;
    use serde_json::json;
    initialize_owner();
    let directory =
        std::env::temp_dir().join(format!("meridian-task6-launch-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    assert!(Command::new("rustc")
        .args(["+1.95.0", "--edition=2021"])
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/process/runtime_tree.rs")
        )
        .arg("-o")
        .arg(directory.join("dreamdaemon.exe"))
        .status()
        .unwrap()
        .success());
    std::fs::copy(directory.join("dreamdaemon.exe"), directory.join("dm.exe")).unwrap();
    std::fs::write(directory.join("fixture.dmb"), "fixture").unwrap();
    let context = crate::tools::ToolExecutionContext::new(
        crate::CapabilityMode::Development,
        crate::PathPolicy::new(vec![directory.clone()], vec![directory.join("dm.exe")]).unwrap(),
    );
    for cancel in [false, true] {
        let state = crate::state::ServerState::new();
        let marker = directory.join(format!("{cancel}.pids"));
        let mut launch = Box::pin(runtime::run(
            &context,
            &state,
            json!({
                "dmb_path":directory.join("fixture.dmb"), "daemon_args":["--marker", marker],
                "wait_for":"NEVER_READY", "startup_timeout_ms":300000
            }),
        ));
        let observe = async {
            tokio::time::timeout(Duration::from_secs(3), async {
                while !marker.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(350)).await;
        };
        tokio::select! {
            result = &mut launch => panic!("launch ended before readiness control: {result:?}"),
            () = observe => {}
        }
        let identities: Vec<_> = std::fs::read_to_string(&marker)
            .unwrap()
            .split_whitespace()
            .map(|pid| process_identity(pid.parse().unwrap(), ProcessRole::DreamDaemon).unwrap())
            .collect();
        tokio::time::timeout(Duration::from_secs(1), runtime::status(&state, json!({})))
            .await
            .expect("launch readiness blocked status")
            .unwrap();
        if cancel {
            drop(launch);
        } else {
            tokio::time::timeout(Duration::from_secs(1), runtime::stop(&state, json!({})))
                .await
                .expect("launch readiness blocked stop")
                .unwrap();
            let replacement = runtime::run(
                &context,
                &state,
                json!({
                    "dmb_path":directory.join("fixture.dmb"),
                    "daemon_args":["--marker", directory.join("replacement.pids")],
                    "wait_for":"RUNTIME_TREE_READY"
                }),
            )
            .await
            .unwrap();
            assert_ne!(replacement.is_error, Some(true));
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), launch)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_error,
                Some(true)
            );
            assert!(
                state.runtime().await.is_game_running(),
                "old launch stopped its replacement"
            );
            runtime::stop(&state, json!({})).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while identities
                .iter()
                .any(|identity| owned_process_liveness(identity, 0).is_ok())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("readiness cancellation left an owned process alive");
        drop(state);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn runtime_observer_does_not_retain_owner() {
    initialize_owner();
    let state = crate::state::ServerState::new();
    let (child, containment) = crate::process::spawn_runtime_process(
        &mut tokio::process::Command::from(fixture_command("leaf", std::path::Path::new("unused"))),
    )
    .unwrap();
    let identity = process_identity(child.id().unwrap(), ProcessRole::DreamDaemon).unwrap();
    let observation = {
        let mut runtime = state.runtime().await;
        runtime.set_game_process(child, 1337, fixture_provenance());
        runtime.containment = Some(containment);
        state.observe_runtime(&mut runtime);
        runtime.output_log.clone()
    };
    tokio::time::sleep(Duration::from_millis(75)).await;
    drop(state);
    assert!(!observation.lock().unwrap().running);
    tokio::time::timeout(Duration::from_secs(1), async {
        while owned_process_liveness(&identity, 0).is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("observer retained runtime ownership after state drop");
}

#[tokio::test]
async fn invalid_readiness_regex_never_starts_the_runtime() {
    initialize_owner();
    let directory =
        std::env::temp_dir().join(format!("meridian-invalid-readiness-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    assert!(Command::new("rustc")
        .args(["+1.95.0", "--edition=2021"])
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/process/runtime_tree.rs")
        )
        .arg("-o")
        .arg(directory.join("dreamdaemon.exe"))
        .status()
        .unwrap()
        .success());
    std::fs::copy(directory.join("dreamdaemon.exe"), directory.join("dm.exe")).unwrap();
    std::fs::write(directory.join("fixture.dmb"), "fixture").unwrap();
    let context = crate::tools::ToolExecutionContext::new(
        crate::CapabilityMode::Development,
        crate::PathPolicy::new(vec![directory.clone()], vec![directory.join("dm.exe")]).unwrap(),
    );
    let state = crate::state::ServerState::new();
    let output_before = state.runtime().await.output_log.clone();
    let marker = directory.join("started.pids");
    let result = crate::tools::call_tool(
        &context,
        &state,
        "dm_run",
        serde_json::json!({
            "dmb_path": directory.join("fixture.dmb"),
            "daemon_args": ["--marker", marker],
            "wait_for": "[",
            "wait_regex": true
        }),
    )
    .await;
    let launched = !std::sync::Arc::ptr_eq(&output_before, &state.runtime().await.output_log);
    let marker_written = marker.exists();
    crate::tools::runtime::stop(&state, serde_json::json!({}))
        .await
        .unwrap();
    let unverified = crate::tools::call_tool(
        &context,
        &state,
        "dm_run",
        serde_json::json!({
            "dmb_path": directory.join("fixture.dmb"),
            "daemon_args": ["--marker", directory.join("unverified.pids")],
            "require_verified_provenance": true
        }),
    )
    .await
    .unwrap();
    let unverified_started = directory.join("unverified.pids").exists();
    let valid = crate::tools::call_tool(
        &context,
        &state,
        "dm_run",
        serde_json::json!({
            "dmb_path": directory.join("fixture.dmb"),
            "daemon_args": ["--marker", directory.join("valid.pids")],
            "wait_for": "^RUNTIME_TREE_READY$",
            "wait_regex": true,
            "startup_timeout_ms": 3000,
            "require_verified_provenance": false
        }),
    )
    .await
    .unwrap();
    let valid_started = directory.join("valid.pids").exists();
    crate::tools::runtime::stop(&state, serde_json::json!({}))
        .await
        .unwrap();
    drop(state);
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        !launched && !marker_written,
        "invalid readiness regex launched a runtime: session_replaced={launched}, marker_written={marker_written}"
    );
    let result = result.unwrap();
    assert_eq!(result.is_error, Some(true));
    let crate::mcp::ToolContent::Text { text } = &result.content[0];
    assert!(text.contains("invalid_input"), "{text}");
    assert_eq!(unverified.is_error, Some(true));
    assert!(!unverified_started);
    assert_ne!(valid.is_error, Some(true));
    assert!(valid_started);
    let crate::mcp::ToolContent::Text { text } = &valid.content[0];
    let valid: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(valid["readiness"]["matched"], true);
}

#[cfg(windows)]
fn owned_process_liveness(identity: &ProcessIdentity, _offset: u64) -> Result<(), ()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{FILETIME, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE,
    };
    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            identity.pid,
        );
        if handle.is_null() {
            return Err(());
        }
        let handle = OwnedHandle::from_raw_handle(handle);
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if GetProcessTimes(
            handle.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return Err(());
        }
        let started = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        if started == identity.started_at_identity
            && WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_TIMEOUT
        {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(target_os = "linux")]
fn owned_process_liveness(identity: &ProcessIdentity, _offset: u64) -> Result<(), ()> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", identity.pid)).map_err(|_| ())?;
    let fields: Vec<_> = stat
        .rsplit_once(") ")
        .ok_or(())?
        .1
        .split_whitespace()
        .collect();
    if fields.get(19).and_then(|value| value.parse::<u64>().ok())
        == Some(identity.started_at_identity)
        && !matches!(fields.first(), Some(&"Z" | &"X"))
    {
        Ok(())
    } else {
        Err(())
    }
}

fn fixture_command(mode: &str, marker: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "server::runtime_ownership_tests::fixture",
            "--nocapture",
        ])
        .env("MERIDIAN_OWNERSHIP_MODE", mode)
        .env("MERIDIAN_OWNERSHIP_MARKER", marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

/// Keep startup evidence outside libtest's captured output, including when the
/// owner is killed before it can publish the process identities.
struct FixtureTrace {
    path: std::path::PathBuf,
    started: Instant,
}

impl FixtureTrace {
    fn new(marker: &std::path::Path) -> Self {
        Self {
            path: marker.with_extension("phase"),
            started: Instant::now(),
        }
    }

    fn record(&self, phase: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .unwrap();
        writeln!(file, "{}ms {phase}", self.started.elapsed().as_millis()).unwrap();
    }
}

fn fixture_evidence(owner: &mut std::process::Child, marker: &std::path::Path) -> String {
    format!(
        "owner_pid={} exit={:?} marker={} pids={} phases:\n{}",
        owner.id(),
        owner.try_wait(),
        marker.exists(),
        marker.with_extension("pids").exists(),
        std::fs::read_to_string(marker.with_extension("phase"))
            .unwrap_or_else(|error| format!("unavailable: {error}")),
    )
}

#[test]
#[ignore]
fn fixture() {
    let Ok(mode) = std::env::var("MERIDIAN_OWNERSHIP_MODE") else {
        return;
    };
    let marker = std::path::PathBuf::from(std::env::var_os("MERIDIAN_OWNERSHIP_MARKER").unwrap());
    if mode == "leaf" {
        std::thread::sleep(Duration::from_secs(20));
        return;
    }
    if mode == "child" || mode == "exit_child" {
        let mut leaf = fixture_command("leaf", &marker).spawn().unwrap();
        let identities = [
            process_identity(std::process::id(), ProcessRole::DreamDaemon).unwrap(),
            process_identity(leaf.id(), ProcessRole::DreamDaemon).unwrap(),
        ];
        std::fs::write(&marker, serde_json::to_vec(&identities).unwrap()).unwrap();
        if mode == "exit_child" {
            std::process::exit(7);
        }
        let _ = leaf.wait();
        return;
    }
    let trace = FixtureTrace::new(&marker);
    trace.record("owner entered; creating executor");
    let executor = tokio::runtime::Runtime::new().unwrap();
    trace.record("executor created; initializing owner");
    initialize_owner();
    trace.record("owner initialized");
    if mode == "cancel" || mode == "abrupt" {
        executor.block_on(async {
            let directory =
                std::path::PathBuf::from(std::env::var_os("MERIDIAN_FAKE_RUNTIME").unwrap());
            let context = crate::tools::ToolExecutionContext::new(
                crate::CapabilityMode::Development,
                crate::PathPolicy::new(vec![directory.clone()], vec![directory.join("dm.exe")])
                    .unwrap(),
            );
            let state = crate::state::ServerState::new();
            let pids = marker.with_extension("pids");
            let mut launch = Box::pin(async {
                trace.record("runtime launch polled");
                crate::tools::runtime::run(
                    &context,
                    &state,
                    serde_json::json!({
                        "dmb_path": directory.join("fixture.dmb"),
                        "daemon_args": ["--marker", pids],
                        "wait_for": "NEVER_READY",
                        "startup_timeout_ms": 300000
                    }),
                )
                .await
            });
            let observe = async {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !pids.exists() {
                    assert!(
                        Instant::now() < deadline,
                        "runtime PID marker not published"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                trace.record("runtime PID marker observed");
                let identities: Vec<_> = std::fs::read_to_string(&pids)
                    .unwrap()
                    .split_whitespace()
                    .map(|pid| {
                        process_identity(pid.parse().unwrap(), ProcessRole::DreamDaemon).unwrap()
                    })
                    .collect();
                std::fs::write(&marker, serde_json::to_vec(&identities).unwrap()).unwrap();
                trace.record("runtime identities published");
                identities
            };
            let identities = tokio::select! {
                outcome = &mut launch => panic!("launch unexpectedly finished: {outcome:?}"),
                identities = observe => identities,
            };
            if mode == "abrupt" {
                trace.record("waiting for abrupt owner termination");
                std::future::pending::<()>().await;
            }
            trace.record("cancelling launch");
            drop(launch);
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline
                && identities
                    .iter()
                    .any(|identity| owned_process_liveness(identity, 0).is_ok())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                identities
                    .iter()
                    .all(|identity| owned_process_liveness(identity, 0).is_err()),
                "cancelled launch retained its tree"
            );
            assert!(!state.runtime().await.is_game_running());
            trace.record("cancelled runtime tree terminated");
        });
        return;
    }
    if mode == "no_executor" {
        let state = executor.block_on(async {
            let (child, containment) = crate::process::spawn_runtime_process(
                &mut tokio::process::Command::from(fixture_command("child", &marker)),
            )
            .unwrap();
            let mut state = RuntimeState::default();
            state.set_game_process(child, 1337, fixture_provenance());
            state.containment = Some(containment);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !marker.exists() {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            state
        });
        drop(executor);
        drop(state);
        let identities: Vec<ProcessIdentity> =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline
            && identities
                .iter()
                .any(|identity| owned_process_liveness(identity, 0).is_ok())
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(identities
            .iter()
            .all(|identity| owned_process_liveness(identity, 0).is_err()));
        return;
    }
    executor.block_on(async {
        let (child, containment) = crate::process::spawn_runtime_process(&mut tokio::process::Command::from(fixture_command("child", &marker))).unwrap();
        let mut state = RuntimeState::default();
        state.set_game_process(child, 1337, fixture_provenance());
        state.containment = Some(containment);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() { assert!(Instant::now() < deadline); tokio::time::sleep(Duration::from_millis(10)).await; }
        if mode == "eof" || mode == "transport_error" {
            use tokio::io::AsyncWriteExt;
            let integrity_root = std::env::temp_dir().join(format!("meridian-eof-integrity-{}", std::process::id()));
            let workspace = integrity_root.join("workspace");
            let journal_root = integrity_root.join("state");
            std::fs::create_dir_all(&workspace).unwrap();
            std::fs::create_dir_all(&journal_root).unwrap();
            std::fs::write(workspace.join("tracked.dm"), "fixture").unwrap();
            let server = super::MeridianServer::new(crate::ServerConfig::from_values_with_state(Some("development"), vec![workspace.clone()], Vec::new(), Some(journal_root)).unwrap()).unwrap();
            let session = crate::runtime_integrity::RuntimeIntegritySession::create(
                server.execution.private_state_arc().unwrap(), &workspace,
                fixture_provenance(),
                state.output_log.clone(), Vec::new()).unwrap();
            state.integrity = Some(std::sync::Arc::new(tokio::sync::Mutex::new(session)));
            *server.state.runtime().await = state;
            let observer = server.clone();
            let (client, transport) = tokio::io::duplex(8192);
            let task = tokio::spawn(crate::mcp::run_transport(server, tokio::io::split(transport)));
            let (reader, mut writer) = tokio::io::split(client);
            if mode == "transport_error" {
                writer.write_all(b"malformed transport\n").await.unwrap();
                writer.shutdown().await.unwrap();
                assert!(task.await.unwrap().is_err());
            } else {
            writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n").await.unwrap();
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(reader);
            let mut response = String::new();
            reader.read_line(&mut response).await.unwrap();
            writer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
            writer.shutdown().await.unwrap();
            task.await.unwrap().unwrap();
            }
            assert!(!observer.state.runtime().await.is_game_running(), "EOF left owned runtime running");
            assert_eq!(observer.state.runtime().await.integrity_summary.as_ref().unwrap().status, crate::runtime_integrity::RuntimeIntegrityStatus::FinalizedClean);
            drop(observer);
            std::fs::remove_dir_all(integrity_root).unwrap();
        } else {
            if mode == "stop" { state.stop_game_process().await.unwrap(); }
            drop(state);
        }
        let identities: Vec<ProcessIdentity> = serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && identities.iter().any(|identity| owned_process_liveness(identity, 0).is_ok()) { tokio::time::sleep(Duration::from_millis(10)).await; }
        assert!(identities.iter().all(|identity| owned_process_liveness(identity, 0).is_err()), "tree survived while owner remained alive");
    });
}

#[tokio::test]
async fn natural_exit_cleans_descendants_with_retained_startup_ownership() {
    initialize_owner();
    for background in [false, true] {
        let marker = std::env::temp_dir().join(format!(
            "meridian-task6-natural-{}-{background}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let state = crate::state::ServerState::new();
        let (child, containment) = crate::process::spawn_runtime_process(
            &mut tokio::process::Command::from(fixture_command("exit_child", &marker)),
        )
        .unwrap();
        let pid = child.id();
        // Retain the startup guard's extra reference through the exit assertion.
        let startup_ownership = containment.clone();
        let output = {
            let mut runtime = state.runtime().await;
            runtime.set_game_process(child, 1337, fixture_provenance());
            runtime.containment = Some(containment);
            if background {
                state.observe_runtime(&mut runtime);
            }
            runtime.output_log.clone()
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            loop {
                let running = if background {
                    output.lock().unwrap().running
                } else {
                    state.runtime().await.is_game_running()
                };
                if !running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let identities: Vec<ProcessIdentity> =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        assert_eq!(Some(identities[0].pid), pid);
        assert_eq!(output.lock().unwrap().last_exit_code, Some(7));
        state
            .runtime()
            .await
            .finish_runtime_cleanup()
            .await
            .unwrap();
        let boundary_pending: Vec<_> = identities
            .iter()
            .filter(|identity| owned_process_liveness(identity, 0).is_ok())
            .cloned()
            .collect();
        if !boundary_pending.is_empty() {
            let started = Instant::now();
            while boundary_pending
                .iter()
                .any(|identity| owned_process_liveness(identity, 0).is_ok())
                && started.elapsed() < Duration::from_millis(100)
            {
                std::thread::yield_now();
            }
            eprintln!("boundary unsignaled identities={boundary_pending:?}; signaled_after={:?}; root={pid:?}", started.elapsed());
        }
        assert!(boundary_pending.is_empty(), "descendant was still running at the integrity finalization boundary: background={background}, identities={identities:?}");
        drop(startup_ownership);
        drop(state);
        std::fs::remove_file(marker).unwrap();
    }
}

#[tokio::test]
async fn failed_cleanup_keeps_integrity_active_and_retryable() {
    initialize_owner();
    for fault in [1, 2, 3] {
        let root = std::env::temp_dir().join(format!(
            "meridian-cleanup-failure-{}-{fault}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        std::fs::write(workspace.join("tracked.dm"), "fixture").unwrap();
        let server = super::MeridianServer::new(
            crate::ServerConfig::from_values_with_state(
                Some("development"),
                vec![workspace.clone()],
                Vec::new(),
                Some(root.join("state")),
            )
            .unwrap(),
        )
        .unwrap();
        let marker = root.join("tree.json");
        let (child, containment) = crate::process::spawn_runtime_process(
            &mut tokio::process::Command::from(fixture_command("exit_child", &marker)),
        )
        .unwrap();
        let retained_startup_owner = containment.clone();
        containment.inject_cleanup_fault(fault);
        {
            let mut runtime = server.state.runtime().await;
            runtime.set_game_process(child, 1337, fixture_provenance());
            runtime.containment = Some(containment);
            let session = crate::runtime_integrity::RuntimeIntegritySession::create(
                server.execution.private_state_arc().unwrap(),
                &workspace,
                fixture_provenance(),
                runtime.output_log.clone(),
                Vec::new(),
            )
            .unwrap();
            runtime.integrity = Some(std::sync::Arc::new(tokio::sync::Mutex::new(session)));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while server.state.runtime().await.is_game_running() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            crate::tools::runtime::status(&server.state, serde_json::json!({}))
                .await
                .is_err()
        );
        {
            let runtime = server.state.runtime().await;
            assert!(runtime.containment.is_some());
            assert!(runtime.integrity.is_some());
            assert!(runtime.integrity_summary.is_none());
        }
        retained_startup_owner.inject_cleanup_fault(0);
        crate::tools::runtime::status(&server.state, serde_json::json!({}))
            .await
            .unwrap();
        let identities: Vec<ProcessIdentity> =
            serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
        assert!(identities
            .iter()
            .all(|identity| owned_process_liveness(identity, 0).is_err()));
        assert_eq!(
            server
                .state
                .runtime()
                .await
                .integrity_summary
                .as_ref()
                .unwrap()
                .status,
            crate::runtime_integrity::RuntimeIntegrityStatus::FinalizedClean
        );
        drop(server);
        drop(retained_startup_owner);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn owned_runtime_lifecycle_keeps_unrelated_sentinel_alive() {
    let directory =
        std::env::temp_dir().join(format!("meridian-fake-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    assert!(Command::new("rustc")
        .args(["+1.95.0", "--edition=2021"])
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/process/runtime_tree.rs")
        )
        .arg("-o")
        .arg(directory.join("dreamdaemon.exe"))
        .status()
        .unwrap()
        .success());
    std::fs::copy(directory.join("dreamdaemon.exe"), directory.join("dm.exe")).unwrap();
    std::fs::write(directory.join("fixture.dmb"), "fixture").unwrap();
    for mode in [
        "stop",
        "drop",
        "eof",
        "transport_error",
        "no_executor",
        "cancel",
        "abrupt",
    ] {
        let marker = std::env::temp_dir().join(format!(
            "meridian-owned-tree-{}-{mode}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::remove_file(marker.with_extension("pids"));
        let _ = std::fs::remove_file(marker.with_extension("phase"));
        let mut sentinel = FixtureChild(fixture_command("leaf", &marker).spawn().unwrap());
        let mut owner = FixtureChild(
            fixture_command(mode, &marker)
                .env("MERIDIAN_FAKE_RUNTIME", &directory)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        if mode == "abrupt" {
            while !marker.exists() {
                assert!(
                    owner.try_wait().unwrap().is_none(),
                    "{mode}: owner exited before startup marker: {}",
                    fixture_evidence(&mut owner, &marker)
                );
                assert!(
                    Instant::now() < deadline,
                    "{mode}: startup marker timed out: {}",
                    fixture_evidence(&mut owner, &marker)
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            owner.kill().unwrap();
        }
        while owner.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "{mode}: owner exit timed out: {}",
                fixture_evidence(&mut owner, &marker)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            owner.wait().unwrap().success() || mode == "abrupt",
            "owner fixture {mode} failed: {}",
            fixture_evidence(&mut owner, &marker)
        );
        let identities: Vec<ProcessIdentity> =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        while Instant::now() < deadline
            && identities
                .iter()
                .any(|identity| owned_process_liveness(identity, 0).is_ok())
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let survivors: Vec<_> = identities
            .iter()
            .filter(|identity| owned_process_liveness(identity, 0).is_ok())
            .collect();
        let sentinel_alive = sentinel.try_wait().unwrap().is_none();
        let _ = sentinel.kill();
        let _ = sentinel.wait();
        // The fixture has its own bounded lifetime, including on a red regression.
        assert!(
            survivors.is_empty(),
            "{mode}: owned processes survived: {survivors:?}"
        );
        assert!(sentinel_alive);
        eprintln!("mode={mode}: owned identities terminated, unrelated sentinel remained alive: {identities:?}");
        eprintln!("mode={mode}: {}", fixture_evidence(&mut owner, &marker));
        std::fs::remove_file(marker.with_extension("phase")).unwrap();
        std::fs::remove_file(marker).unwrap();
        let _ = std::fs::remove_file(std::env::temp_dir().join(format!(
            "meridian-owned-tree-{}-{mode}.pids",
            std::process::id()
        )));
    }
    std::fs::remove_dir_all(directory).unwrap();
}
