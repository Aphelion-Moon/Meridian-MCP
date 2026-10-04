use meridian_mcp::result::ToolContent;
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};

struct FixtureDirectory(std::path::PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("meridian-language-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn payload(result: meridian_mcp::result::ToolResult) -> Value {
    assert_eq!(result.is_error, None, "{result:?}");
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap()
}

fn error_payload(result: meridian_mcp::result::ToolResult) -> Value {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap()
}

async fn fixture() -> (ToolExecutionContext, ServerState, std::path::PathBuf) {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/language");
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    let parsed = payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":root.join("semantics.dme")}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(parsed["success"], true);
    (context, state, root)
}

#[tokio::test]
async fn implementation_queries_follow_semantic_parents_and_include_variable_overrides() {
    let (context, state, _) = fixture().await;
    for member in [None, Some("work"), Some("charge")] {
        let mut args = json!({"type_path":"/datum/query_base"});
        if let Some(member) = member {
            args["member_name"] = json!(member);
        }
        let body = payload(
            call_tool(&context, &state, "dm_find_implementations", args)
                .await
                .unwrap(),
        );
        let rows = body["implementations"].as_array().unwrap();
        let owners: Vec<_> = rows
            .iter()
            .map(|row| row["implementation_owner"].as_str().unwrap())
            .collect();
        assert_eq!(
            owners,
            [
                "/datum/query_base",
                "/datum/query_alias",
                "/datum/query_base/local"
            ],
            "member {member:?}: {body:#}"
        );
        if member.is_some() {
            assert!(rows
                .iter()
                .all(|row| row["declaration_owner"] == "/datum/query_base"));
        }
    }
}

#[tokio::test]
async fn references_include_the_actual_declaration_on_request() {
    let (context, state, _) = fixture().await;
    let args = json!({"type_path":"/datum/query_alias","member_name":"charge"});
    let uses = payload(
        call_tool(&context, &state, "dm_find_references", args.clone())
            .await
            .unwrap(),
    );
    let mut including = args.clone();
    including["include_declaration"] = json!(true);
    let body = payload(
        call_tool(&context, &state, "dm_find_references", including.clone())
            .await
            .unwrap(),
    );
    assert_eq!(
        body["count"].as_u64().unwrap(),
        uses["count"].as_u64().unwrap() + 1,
        "{body:#}"
    );
    let declaration = body["references"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "declaration")
        .expect("declaration must be labeled separately from a use");
    assert_eq!(declaration["symbol"]["owner"], "/datum/query_base");
    assert_eq!(declaration["line"], 2);
    including["kind"] = json!("declaration");
    let only = payload(
        call_tool(&context, &state, "dm_find_references", including)
            .await
            .unwrap(),
    );
    assert_eq!(only["count"], 1);
}

#[tokio::test]
async fn language_index_shares_repeated_file_and_owner_storage() {
    let (_, state, _) = fixture().await;
    let snapshot = state.snapshot().await.unwrap();
    let file = snapshot
        .language_index
        .source_files()
        .into_iter()
        .find(|file| file.file_name().is_some_and(|name| name == "semantics.dm"))
        .unwrap();
    let symbols = snapshot.language_index.document_symbols(&file);
    assert!(symbols.len() > 10);
    // Guard the measured storage regression: adding assignments must not allocate
    // another copy of the source path for every document and implementation row.
    let file = symbols[0].file.as_ptr();
    assert!(symbols.iter().all(|symbol| symbol.file.as_ptr() == file));
    let implementations = snapshot
        .language_index
        .implementations("/datum/query_base", Some("work"));
    for hit in implementations {
        assert_eq!(hit.file.as_ptr(), file);
        let document = symbols
            .iter()
            .find(|symbol| symbol.id == hit.symbol)
            .unwrap();
        assert_eq!(
            hit.implementation_owner.as_ptr(),
            document.implementation_owner.as_ref().unwrap().as_ptr()
        );
        assert_eq!(hit.declared_in.as_ptr(), hit.implementation_owner.as_ptr());
        assert_eq!(
            hit.declaration_owner.as_ptr(),
            document.declaration_owner.as_ref().unwrap().as_ptr()
        );
    }
}

#[tokio::test]
async fn language_queries_reject_invalid_limits_and_missing_types() {
    let (context, state, root) = fixture().await;
    for tool in [
        "dm_find_references",
        "dm_find_implementations",
        "dm_document_symbols",
    ] {
        for limit in [json!(0), json!(-1), json!("10"), json!(true)] {
            let mut args = if tool == "dm_document_symbols" {
                json!({"file_path":root.join("semantics.dm")})
            } else {
                json!({"type_path":"/datum/query_base","member_name":"charge"})
            };
            args["limit"] = limit.clone();
            let result = call_tool(&context, &state, tool, args).await.unwrap();
            assert_eq!(
                result.is_error,
                Some(true),
                "{tool} accepted invalid limit {limit}"
            );
            let error = error_payload(result);
            assert_eq!(error["code"], "invalid_input", "{error}");
            assert_eq!(error["details"]["field"], "limit", "{error}");
        }
    }
    let missing = call_tool(
        &context,
        &state,
        "dm_find_implementations",
        json!({"type_path":"/datum/typo"}),
    )
    .await;
    assert!(missing.is_err());
}

#[tokio::test]
async fn declaration_inclusion_supports_types_and_inherited_procs() {
    let (context, state, _) = fixture().await;
    for (owner, member, declaration_line) in [
        ("/datum/query_base", None, 1),
        ("/datum/query_alias", Some("work"), 4),
    ] {
        let mut args = json!({"type_path":owner,"include_declaration":true,"kind":"declaration"});
        if let Some(member) = member {
            args["member_name"] = json!(member);
        }
        let body = payload(
            call_tool(&context, &state, "dm_find_references", args)
                .await
                .unwrap(),
        );
        assert_eq!(body["count"], 1, "{body:#}");
        assert_eq!(body["references"][0]["line"], declaration_line);
        assert_eq!(body["references"][0]["kind"], "declaration");
    }
}

fn expanded_rows(body: &Value, key: &str) -> Vec<Value> {
    body[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let mut full = body
                .get("shared")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            full.extend(row.as_object().unwrap().clone());
            Value::Object(full)
        })
        .collect()
}

#[tokio::test]
async fn language_pages_preserve_complete_results_and_compact_fields() {
    let (context, state, root) = fixture().await;
    for (tool, key, args) in [
        (
            "dm_document_symbols",
            "symbols",
            json!({"file_path":root.join("semantics.dm")}),
        ),
        (
            "dm_find_implementations",
            "implementations",
            json!({"type_path":"/datum/query_base", "member_name":"work"}),
        ),
        (
            "dm_find_references",
            "references",
            json!({"type_path":"/datum/query_base", "member_name":"charge", "include_declaration":true}),
        ),
    ] {
        let full = payload(
            call_tool(&context, &state, tool, args.clone())
                .await
                .unwrap(),
        );
        let expected = full[key].as_array().unwrap();
        assert!(expected.len() >= 3);
        for detail in ["full", "compact"] {
            let mut query = args.clone();
            query["limit"] = json!(2);
            query["detail"] = json!(detail);
            let mut collected = Vec::new();
            for _ in 0..expected.len() {
                let page = payload(
                    call_tool(&context, &state, tool, query.clone())
                        .await
                        .unwrap(),
                );
                assert_eq!(page["total_count"], expected.len());
                assert_eq!(page["detail"], detail);
                assert_eq!(page["state_generation"], full["state_generation"]);
                collected.extend(expanded_rows(&page, key));
                if page["pagination"]["next_cursor"].is_null() {
                    assert_eq!(page["truncated"], false);
                    break;
                }
                assert_eq!(page["truncated"], true);
                query["cursor"] = page["pagination"]["next_cursor"].clone();
            }
            assert_eq!(
                &collected, expected,
                "lost, repeated, or changed {tool} rows in {detail}"
            );
        }
        let mut query = args;
        query["detail"] = json!("compact");
        let compact = payload(call_tool(&context, &state, tool, query).await.unwrap());
        assert_eq!(expanded_rows(&compact, key), *expected);
        assert!(!compact["shared"].as_object().unwrap().is_empty());
    }
}

#[tokio::test]
async fn continuation_rejects_changed_queries_and_stale_snapshots() {
    let (context, state, root) = fixture().await;
    let query = json!({"type_path":"/datum/query_base", "member_name":"work", "limit":1});
    let first = payload(
        call_tool(&context, &state, "dm_find_implementations", query.clone())
            .await
            .unwrap(),
    );
    let cursor = first["pagination"]["next_cursor"]
        .as_str()
        .expect("continuation required");
    let mut next = query.clone();
    next["cursor"] = json!(cursor);
    let mut wrong = next.clone();
    wrong["member_name"] = json!("charge");
    assert!(
        call_tool(&context, &state, "dm_find_implementations", wrong)
            .await
            .is_err()
    );
    assert!(
        call_tool(&context, &state, "dm_find_references", next.clone())
            .await
            .is_err()
    );
    // Presentation and page size do not change the ordered result set.
    next["detail"] = json!("compact");
    next["limit"] = json!(2);
    let rest = payload(
        call_tool(&context, &state, "dm_find_implementations", next.clone())
            .await
            .unwrap(),
    );
    assert_eq!(rest["count"], 2);
    assert!(rest["pagination"]["next_cursor"].is_null());
    payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":root.join("semantics.dme"),"force":true}),
        )
        .await
        .unwrap(),
    );
    assert!(call_tool(&context, &state, "dm_find_implementations", next)
        .await
        .is_err());
    for field in ["cursor", "detail"] {
        for invalid in [json!(true), json!(1), json!("invalid"), json!(null)] {
            let mut args = query.clone();
            args[field] = invalid;
            let result = call_tool(&context, &state, "dm_find_implementations", args).await;
            if let Ok(result) = result {
                assert_eq!(result.is_error, Some(true), "accepted {field}");
                let error = error_payload(result);
                assert_eq!(error["code"], "invalid_input", "{error}");
                assert_eq!(error["details"]["field"], field, "{error}");
            }
        }
    }
}

#[tokio::test]
async fn default_language_page_can_continue_a_large_result_set() {
    let dir = FixtureDirectory::new();
    let mut source = "/datum/page_base\n".to_owned();
    for i in 0..150 {
        source.push_str(&format!("/datum/page_base/child_{i:03}\n"));
    }
    std::fs::write(dir.path().join("fixture.dm"), source).unwrap();
    std::fs::write(dir.path().join("fixture.dme"), "#include \"fixture.dm\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![dir.path().to_owned()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":dir.path().join("fixture.dme")}),
        )
        .await
        .unwrap(),
    );
    let first = payload(
        call_tool(
            &context,
            &state,
            "dm_find_implementations",
            json!({"type_path":"/datum/page_base"}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(first["total_count"], 151);
    assert!(first["count"].as_u64().unwrap() < 151);
    assert_eq!(first["truncated"], true);
    let second = payload(
        call_tool(
            &context,
            &state,
            "dm_find_implementations",
            json!({"type_path":"/datum/page_base", "cursor":first["pagination"]["next_cursor"]}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        first["count"].as_u64().unwrap() + second["count"].as_u64().unwrap(),
        151
    );
    assert_eq!(second["truncated"], false);
    assert_ne!(first["implementations"][0], second["implementations"][0]);
}

#[tokio::test]
async fn oversized_language_results_page_before_the_transport_ceiling() {
    let dir = FixtureDirectory::new();
    let mut source = "/datum/large_page\n".to_owned();
    let mut expected = std::collections::BTreeSet::from(["/datum/large_page".to_owned()]);
    for i in 0..150 {
        let path = format!("/datum/large_page/child_{i:03}_{}", "x".repeat(3000));
        source.push_str(&format!("{path}\n"));
        expected.insert(path);
    }
    std::fs::write(dir.path().join("fixture.dm"), source).unwrap();
    std::fs::write(dir.path().join("fixture.dme"), "#include \"fixture.dm\"\n").unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![dir.path().to_owned()], vec![]).unwrap(),
    );
    let state = ServerState::new();
    payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":dir.path().join("fixture.dme")}),
        )
        .await
        .unwrap(),
    );
    for detail in ["full", "compact"] {
        let mut args = json!({"type_path":"/datum/large_page", "limit":10000, "detail":detail});
        let mut received = std::collections::BTreeSet::new();
        for _ in 0..151 {
            let result = call_tool(&context, &state, "dm_find_implementations", args.clone())
                .await
                .unwrap();
            let ToolContent::Text { text } = &result.content[0];
            assert!(
                text.len() <= 1_048_576,
                "oversized {detail} page: {} bytes",
                text.len()
            );
            let body = payload(result);
            assert_eq!(body["total_count"], 151);
            for row in expanded_rows(&body, "implementations") {
                assert!(
                    received.insert(row["implementation_owner"].as_str().unwrap().to_owned()),
                    "duplicate across pages"
                );
            }
            if body["pagination"]["next_cursor"].is_null() {
                break;
            }
            assert!(body["truncation_reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("language_page_bytes")));
            args["cursor"] = body["pagination"]["next_cursor"].clone();
        }
        assert_eq!(received, expected);
    }
}
