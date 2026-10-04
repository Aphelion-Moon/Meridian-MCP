use meridian_mcp::{EffectiveRoot, PrivateStateStore, RootSource};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

struct StateFixture {
    base: PathBuf,
    workspace: PathBuf,
    state: PathBuf,
}

impl StateFixture {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "meridian-mcp-private-state-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = base.join("workspace");
        let state = base.join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        Self {
            base,
            workspace,
            state,
        }
    }

    fn roots(&self) -> Vec<EffectiveRoot> {
        vec![EffectiveRoot {
            path: self.workspace.canonicalize().unwrap(),
            source: RootSource::ExplicitRoot,
            repository_identity: None,
            head_revision: None,
            dirty: None,
        }]
    }
}

impl Drop for StateFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn development_state_must_be_outside_workspace_and_writable() {
    let fixture = StateFixture::new("boundary");
    assert!(PrivateStateStore::open(&fixture.state, &fixture.roots()).is_ok());
    let nested_state = fixture.workspace.join("state");
    std::fs::create_dir_all(&nested_state).unwrap();
    assert!(PrivateStateStore::open(&nested_state, &fixture.roots()).is_err());
}

#[test]
fn atomic_records_survive_reopen_and_reject_traversal() {
    let fixture = StateFixture::new("reopen");
    let store = PrivateStateStore::open(&fixture.state, &fixture.roots()).unwrap();
    store
        .write_json_atomic("builds/example.json", &json!({"schema": 1}))
        .unwrap();
    assert!(store
        .write_json_atomic("../escape.json", &json!({"schema": 1}))
        .is_err());
    drop(store);

    let reopened = PrivateStateStore::open(&fixture.state, &fixture.roots()).unwrap();
    assert_eq!(
        reopened.read_json::<Value>("builds/example.json").unwrap()["schema"],
        1
    );
    assert_eq!(reopened.list_records("builds", 10).unwrap().len(), 1);
}

#[test]
fn escaped_namespace_is_rejected_before_creating_directories() {
    let fixture = StateFixture::new("linked-namespace");
    let store = PrivateStateStore::open(&fixture.state, &fixture.roots()).unwrap();
    let outside = fixture.base.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = fixture.state.join("linked");
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success());
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let result = store.write_json_atomic("linked/new/record.json", &json!({"schema": 1}));
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
    assert!(result.is_err());
    assert!(
        !outside.join("new").exists(),
        "rejection must precede outside writes"
    );
}

#[test]
fn multiple_store_instances_share_one_private_state_directory() {
    let fixture = StateFixture::new("concurrent-open");
    let first = PrivateStateStore::open(&fixture.state, &fixture.roots()).unwrap();
    let second = PrivateStateStore::open(&fixture.state, &fixture.roots()).unwrap();

    first
        .write_json_atomic("builds/first.json", &json!({"writer": "first"}))
        .unwrap();
    second
        .write_json_atomic("builds/second.json", &json!({"writer": "second"}))
        .unwrap();

    assert_eq!(
        second.read_json::<Value>("builds/first.json").unwrap()["writer"],
        "first"
    );
    assert_eq!(first.list_records("builds", 10).unwrap().len(), 2);
}

#[test]
fn write_waits_for_the_cross_process_operation_lock() {
    let fixture = StateFixture::new("operation-lock");
    let lock_path = fixture.state.join(".meridian-mcp.lock");
    let external_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    external_lock.lock().unwrap();
    let store = PrivateStateStore::open(&fixture.state, &fixture.roots()).unwrap();
    let (sender, receiver) = mpsc::channel();

    let writer = std::thread::spawn(move || {
        let result = store.write_json_atomic("builds/blocked.json", &json!({"complete": true}));
        sender.send(result).unwrap();
    });

    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    external_lock.unlock().unwrap();
    receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    writer.join().unwrap();
}
