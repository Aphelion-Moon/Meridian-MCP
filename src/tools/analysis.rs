use anyhow::Result;
use serde_json::json;

use crate::analysis_snapshot::AnalysisContext;
use crate::mcp::ToolResult;
use crate::state::ServerState;

/// Helper to get file path string from a location
fn get_file_path(context: &AnalysisContext, file_id: dreammaker::FileId) -> String {
    context.file_path(file_id).display().to_string()
}

/// Get definition location for a symbol
pub async fn get_definition(
    state: &ServerState,
    args: crate::parameters::GetDefinitionParams,
) -> Result<ToolResult> {
    let snapshot = state.snapshot().await?;
    let objtree = &snapshot.objtree;
    let context = &snapshot.context;

    let type_path = args.type_path.as_str();

    let member_name = args.member_name.as_deref();

    match objtree.find(type_path) {
        Some(ty) => {
            if let Some(member) = member_name {
                let mut current = Some(ty);
                let mut variable = None;
                while let Some(t) = current {
                    if let Some(var) = t.vars.get(member) {
                        if var.declaration.is_some() {
                            variable = Some((
                                t.path.to_string(),
                                get_file_path(context, var.value.location.file),
                                var.value.location.line,
                                var.value.location.column,
                            ));
                            break;
                        }
                    }
                    current = t.parent_type();
                }

                let procedure = snapshot.proc_resolver().resolve(type_path, member).ok();
                if variable.is_some() && procedure.is_some() {
                    return Ok(ToolResult::error(format!(
                        "Ambiguous member {type_path}/{member}: both variable and procedure declarations exist"
                    )));
                }
                if let Some(resolution) = procedure {
                    let first = resolution
                        .implementations
                        .first()
                        .expect("a resolved procedure has an implementation");
                    let result = json!({
                        "kind": "proc",
                        "name": member,
                        "type_path": type_path,
                        "defined_in": resolution.implementation_owner,
                        "file": first.location.file,
                        "line": first.location.line,
                        "column": first.location.column,
                        "declaration_kind": "proc",
                        "resolved_type_owner": resolution.implementation_owner,
                        "implementation_owner": resolution.implementation_owner,
                        "declaration_owner": resolution.declaration_owner,
                        "resolution_kind": resolution.resolution_kind,
                        "resolution_diagnostics": resolution.diagnostics(),
                        "state_generation": snapshot.generation,
                        "spacemandmm_revision": snapshot.spacemandmm_revision,
                    });
                    return crate::result::analysis_text(&snapshot, result);
                }
                if let Some((owner, file_path, line, column)) = variable {
                    let result = json!({
                        "kind": "var",
                        "name": member,
                        "type_path": type_path,
                        "defined_in": owner,
                        "file": file_path,
                        "line": line,
                        "column": column,
                        "declaration_kind": "var",
                        "resolved_type_owner": owner,
                        "state_generation": snapshot.generation,
                        "spacemandmm_revision": snapshot.spacemandmm_revision,
                    });
                    return crate::result::analysis_text(&snapshot, result);
                }

                Ok(ToolResult::error(format!(
                    "Member not found: {type_path}/{member}"
                )))
            } else {
                // Just get the type definition
                let file_path = get_file_path(context, ty.location.file);
                let result = json!({
                    "kind": "type",
                    "path": ty.path,
                    "file": file_path,
                    "line": ty.location.line,
                    "column": ty.location.column
                    ,"declaration_kind": "type",
                    "resolved_type_owner": ty.path,
                    "state_generation": snapshot.generation,
                    "spacemandmm_revision": snapshot.spacemandmm_revision
                });
                crate::result::analysis_text(&snapshot, result)
            }
        }
        None => Ok(ToolResult::error(format!("Type not found: {type_path}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::ToolContent;
    use crate::tools::parse::parse_environment;
    use serde_json::Value;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn result_json(result: &ToolResult) -> Value {
        let ToolContent::Text { text } = &result.content[0];
        serde_json::from_str(text).expect("tool result should be JSON")
    }

    async fn inherited_definition_fixture() -> (std::path::PathBuf, ServerState) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "meridian-mcp-definition-{}-{unique}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let dme_path = directory.join("fixture.dme");
        std::fs::write(&dme_path, "#include \"fixture.dm\"\n").unwrap();
        std::fs::write(
            directory.join("fixture.dm"),
            r#"/datum/definition_parent
	var/inherited_value = 1

/datum/definition_parent/proc/inherited_proc()
	return inherited_value

/datum/definition_parent/child
	inherited_value = 2
"#,
        )
        .unwrap();

        let state = ServerState::new();
        let parse_result = parse_environment(&state, json!({"dme_path": dme_path}))
            .await
            .unwrap();
        assert_eq!(parse_result.is_error, None);
        (directory, state)
    }

    #[tokio::test]
    async fn inherited_member_definitions_report_the_declaring_type() {
        let (directory, state) = inherited_definition_fixture().await;

        for (member, kind) in [("inherited_value", "var"), ("inherited_proc", "proc")] {
            let result = get_definition(
                &state,
                crate::parameters::decode(json!({
                    "type_path": "/datum/definition_parent/child",
                    "member_name": member,
                }))
                .expect("valid fixture request"),
            )
            .await
            .unwrap();
            let payload = result_json(&result);
            assert_eq!(payload["kind"], kind);
            assert_eq!(payload["defined_in"], "/datum/definition_parent");
            assert_eq!(payload["type_path"], "/datum/definition_parent/child");
        }

        std::fs::remove_dir_all(directory).unwrap();
    }
}
