use meridian_mcp::build_identity::{BuildIdentity, BuildIdentityInput};
use std::path::{Path, PathBuf};
use std::process::Command;

struct BuildScriptFixture {
    root: PathBuf,
}

impl BuildScriptFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "meridian-mcp-build-identity-rerun-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        Self { root }
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.root);
        command
    }

    fn cargo_build(&self) {
        let status = self
            .command("cargo")
            .args(["build", "--quiet"])
            .env("CARGO_TARGET_DIR", self.root.join("target"))
            .env_remove("MERIDIAN_BUILD_DIRTY")
            .env_remove("MERIDIAN_BUILD_REVISION")
            .status()
            .unwrap();
        assert!(status.success(), "fixture Cargo build failed");
    }

    fn embedded_dirty_state(&self) -> String {
        let executable = self.root.join("target/debug").join(if cfg!(windows) {
            "identity.exe"
        } else {
            "identity"
        });
        let output = Command::new(executable).output().unwrap();
        assert!(output.status.success(), "fixture executable failed");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

impl Drop for BuildScriptFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn write(path: impl AsRef<Path>, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

#[test]
fn build_identity_changes_when_source_or_binary_identity_changes() {
    let baseline = BuildIdentity::from_input(BuildIdentityInput {
        version: "0.1.0".to_owned(),
        source_revision: Some("a".repeat(40)),
        source_dirty: Some(false),
        target: "x86_64-pc-windows-msvc".to_owned(),
        profile: "release".to_owned(),
        executable_sha256: Some("1".repeat(64)),
    });
    let different_source = BuildIdentity::from_input(BuildIdentityInput {
        source_revision: Some("b".repeat(40)),
        ..baseline.as_input()
    });
    let different_binary = BuildIdentity::from_input(BuildIdentityInput {
        executable_sha256: Some("2".repeat(64)),
        ..baseline.as_input()
    });

    assert!(baseline.complete);
    assert_ne!(baseline.build_id, different_source.build_id);
    assert_ne!(baseline.build_id, different_binary.build_id);
}

#[test]
fn running_build_identity_hashes_the_current_executable() {
    let identity = meridian_mcp::build_identity::current();
    assert_eq!(identity.version, env!("CARGO_PKG_VERSION"));
    assert!(!identity.target.is_empty());
    assert!(!identity.profile.is_empty());
    assert_eq!(
        identity.executable_sha256.as_deref().map(str::len),
        Some(64)
    );
}

#[test]
fn local_dirty_detection_includes_untracked_source_files() {
    let build_script =
        std::fs::read_to_string("build.rs").expect("build script should be readable");
    assert!(build_script.contains("--untracked-files=all"));
    assert!(!build_script.contains("--untracked-files=no"));
}

#[test]
fn cargo_rebuild_refreshes_dirty_identity_after_an_unstaged_tracked_source_change() {
    let fixture = BuildScriptFixture::new();
    std::fs::copy("build.rs", fixture.root.join("build.rs")).unwrap();
    write(
        fixture.root.join("Cargo.toml"),
        "[package]\nname = \"identity\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    );
    write(
        fixture.root.join("src/main.rs"),
        "fn main() { println!(\"{}\", env!(\"MERIDIAN_BUILD_DIRTY\")); }\n",
    );
    write(fixture.root.join("README.md"), "clean\n");
    write(fixture.root.join(".gitignore"), "/target/\n");
    assert!(fixture
        .command("git")
        .args(["init", "--quiet"])
        .status()
        .unwrap()
        .success());
    assert!(fixture
        .command("cargo")
        .arg("generate-lockfile")
        .env_remove("MERIDIAN_BUILD_DIRTY")
        .env_remove("MERIDIAN_BUILD_REVISION")
        .status()
        .unwrap()
        .success());
    assert!(fixture
        .command("git")
        .args(["config", "core.autocrlf", "false"])
        .status()
        .unwrap()
        .success());
    assert!(fixture
        .command("git")
        .args(["add", "."])
        .status()
        .unwrap()
        .success());
    assert!(fixture
        .command("git")
        .args([
            "-c",
            "user.name=Meridian MCP Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "baseline",
        ])
        .status()
        .unwrap()
        .success());

    fixture.cargo_build();
    fixture.cargo_build();
    assert_eq!(fixture.embedded_dirty_state(), "false");

    write(
        fixture.root.join("src/main.rs"),
        "fn main() { println!(\"{}\", env!(\"MERIDIAN_BUILD_DIRTY\")); }\n// dirty\n",
    );
    fixture.cargo_build();
    assert_eq!(fixture.embedded_dirty_state(), "true");
}
