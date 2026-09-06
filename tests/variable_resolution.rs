use meridian_mcp::result::ToolContent;
use meridian_mcp::state::ServerState;
use meridian_mcp::tools::{call_tool, ToolExecutionContext};
use meridian_mcp::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};

fn payload(result: meridian_mcp::result::ToolResult) -> Value {
    let ToolContent::Text { text } = &result.content[0];
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn variables_resolve_values_and_declarations_through_semantic_parents() {
    let root = std::env::temp_dir().join(format!("meridian-var-resolution-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(root.join("fixture.dme"), "#include \"fixture.dm\"\n").unwrap();
    std::fs::write(
        root.join("fixture.dm"),
        "/datum/cell\n\t/// Stored charge.\n\tvar/charge = 7\n\tvar/list/items = list()\n\
         /datum/cell/special\n\tcharge = 40\n\titems = null\n\
         /datum/alias\n\tparent_type = /datum/cell/special\n\
         /datum/cell/redirected\n\tparent_type = /datum\n",
    )
    .unwrap();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root.clone()], Vec::new()).unwrap(),
    );
    let state = ServerState::new();
    let parsed = payload(
        call_tool(
            &context,
            &state,
            "dm_parse_environment",
            json!({"dme_path":root.join("fixture.dme")}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(parsed["success"], true, "{parsed:#}");

    for (requested, owner, value, declared) in [
        ("/datum/cell", "/datum/cell", "Float(7.0)", true),
        (
            "/datum/cell/special",
            "/datum/cell/special",
            "Float(40.0)",
            false,
        ),
        ("/datum/alias", "/datum/cell/special", "Float(40.0)", false),
    ] {
        let result = call_tool(
            &context,
            &state,
            "dm_get_var",
            json!({"type_path":requested,"var_name":"charge"}),
        )
        .await
        .unwrap();
        assert_eq!(result.is_error, None, "{result:?}");
        let body = payload(result);
        assert_eq!(body["constant"], value, "{body:#}");
        assert_eq!(body["value_owner"], owner, "{body:#}");
        assert_eq!(body["declaration_owner"], "/datum/cell");
        assert_eq!(body["declared"], declared);
        assert_eq!(body["inherited"], requested != owner);
        assert_eq!(body["state_generation"], 1);
        assert!(body["documentation"]
            .as_str()
            .unwrap()
            .contains("Stored charge"));
        let definition = payload(
            call_tool(
                &context,
                &state,
                "dm_get_definition",
                json!({"type_path":requested,"member_name":"charge"}),
            )
            .await
            .unwrap(),
        );
        assert_eq!(body["declaration_owner"], definition["defined_in"]);
    }

    let items = payload(
        call_tool(
            &context,
            &state,
            "dm_get_var",
            json!({"type_path":"/datum/alias","var_name":"items"}),
        )
        .await
        .unwrap(),
    );
    assert!(items["declared_type"].as_str().unwrap().contains("list"));
    assert!(items["constant"].as_str().unwrap().starts_with("Null"));
    assert_eq!(items["value_owner"], "/datum/cell/special");
    assert_ne!(items["location"], items["declaration_location"]);

    let builtin = payload(
        call_tool(
            &context,
            &state,
            "dm_get_var",
            json!({"type_path":"/datum/alias","var_name":"type"}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(builtin["name"], "type", "{builtin:#}");
    assert!(builtin["declaration_owner"].is_string());

    for requested in ["/datum/cell/redirected", "/datum/missing"] {
        let result = call_tool(
            &context,
            &state,
            "dm_get_var",
            json!({"type_path":requested,"var_name":"charge"}),
        )
        .await
        .unwrap();
        assert_eq!(result.is_error, Some(true));
    }
    std::fs::remove_dir_all(root).unwrap();
}
