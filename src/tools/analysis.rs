use anyhow::Result;
#[cfg(test)]
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
    let Some(ty) = snapshot.objtree.find(&args.type_path) else {
        return Ok(ToolResult::error(format!(
            "Type not found: {}",
            args.type_path
        )));
    };
    let mut data = crate::outputs::DefinitionData {
        kind: "type",
        name: None,
        type_path: None,
        path: Some(ty.path.to_string()),
        defined_in: None,
        file: get_file_path(&snapshot.context, ty.location.file),
        line: ty.location.line,
        column: ty.location.column,
        declaration_kind: "type",
        resolved_type_owner: ty.path.to_string(),
        implementation_owner: None,
        declaration_owner: None,
        resolution_kind: None,
        resolution_diagnostics: None,
        state_generation: snapshot.generation,
        spacemandmm_revision: snapshot.spacemandmm_revision,
    };
    if let Some(member) = args.member_name.as_deref() {
        let variable = ty.iter_parent_types().find_map(|owner| {
            owner
                .get()
                .vars
                .get(member)
                .filter(|v| v.declaration.is_some())
                .map(|v| (owner, v))
        });
        let procedure = match snapshot.proc_resolver().view(&args.type_path, member) {
            Ok(view) => Some(view),
            Err(error @ crate::proc_resolution::ProcResolutionError::HierarchyLimit) => {
                return Ok(crate::result::structured_error(
                    crate::result::ToolErrorCode::LimitExceeded,
                    error.to_string(),
                    None,
                    serde_json::to_value(error)?,
                ))
            }
            Err(_) => None,
        };
        if variable.is_some() && procedure.is_some() {
            return Ok(ToolResult::error(format!(
                "Ambiguous member {}/{member}: both variable and procedure declarations exist",
                args.type_path
            )));
        }
        data.name = Some(member.into());
        data.type_path = Some(args.type_path.clone());
        data.path = None;
        if let Some(mut procedure) = procedure {
            let first = procedure
                .implementations
                .next()
                .expect("resolved implementation");
            data.kind = "proc";
            data.declaration_kind = "proc";
            data.defined_in = Some(procedure.implementation_owner.into());
            data.resolved_type_owner = procedure.implementation_owner.into();
            data.implementation_owner = Some(procedure.implementation_owner.into());
            data.declaration_owner = Some(procedure.declaration_owner.into());
            data.resolution_kind = Some(procedure.resolution_kind);
            data.resolution_diagnostics =
                Some(if procedure.implementation_owner == args.type_path {
                    Vec::new()
                } else {
                    vec![format!(
                        "requested type inherits the implementation from {}",
                        procedure.implementation_owner
                    )]
                });
            data.file = first.location.file.clone();
            data.line = first.location.line;
            data.column = first.location.column;
        } else if let Some((owner, var)) = variable {
            data.kind = "var";
            data.declaration_kind = "var";
            data.defined_in = Some(owner.path.to_string());
            data.resolved_type_owner = owner.path.to_string();
            data.file = get_file_path(&snapshot.context, var.value.location.file);
            data.line = var.value.location.line;
            data.column = var.value.location.column;
        } else {
            return Ok(ToolResult::error(format!(
                "Member not found: {}/{member}",
                args.type_path
            )));
        }
    }
    crate::result::analysis_text(&snapshot, data)
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
