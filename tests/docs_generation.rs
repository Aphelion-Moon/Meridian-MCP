use meridian_mcp::result::ToolContent;
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy, RiftBuildAccess};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;

fn root() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "meridian-docs-audit-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

fn helper() -> &'static PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let binary = root().join(if cfg!(windows) {
            "helper.exe"
        } else {
            "helper"
        });
        let output = std::process::Command::new("rustc")
            .args([
                "+1.95.0",
                "--edition=2021",
                "tests/fixtures/docs_helper.rs",
                "-o",
            ])
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        binary
    })
}

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    project: PathBuf,
    context: Arc<ToolExecutionContext>,
    state: Arc<ServerState>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let root = root();
        let workspace = root.join("workspace");
        let project = workspace.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("fixture.dme"), "/world\n    fps = 10\n").unwrap();
        std::fs::write(project.join("mode.txt"), mode).unwrap();
        let context = Arc::new(ToolExecutionContext::with_features(
            CapabilityMode::Development,
            PathPolicy::new(vec![workspace.clone()], vec![]).unwrap(),
            RiftBuildAccess::Disabled,
            Some(helper().clone()),
            None,
            None,
        ));
        let state = Arc::new(ServerState::new());
        let parsed = call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":project.join("fixture.dme")}),
        )
        .await
        .unwrap();
        assert_ne!(parsed.is_error, Some(true));
        Self {
            root,
            workspace,
            project,
            context,
            state,
        }
    }
    fn output(&self) -> PathBuf {
        self.workspace.join("html")
    }
    async fn parse(&self) {
        let parsed = call_tool(
            &self.context,
            &self.state,
            "dm_parse_environment",
            json!({"dme_path":self.project.join("fixture.dme"),"force":true}),
        )
        .await
        .unwrap();
        assert_ne!(parsed.is_error, Some(true));
    }
    async fn call(&self, args: Value) -> (bool, usize, Value) {
        let mut request = json!({"output_directory":self.output()});
        request
            .as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        match call_tool(&self.context, &self.state, "dm_generate_docs", request).await {
            Ok(result) => {
                let ToolContent::Text { text } = &result.content[0];
                (
                    result.is_error == Some(true),
                    text.len(),
                    serde_json::from_str(text).unwrap_or_else(|_| json!({"message":text})),
                )
            }
            Err(error) => (
                true,
                error.to_string().len(),
                json!({"message":error.to_string()}),
            ),
        }
    }
    fn leftovers(&self) -> Vec<PathBuf> {
        [&self.root, &self.workspace, &self.project]
            .into_iter()
            .filter(|p| p.is_dir())
            .flat_map(|p| std::fs::read_dir(p).unwrap())
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".meridian-mcp-dmdoc-")
            })
            .collect()
    }
}

#[tokio::test]
async fn late_collision_cleans_only_owned_staging() {
    let f = Fixture::new("collision").await;
    assert!(f.call(json!({})).await.0);
    assert_eq!(
        std::fs::read_to_string(f.output().join("sentinel.txt")).unwrap(),
        "preserve collision"
    );
    assert!(
        f.leftovers().is_empty(),
        "staging leaked after late collision"
    );
}

#[tokio::test]
async fn markdown_and_index_inputs_cannot_be_replaced_by_documentation() {
    for index_only in [false, true] {
        let f = Fixture::new("quiet").await;
        let manual = f.project.join("manual");
        std::fs::create_dir(&manual).unwrap();
        std::fs::write(manual.join("index.md"), "# Owned documentation source\n").unwrap();
        let config = if index_only {
            "[dmdoc]\nmodule_directories = [\"fixture.dme\"]\nindex_file = \"manual/index.md\"\n"
        } else {
            "[dmdoc]\nmodule_directories = [\"manual\"]\n"
        };
        // dmdoc discovers configuration at execution time, including config
        // added after the active analysis snapshot was built.
        std::fs::write(f.project.join("SpacemanDMM.toml"), config).unwrap();
        let (error, _, _) = f
            .call(json!({"output_directory": manual, "overwrite": true}))
            .await;
        assert!(error, "documentation source was replaced");
        assert!(
            !f.project.join("helper.pid").exists(),
            "reject before invoking the helper"
        );
        assert_eq!(
            std::fs::read_to_string(manual.join("index.md")).unwrap(),
            "# Owned documentation source\n"
        );
    }
}

#[tokio::test]
async fn output_files_and_source_directories_are_rejected_before_execution() {
    let mut observations = Vec::new();
    for kind in ["file", "project", "workspace"] {
        let f = Fixture::new("quiet").await;
        let output = match kind {
            "file" => {
                std::fs::write(f.output(), "preserve file").unwrap();
                f.output()
            }
            "project" => f.project.clone(),
            _ => f.workspace.clone(),
        };
        let (error, _, _) = f
            .call(json!({"output_directory":output,"overwrite":true}))
            .await;
        let preserved = kind != "file"
            || std::fs::read_to_string(&output).ok().as_deref() == Some("preserve file");
        observations.push((
            kind,
            error,
            f.project.join("fixture.dme").is_file(),
            f.project.join("helper.pid").exists(),
            preserved,
            f.leftovers().len(),
        ));
    }
    assert_eq!(
        observations,
        vec![
            ("file", true, true, false, true, 0),
            ("project", true, true, false, true, 0),
            ("workspace", true, true, false, true, 0)
        ]
    );
}

#[tokio::test]
async fn nested_source_output_is_rejected_before_execution() {
    let original = "/datum/docs_source\n    var/keep = 1\n";
    for include in ["code", "../shared"] {
        let f = Fixture::new("quiet").await;
        let source_dir = f.project.join(include);
        std::fs::create_dir(&source_dir).unwrap();
        let source = source_dir.join("example.dm");
        std::fs::write(&source, original).unwrap();
        std::fs::write(
            f.project.join("fixture.dme"),
            format!("#include \"{include}/example.dm\"\n"),
        )
        .unwrap();
        f.parse().await;

        let (error, _, _) = f
            .call(json!({"output_directory":source_dir,"overwrite":true}))
            .await;
        let observed = (
            error,
            f.project.join("helper.pid").exists(),
            std::fs::read_to_string(&source).ok(),
            f.leftovers().len(),
        );
        assert_eq!(observed, (true, false, Some(original.to_owned()), 0));
    }
}

#[cfg(any(windows, unix))]
#[tokio::test]
async fn linked_output_is_rejected_before_execution_and_preserves_its_target() {
    let f = Fixture::new("quiet").await;
    let target = f.workspace.join("previous-docs");
    std::fs::create_dir(&target).unwrap();
    let sentinel = target.join("sentinel.txt");
    std::fs::write(&sentinel, "preserve target").unwrap();
    let link = f.output();
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap()
        .status
        .success());
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let mut observations = Vec::new();
    for output in [link.clone(), link.join(""), link.join(".")] {
        let (error, _, _) = f
            .call(json!({"output_directory":output,"overwrite":true}))
            .await;
        observations.push((
            error,
            f.project.join("helper.pid").exists(),
            std::fs::read_to_string(&sentinel).ok(),
            std::fs::symlink_metadata(&link).is_ok(),
            f.leftovers().len(),
        ));
    }
    // Remove only the fixture link before the fixture's recursive cleanup.
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
    assert_eq!(
        observations,
        vec![(true, false, Some("preserve target".to_owned()), true, 0); 3]
    );
}

#[tokio::test]
async fn malformed_output_arguments_are_rejected_before_execution() {
    for args in [
        json!({"overwrite":"false"}),
        json!({"overwrite":null}),
        json!({"unknown":1}),
        json!({"include_output":null}),
        json!({"output_max_bytes":0}),
        json!({"output_max_bytes":65537}),
    ] {
        let f = Fixture::new("quiet").await;
        assert!(f.call(args).await.0);
        assert!(!f.project.join("helper.pid").exists());
        assert!(f.leftovers().is_empty());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires TMPDIR on a filesystem that rejects RENAME_NOREPLACE"]
async fn unsupported_filesystem_is_rejected_before_execution() {
    let f = Fixture::new("quiet").await;
    std::fs::create_dir(f.output()).unwrap();
    std::fs::write(f.output().join("old.html"), "old docs").unwrap();
    let (error, _, body) = f.call(json!({"overwrite":true})).await;
    let observed = (
        error,
        f.project.join("helper.pid").exists(),
        std::fs::read_to_string(f.output().join("old.html")).ok(),
        f.leftovers().len(),
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("RENAME_NOREPLACE")),
    );
    assert_eq!(
        observed,
        (true, false, Some("old docs".to_owned()), 0, true)
    );
}

#[tokio::test]
async fn helper_failure_and_missing_index_preserve_existing_docs_and_clean_staging() {
    for mode in ["fail", "missing_index"] {
        let f = Fixture::new(mode).await;
        std::fs::create_dir(f.output()).unwrap();
        std::fs::write(f.output().join("sentinel.txt"), "old docs").unwrap();
        assert!(f.call(json!({"overwrite":true})).await.0);
        assert_eq!(
            std::fs::read_to_string(f.output().join("sentinel.txt")).unwrap(),
            "old docs"
        );
        assert!(f.leftovers().is_empty());
    }
}

#[tokio::test]
async fn successful_replacement_installs_complete_docs_without_backups() {
    let f = Fixture::new("quiet").await;
    for output in [f.output(), f.project.join("html")] {
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("old.html"), "old docs").unwrap();
        let (error, _, body) = f
            .call(json!({"output_directory":output,"overwrite":true}))
            .await;
        assert!(!error, "documentation reply: {body}");
        assert_eq!(body["files"], 2);
        assert!(output.join("types/example.html").is_file());
        assert!(!output.join("old.html").exists());
        assert!(f.leftovers().is_empty());
    }
}

#[tokio::test]
async fn documentation_inputs_created_during_generation_are_preserved() {
    let f = Fixture::new("new_input").await;
    let output = f.project.join("manual");
    std::fs::create_dir(&output).unwrap();
    let (error, _, body) = f
        .call(json!({"output_directory":output,"overwrite":true}))
        .await;
    assert!(error);
    assert_eq!(body["installed"], false);
    assert_eq!(
        std::fs::read_to_string(output.join("late.md")).unwrap(),
        "preserve late source"
    );
    assert!(f.leftovers().is_empty());
}

#[tokio::test]
async fn bounded_logs_preserve_install_and_failure_results() {
    for mode in ["flood", "flood_fail"] {
        for options in [
            json!({}),
            json!({"include_output":false}),
            json!({"output_max_bytes":65536}),
        ] {
            let f = Fixture::new(mode).await;
            let (error, bytes, body) = f.call(options.clone()).await;
            assert!(bytes <= 262144, "reply bytes: {bytes}");
            assert_eq!(error, mode == "flood_fail");
            assert_eq!(body["installed"], mode == "flood");
            assert_eq!(body["truncated"], true);
            if options["include_output"] == false {
                assert!(body.get("stdout").is_none() && body.get("stderr").is_none());
            }
            if mode == "flood" {
                assert!(f.output().join("index.html").is_file());
            }
            assert!(f.leftovers().is_empty());
        }
    }
}

async fn wait_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("helper did not become ready");
}

#[tokio::test]
async fn cancellation_stops_the_helper_then_removes_locked_staging() {
    let f = Fixture::new("wait").await;
    let (context, state, output) = (f.context.clone(), f.state.clone(), f.output());
    let task = tokio::spawn(async move {
        call_tool(
            &context,
            &state,
            "dm_generate_docs",
            json!({"output_directory":output}),
        )
        .await
    });
    wait_file(&f.project.join("ready")).await;
    wait_file(&f.project.join("heartbeat")).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.leftovers().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled generation leaked staging");
    let heartbeat = std::fs::read(f.project.join("heartbeat")).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        heartbeat,
        std::fs::read(f.project.join("heartbeat")).unwrap(),
        "cancelled helper remains active"
    );
    assert!(!f.output().exists());
}
