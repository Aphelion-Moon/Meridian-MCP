use super::*;
use crate::mcp::ToolContent;
use crate::tools::{call_tool, ToolExecutionContext};

fn payload(result: &ToolResult) -> Value {
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap_or_else(|_| json!({"text":text}))
}

fn directory(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "meridian-launch-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

#[test]
fn launch_reserved_arguments_cannot_override_managed_settings() {
    for extra in [
        json!(["-ip", "0.0.0.0"]),
        json!(["-IP=0.0.0.0"]),
        json!(["-ip 0.0.0.0"]),
        json!(["--ip", "0.0.0.0"]),
        json!(["-cd", "elsewhere"]),
        json!(["-port", "9999"]),
        json!(["-ports", "1-65535"]),
        json!(["9999"]),
        json!(["+9999"]),
        json!(["replacement.dmb"]),
        json!(["-params"]),
        json!(["-params", "x\u{0}y"]),
    ] {
        assert!(
            crate::parameters::decode::<crate::parameters::RunParams>(
                json!({"dmb_path":"fixture.dmb", "daemon_args":extra})
            )
            .is_err(),
            "accepted {extra}"
        );
    }
    for extra in [
        json!([
            "-close",
            "-verbose",
            "-params",
            "port=9999&note=-ip 0.0.0.0"
        ]),
        json!(["-params", "-ip", "-params", "9999"]),
        json!(["-log", "9999", "-params", "replacement.dmb"]),
        json!(["-params note=-ip 0.0.0.0", "-nologdates"]),
        json!(["-params file=fixture.dmb"]),
    ] {
        assert!(
            crate::parameters::decode::<crate::parameters::RunParams>(
                json!({"dmb_path":"fixture.dmb", "daemon_args":extra})
            )
            .is_ok(),
            "rejected option values {extra}"
        );
    }
}

#[tokio::test]
async fn launch_file_working_directory_is_rejected_before_lifecycle_access() {
    let root = directory("invalid-directory");
    let dmb = root.join("fixture.dmb");
    std::fs::write(&dmb, "fixture").unwrap();
    let context = ToolExecutionContext::new(
        crate::CapabilityMode::Development,
        crate::PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let lifecycle = state.lifecycle().await;
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        call_tool(
            &context,
            &state,
            "dm_run",
            json!({"dmb_path":dmb, "working_directory":dmb}),
        ),
    )
    .await;
    drop(lifecycle);
    std::fs::remove_dir_all(&root).unwrap();
    let result = result
        .expect("file-valued working_directory waited for lifecycle access")
        .unwrap();
    assert_eq!(payload(&result)["code"], "invalid_input");
}

#[tokio::test]
async fn launch_relative_paths_keep_containment_checks() {
    let root = directory("containment");
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let dmb = root.join("outside.dmb");
    std::fs::write(&dmb, "fixture").unwrap();
    std::fs::write(workspace.join("inside.dmb"), "fixture").unwrap();
    let context = ToolExecutionContext::new(
        crate::CapabilityMode::Development,
        crate::PathPolicy::new(vec![workspace.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let mut outcomes = Vec::new();
    for args in [
        json!({"dmb_path":"../outside.dmb", "working_directory":workspace}),
        json!({"dmb_path":workspace.join("inside.dmb"), "working_directory":root}),
    ] {
        outcomes.push(payload(
            &call_tool(&context, &state, "dm_run", args).await.unwrap(),
        ));
    }
    std::fs::remove_dir_all(root).unwrap();
    for outcome in outcomes {
        assert_eq!(outcome["code"], "path_outside_workspace", "{outcome}");
    }
}

#[tokio::test]
async fn launch_uses_requested_directory_and_preserves_artifact_integrity_scope() {
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
    let root = directory("directory-with-spaces");
    let workspace = root.join("workspace");
    let artifacts = workspace.join("artifacts");
    let requested = workspace.join("requested directory");
    let private = root.join("state");
    for path in [&artifacts, &requested, &private] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::create_dir(workspace.join(".git")).unwrap();
    let source = root.join("daemon.rs");
    std::fs::write(
        &source,
        r#"
        use std::io::Write;
        fn main() {
            println!("LAUNCH_CWD:{}", std::env::current_dir().unwrap().display());
            for arg in std::env::args().skip(1) { println!("LAUNCH_ARG:{arg}"); }
            println!("LAUNCH_READY");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(15));
        }
    "#,
    )
    .unwrap();
    let compiler = root.join("dm.exe");
    assert!(std::process::Command::new("rustc")
        .args(["+1.95.0", "--edition=2021"])
        .arg(&source)
        .arg("-o")
        .arg(root.join("dreamdaemon.exe"))
        .status()
        .unwrap()
        .success());
    std::fs::copy(root.join("dreamdaemon.exe"), &compiler).unwrap();
    let dmb = artifacts.join("fixture.dmb");
    std::fs::write(&dmb, "fixture").unwrap();
    let tracked = artifacts.join("tracked.dm");
    std::fs::write(&tracked, "before").unwrap();
    let policy = crate::PathPolicy::new(vec![workspace.clone()], vec![compiler]).unwrap();
    let store =
        Arc::new(crate::PrivateStateStore::open(&private, policy.effective_roots()).unwrap());
    let context = ToolExecutionContext::with_features_and_state(
        crate::CapabilityMode::Development,
        policy,
        crate::RiftBuildAccess::Disabled,
        None,
        None,
        None,
        Some(store),
    );
    let state = ServerState::new();
    let mut outcomes = Vec::new();
    for (case, input, cwd) in [
        ("default", json!(dmb), None),
        ("absolute", json!(dmb), Some(&requested)),
        (
            "relative",
            json!("../artifacts/fixture.dmb"),
            Some(&requested),
        ),
    ] {
        let mut args =
            json!({"dmb_path":input, "wait_for":"LAUNCH_READY", "startup_timeout_ms":3000});
        if let Some(cwd) = cwd {
            args["working_directory"] = json!(cwd);
        }
        let result = call_tool(&context, &state, "dm_run", args).await.unwrap();
        if case == "absolute" {
            std::fs::write(&tracked, "after").unwrap();
        }
        let stopped = super::stop(&state, crate::parameters::StopParams::default())
            .await
            .unwrap();
        outcomes.push((case, payload(&result), payload(&stopped)));
    }
    drop(state);
    drop(context);
    std::fs::remove_dir_all(&root).unwrap();
    let problems: Vec<_> = outcomes.iter().filter_map(|(case,result,stopped)| {
        let expected = if *case == "default" { &artifacts } else { &requested };
        let lines = result["readiness"]["recent_output"].as_array().cloned().unwrap_or_default();
        let expected_cwd = format!("LAUNCH_CWD:{}", expected.display());
        let has_cwd = lines.iter().any(|line| line == &expected_cwd);
        let args: Vec<_> = lines.iter().filter_map(|line| line.as_str()?.strip_prefix("LAUNCH_ARG:")).collect();
        let has_bind = args.windows(2).any(|pair| pair == ["-ip", "127.0.0.1"]);
        let expected_cd = expected.display().to_string();
        let has_cd = args.windows(2).any(|pair| pair == ["-cd", &expected_cd]);
        let tracks_artifact = *case != "absolute" || stopped["integrity"]["warnings"].as_array().is_some_and(|warnings| warnings.iter().any(|warning| warning["relative_path"] == "tracked.dm"));
        (result["success"] != true || result["working_directory"] != expected_cd || !has_cwd || !has_bind || !has_cd || !tracks_artifact).then(|| format!("{case}: cwd={has_cwd}, explicit_cd={has_cd}, loopback={has_bind}, artifact_integrity={tracks_artifact}, result={result}"))
    }).collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
