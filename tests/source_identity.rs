use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct SourceFixture(PathBuf);

impl SourceFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "meridian-source-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, source).unwrap();
        path
    }

    async fn parse(&self, environment: &Path) -> (ToolExecutionContext, ServerState) {
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![self.0.clone()], vec![]).unwrap(),
        );
        let state = ServerState::new();
        let parsed = payload(
            call_tool(
                &context,
                &state,
                "dm_parse_environment",
                json!({"dme_path":environment}),
            )
            .await
            .unwrap(),
        );
        assert_eq!(parsed["success"], true, "{parsed:#}");
        assert_eq!(
            parsed["error_count"], 0,
            "fixture did not parse cleanly: {parsed:#}"
        );
        (context, state)
    }
}

impl Drop for SourceFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn payload(result: ToolResult) -> Value {
    assert_eq!(result.is_error, None, "{result:?}");
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap()
}

#[cfg(unix)]
async fn case_fixture() -> (SourceFixture, ToolExecutionContext, ServerState) {
    let fixture = SourceFixture::new();
    let upper =
        "/datum/upper_source\n\tvar/value = 11\n/datum/upper_source/proc/resolve()\n\treturn 11\n";
    let lower =
        "/datum/lower_source\n\tvar/value = 22\n/datum/lower_source/proc/resolve()\n\treturn 22\n";
    let upper_path = fixture.write("Source.dm", upper);
    let lower_path = fixture.write("source.dm", lower);
    // Verify that the fixture actually has distinct files before testing lookup.
    assert_eq!(std::fs::read_to_string(upper_path).unwrap(), upper);
    assert_eq!(std::fs::read_to_string(lower_path).unwrap(), lower);
    let environment = fixture.write(
        "fixture.dme",
        "#include \"Source.dm\"\n#include \"source.dm\"\n",
    );
    let (context, state) = fixture.parse(&environment).await;
    (fixture, context, state)
}

#[cfg(unix)]
#[tokio::test]
async fn case_distinct_files_have_separate_document_symbols() {
    let (fixture, context, state) = case_fixture().await;
    for (file, owner) in [
        ("Source.dm", "/datum/upper_source"),
        ("source.dm", "/datum/lower_source"),
    ] {
        let body = payload(
            call_tool(
                &context,
                &state,
                "dm_document_symbols",
                json!({"file_path":fixture.0.join(file)}),
            )
            .await
            .unwrap(),
        );
        let rows = body["symbols"].as_array().unwrap();
        assert_eq!(rows.len(), 3, "merged symbols for {file}: {body:#}");
        assert!(
            rows.iter()
                .all(|row| row["id"]["path"] == owner || row["id"]["owner"] == owner),
            "{body:#}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn case_distinct_proc_locations_keep_their_search_owners_and_source() {
    let (_fixture, context, state) = case_fixture().await;
    for (file, owner, value) in [
        ("Source.dm", "/datum/upper_source", 11),
        ("source.dm", "/datum/lower_source", 22),
    ] {
        let body = payload(
            call_tool(
                &context,
                &state,
                "dm_search_context",
                json!({"query":format!("{owner}/proc/resolve"),"kind":"proc"}),
            )
            .await
            .unwrap(),
        );
        let rows = body["results"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "lost or misassigned procedure in {file}: {body:#}"
        );
        assert_eq!(rows[0]["implementation_owner"], owner);
        assert_eq!(rows[0]["declaration_owner"], owner);
        assert_eq!(rows[0]["line"], 3);
        assert!(rows[0]["file"].as_str().unwrap().ends_with(file));
        assert!(
            rows[0]["source"]
                .as_str()
                .unwrap()
                .contains(&format!("return {value}")),
            "{body:#}"
        );
    }
}

#[cfg(windows)]
#[tokio::test]
async fn windows_document_lookup_accepts_include_and_query_case_aliases() {
    let fixture = SourceFixture::new();
    fixture.write(
        "Source.dm",
        "/datum/case_alias\n\tvar/value = 11\n/datum/case_alias/proc/resolve()\n\treturn 11\n",
    );
    let environment = fixture.write("fixture.dme", "#include \"SOURCE.dm\"\n");
    let (context, state) = fixture.parse(&environment).await;
    for file in ["Source.dm", "source.dm", "SOURCE.dm"] {
        let body = payload(
            call_tool(
                &context,
                &state,
                "dm_document_symbols",
                json!({"file_path":fixture.0.join(file)}),
            )
            .await
            .unwrap(),
        );
        assert_eq!(body["count"], 3, "case alias {file}: {body:#}");
    }
    let body = payload(
        call_tool(
            &context,
            &state,
            "dm_search_context",
            json!({"query":"/datum/case_alias/proc/resolve","kind":"proc"}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(body["count"], 1, "{body:#}");
    assert_eq!(
        body["results"][0]["implementation_owner"],
        "/datum/case_alias"
    );
}
