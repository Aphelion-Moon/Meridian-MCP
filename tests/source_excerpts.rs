use meridian_mcp::result::{ToolContent, ToolResult};
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    root: PathBuf,
    context: ToolExecutionContext,
    state: ServerState,
}

impl Fixture {
    async fn new(source: impl AsRef<[u8]>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "meridian-source-excerpts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("fixture.dm"), source).unwrap();
        std::fs::write(root.join("fixture.dme"), "#include \"fixture.dm\"\n").unwrap();
        let context = ToolExecutionContext::new(
            CapabilityMode::Analysis,
            PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
        );
        let fixture = Self {
            root,
            context,
            state: ServerState::new(),
        };
        let result = fixture
            .call(
                "dm_parse_environment",
                json!({"dme_path":fixture.root.join("fixture.dme")}),
            )
            .await;
        assert_eq!(result["success"], true, "{result:#}");
        assert_eq!(result["error_count"], 0, "{result:#}");
        fixture
    }

    async fn call(&self, tool: &str, args: Value) -> Value {
        payload(
            call_tool(&self.context, &self.state, tool, args)
                .await
                .unwrap(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn payload(result: ToolResult) -> Value {
    assert_eq!(result.is_error, None, "{result:?}");
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn nested_proc_excerpts_do_not_include_siblings() {
    let fixture = Fixture::new("/datum/nested\n\tproc/first()\n\t\treturn \"FIRST\"\n\tproc/second()\n\t\treturn \"SECOND\"\n").await;
    for (name, expected) in [
        ("first", "\tproc/first()\n\t\treturn \"FIRST\""),
        ("second", "\tproc/second()\n\t\treturn \"SECOND\""),
    ] {
        let exact = fixture
            .call(
                "dm_get_proc",
                json!({"type_path":"/datum/nested","proc_name":name}),
            )
            .await;
        let search = fixture
            .call(
                "dm_search_context",
                json!({"query":format!("/datum/nested/proc/{name}")}),
            )
            .await;
        assert_eq!(exact["overrides"][0]["source"], expected, "{exact:#}");
        assert_eq!(search["results"][0]["source"], expected, "{search:#}");
        assert_eq!(exact["overrides"][0]["source_truncated"], false);
        assert_eq!(search["results"][0]["source_truncated"], false);
    }
}

#[tokio::test]
async fn same_line_procs_retain_their_own_text() {
    let first = "/proc/inline_first() { return \"caf\u{e9}\"; }";
    let second = "/proc/inline_second() { return 2; }";
    let fixture = Fixture::new(format!("{first}; {second}\n")).await;
    for (name, expected) in [("inline_first", first), ("inline_second", second)] {
        let exact = fixture
            .call("dm_get_proc", json!({"type_path":"","proc_name":name}))
            .await;
        assert_eq!(exact["overrides"][0]["source"], expected, "{exact:#}");
        assert_eq!(exact["overrides"][0]["source_start_line"], 1);
    }
}

#[tokio::test]
async fn source_decoding_matches_the_parser_for_bom_crlf_and_latin1() {
    for bytes in [
        b"\xef\xbb\xbf/proc/encoding()\r\n\treturn \"caf\xc3\xa9\"\r\n".as_slice(),
        b"/proc/encoding()\r\n\treturn \"caf\xe9\"\r\n".as_slice(),
    ] {
        let fixture = Fixture::new(bytes).await;
        let exact = fixture
            .call(
                "dm_get_proc",
                json!({"type_path":"","proc_name":"encoding"}),
            )
            .await;
        assert_eq!(
            exact["overrides"][0]["source"], "/proc/encoding()\n\treturn \"caf\u{e9}\"",
            "{exact:#}"
        );
    }
}

#[tokio::test]
async fn multiline_strings_and_comment_markers_do_not_change_proc_boundaries() {
    let expected = "/datum/text/proc/body()\n\tvar/text = {\"\nCOLUMN ZERO\n/* literal marker\n\"}\n\treturn text";
    let fixture = Fixture::new(&format!(
        "{expected}\n/datum/text/proc/next()\n\treturn \"NEXT\"\n"
    ))
    .await;
    let exact = fixture
        .call(
            "dm_get_proc",
            json!({"type_path":"/datum/text","proc_name":"body"}),
        )
        .await;
    assert_eq!(exact["overrides"][0]["source"], expected, "{exact:#}");
}

#[tokio::test]
async fn source_limits_are_honored_and_snapshot_truncation_is_explicit() {
    let mut source = String::from("/datum/long_proc/proc/run()\n\tvar/total = 0\n");
    for value in 1..=240 {
        source.push_str(&format!("\ttotal += {value}\n"));
    }
    source.push_str("\treturn total\n");
    let fixture = Fixture::new(&source).await;
    // Both readers must retain snapshot text after the source changes on disk.
    std::fs::write(fixture.root.join("fixture.dm"), "/datum/changed\n").unwrap();
    for limit in [1, 40, 80, 200] {
        let search = fixture
            .call(
                "dm_search_context",
                json!({"query":"/datum/long_proc/proc/run", "max_source_lines":limit}),
            )
            .await;
        let row = &search["results"][0];
        assert_eq!(row["source"].as_str().unwrap().lines().count(), limit);
        assert_eq!(row["source_truncated"], true, "{search:#}");
        assert_eq!(row["source_total_lines"], 243);
        assert_eq!(search["state_generation"], 1);
        assert_eq!(search["source_origin"], "analysis_snapshot");
    }
    let exact = fixture
        .call(
            "dm_get_proc",
            json!({"type_path":"/datum/long_proc","proc_name":"run"}),
        )
        .await;
    let row = &exact["overrides"][0];
    assert_eq!(row["source"].as_str().unwrap().lines().count(), 80);
    assert_eq!(row["source_truncated"], true);
    assert_eq!(row["source_total_lines"], 243);
}

#[tokio::test]
async fn search_rejects_invalid_filters_flags_and_limits() {
    let fixture = Fixture::new("/datum/simple/proc/run()\n\treturn 1\n").await;
    for (key, value) in [
        ("kind", json!(false)),
        ("kind", json!(null)),
        ("kind", json!("invalid")),
        ("include_source", json!("false")),
        ("include_source", json!(null)),
        ("type_prefix", json!(17)),
        ("type_prefix", json!(" ")),
        ("file_filter", json!(false)),
        ("file_filter", json!("")),
        ("limit", json!(0)),
        ("limit", json!(51)),
        ("limit", json!(1.5)),
        ("max_source_lines", json!(0)),
        ("max_source_lines", json!(201)),
    ] {
        let mut args = json!({"query":"run"});
        args[key] = value;
        let result = call_tool(
            &fixture.context,
            &fixture.state,
            "dm_search_context",
            args.clone(),
        )
        .await;
        let result = result.unwrap();
        assert_eq!(result.is_error, Some(true), "accepted {args}");
        let ToolContent::Text { text } = &result.content[0];
        let error: Value = serde_json::from_str(text).unwrap();
        assert_eq!(error["code"], "invalid_input", "{error}");
        assert_eq!(error["details"]["field"], key, "{error}");
    }
}

#[tokio::test]
async fn source_omission_keeps_results_and_snapshot_identity() {
    let fixture = Fixture::new("/datum/simple/proc/run()\n\treturn 1\n").await;
    let with = fixture
        .call("dm_search_context", json!({"query":"run"}))
        .await;
    let without = fixture
        .call(
            "dm_search_context",
            json!({"query":"run","include_source":false}),
        )
        .await;
    assert_eq!(with["count"], without["count"]);
    assert_eq!(without["state_generation"], 1);
    for (a, b) in with["results"]
        .as_array()
        .unwrap()
        .iter()
        .zip(without["results"].as_array().unwrap())
    {
        assert_eq!(a["symbol"], b["symbol"]);
        assert_eq!(a["score"], b["score"]);
        assert!(b.get("source").is_none());
        assert!(b.get("source_truncated").is_none());
    }
}

#[tokio::test]
async fn exact_inspection_accepts_a_source_budget_or_omits_source() {
    let fixture = Fixture::new("/datum/simple/proc/run()\n\treturn 1\n").await;
    let small = fixture
        .call(
            "dm_get_proc",
            json!({"type_path":"/datum/simple","proc_name":"run","max_source_lines":1}),
        )
        .await;
    assert_eq!(small["overrides"][0]["source"], "/datum/simple/proc/run()");
    assert_eq!(small["overrides"][0]["source_truncated"], true);
    let omitted = fixture
        .call(
            "dm_get_proc",
            json!({"type_path":"/datum/simple","proc_name":"run","include_source":false}),
        )
        .await;
    assert_eq!(
        omitted["implementation_owner"],
        small["implementation_owner"]
    );
    assert_eq!(
        omitted["overrides"][0]["parameters"],
        small["overrides"][0]["parameters"]
    );
    assert!(omitted["overrides"][0].get("source").is_none());
    assert!(omitted["overrides"][0].get("source_truncated").is_none());
    for (key, value) in [
        ("include_source", json!("false")),
        ("max_source_lines", json!(0)),
        ("max_source_lines", json!(201)),
    ] {
        let mut args = json!({"type_path":"/datum/simple","proc_name":"run"});
        args[key] = value;
        let result = call_tool(
            &fixture.context,
            &fixture.state,
            "dm_get_proc",
            args.clone(),
        )
        .await;
        let result = result.unwrap();
        assert_eq!(result.is_error, Some(true), "accepted {args}");
        let ToolContent::Text { text } = &result.content[0];
        let error: Value = serde_json::from_str(text).unwrap();
        assert_eq!(error["code"], "invalid_input", "{error}");
        assert_eq!(error["details"]["field"], key, "{error}");
    }
}
