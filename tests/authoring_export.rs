use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &[u8]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "meridian-authoring-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("tgstation.dme"), "#include \"fixture.dm\"\n").unwrap();
        std::fs::write(root.join("fixture.dm"), source).unwrap();
        Self(root)
    }
    fn invoke(&self, output: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_meridian-mcp"))
            .args(["authoring-export", "--project"])
            .arg(&self.0)
            .arg("--output")
            .arg(output)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
    fn export(&self) -> Value {
        let output = self.0.join("export.json");
        let result = self.invoke(&output);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn definition<'a>(export: &'a Value, path: &str) -> &'a Value {
    export["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["type_path"] == path)
        .unwrap()
}
fn field<'a>(definition: &'a Value, name: &str) -> &'a Value {
    definition["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap()
}
fn source_bytes<'a>(source: &Value, bytes: &'a [u8]) -> &'a [u8] {
    assert_eq!(source["path"], "fixture.dm");
    assert_eq!(source["sha256"], format!("{:x}", Sha256::digest(bytes)));
    &bytes[source["start"].as_u64().unwrap() as usize..source["end"].as_u64().unwrap() as usize]
}

#[test]
fn exports_effective_values_physical_expressions_and_semantic_membership() {
    let source = b"#define ACCESS_A 1\n/datum/job\n\tvar/title = \"Base\"\n\tvar/list/access = list(ACCESS_A, 2)\n\tvar/outfit = /datum/outfit/base\n/datum/job/child\n\ttitle = \"Child\"\n/datum/redirected\n\tparent_type = /datum/job\n/datum/outfit\n\tvar/uniform = /obj/item/uniform\n/datum/outfit/base\n/obj/item\n/obj/item/uniform\n";
    let fixture = Fixture::new(source);
    let export = fixture.export();
    assert_eq!(export["schema_version"], 1);
    assert_eq!(definition(&export, "/datum/redirected")["kind"], "job");
    let child = definition(&export, "/datum/job/child");
    assert_eq!(field(child, "title")["value"], "Child");
    assert_eq!(field(child, "title")["expression"], "\"Child\"");
    assert_eq!(
        source_bytes(&field(child, "title")["source"], source),
        b"\"Child\""
    );
    assert_eq!(field(child, "access")["owner_type"], "/datum/job");
    assert_eq!(field(child, "access")["value"], serde_json::json!([1, 2]));
    assert_eq!(field(child, "access")["expression"], "list(ACCESS_A, 2)");
    assert_eq!(export["input_files"].as_array().unwrap().len(), 2);
    assert_eq!(
        fixture.export(),
        export,
        "unchanged sources must export deterministically"
    );
}

#[test]
fn retains_repeated_assignments_and_complete_procedures_in_physical_bytes() {
    let mut source = b"\xef\xbb\xbf/datum/job\r\n\tvar/title = \"First\"\r\n/datum/job\r\n\ttitle = \"Final\"\r\n/datum/job/proc/after_spawn()\r\n".to_vec();
    for _ in 0..220 {
        source.extend_from_slice(b"\t// keep this physical comment\r\n");
    }
    source.extend_from_slice(b"\treturn title\r\n/datum/job/next\r\n\ttitle = \"Next\"\r\n");
    let fixture = Fixture::new(&source);
    let export = fixture.export();
    let job = definition(&export, "/datum/job");
    let title = field(job, "title");
    assert_eq!(title["occurrences"].as_array().unwrap().len(), 2);
    assert_eq!(source_bytes(&title["source"], &source), b"\"Final\"");
    assert_eq!(job["occurrences"].as_array().unwrap().len(), 2);
    let procedure = job["procedures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "after_spawn")
        .unwrap();
    let text = procedure["text"].as_str().unwrap();
    assert!(text.starts_with("/datum/job/proc/after_spawn()"));
    assert_eq!(text.matches("keep this physical comment").count(), 220);
    assert!(!text.contains("/datum/job/next"));
    assert_eq!(source_bytes(&procedure["source"], &source), text.as_bytes());
    assert_eq!(procedure["editable"], true);
}

#[test]
fn reference_candidates_distinguish_physical_literals_and_macro_expansions() {
    let source = b"#define BASE_OUTFIT /datum/outfit/base\n/datum/outfit\n/datum/outfit/base\n/datum/job\n\tvar/outfit = /datum/outfit/base\n\tvar/macro_outfit = BASE_OUTFIT\n\tvar/text = \"/datum/outfit/base\"\n/datum/job/proc/pick_outfit()\n\treturn /datum/outfit/base\n";
    let fixture = Fixture::new(source);
    let export = fixture.export();
    let references = definition(&export, "/datum/job")["references"]
        .as_array()
        .unwrap();
    let editable = references
        .iter()
        .filter(|v| v["target_type"] == "/datum/outfit/base" && v["editable"] == true)
        .collect::<Vec<_>>();
    assert_eq!(editable.len(), 2, "{references:?}");
    for reference in editable {
        assert_eq!(
            source_bytes(&reference["source"], source),
            b"/datum/outfit/base"
        );
    }
    assert!(references.iter().any(|v| v["editable"] == false
        && v["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("macro"))));
    assert!(export["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap().contains("macro")));
}

#[test]
fn rejects_external_includes_without_publishing_partial_export() {
    let fixture = Fixture::new(b"/datum/job\n");
    let outside = fixture.0.with_extension("outside.dm");
    std::fs::write(&outside, "/datum/outside\n").unwrap();
    std::fs::write(
        fixture.0.join("tgstation.dme"),
        format!(
            "#include \"{}\"\n",
            outside.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let output = fixture.0.join("export.json");
    let result = fixture.invoke(&output);
    std::fs::remove_file(outside).unwrap();
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
fn refuses_to_overwrite_project_sources() {
    let source = b"/datum/job\n";
    let fixture = Fixture::new(source);
    assert!(!fixture
        .invoke(&fixture.0.join("fixture.dm"))
        .status
        .success());
    assert_eq!(std::fs::read(fixture.0.join("fixture.dm")).unwrap(), source);
}

#[test]
fn parser_errors_do_not_publish_a_writable_catalog() {
    let fixture = Fixture::new(b"/datum/job\n\tvar/title = list(\n");
    let output = fixture.0.join("export.json");
    let result = fixture.invoke(&output);
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
fn initializer_spans_preserve_trailing_comments_and_input_specifiers() {
    let source = b"/datum/job\n\tvar/title = \"Crew // name\" // keep this comment\n\tvar/numeric = 3 as num\n\tvar/list/access = list(1, /* inside list */ 2) /* keep tail */\n";
    let fixture = Fixture::new(source);
    let export = fixture.export();
    let job = definition(&export, "/datum/job");
    assert_eq!(
        source_bytes(&field(job, "title")["source"], source),
        b"\"Crew // name\""
    );
    assert_eq!(source_bytes(&field(job, "numeric")["source"], source), b"3");
    assert_eq!(
        source_bytes(&field(job, "access")["source"], source),
        b"list(1, /* inside list */ 2)"
    );
}

#[test]
fn migration_includes_related_owners_and_items_project_appearance_only() {
    let source = b"/datum/job\n/datum/outfit\n/datum/outfit/base\n/datum/id_trim\n/datum/controller\n\tvar/selected = /datum/outfit/base\n/obj/item\n\tvar/name = \"Item\"\n\tvar/icon_state = \"base\"\n\tvar/slot_flags = 4\n\tvar/internal_runtime_value = 9\n/obj/item/child\n\tname = \"Child\"\n/obj/item/proc/runtime_only()\n\treturn 1\n";
    let fixture = Fixture::new(source);
    let export = fixture.export();
    let controller = definition(&export, "/datum/controller");
    assert_eq!(controller["references"].as_array().unwrap().len(), 1);
    assert_eq!(controller["references"][0]["editable"], true);
    let item = definition(&export, "/obj/item/child");
    assert_eq!(field(item, "icon_state")["value"], "base");
    assert_eq!(field(item, "slot_flags")["value"], 4);
    assert!(!item["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == "internal_runtime_value"));
    assert!(item["procedures"].as_array().unwrap().is_empty());
}

#[test]
fn exports_qualified_constants_and_explicit_analysis_configuration() {
    let source = b"#define ITEM_SLOT_HEAD (1<<6) // preserve note\n#define SLOT_HELPER(x) (1<<(x))\n#define ITEM_SLOT_MASK SLOT_HELPER(5)\n#define JOB_CREW_MEMBER 1\n#define JOB_CREW_MEMBER 4\n#define JOB_ALIAS (JOB_CREW_MEMBER | 2)\n#define ACCESS_COMMAND \"command\"\n#define PAYCHECK_COMMAND 100\n#define DEPARTMENT_COMMAND \"Command\"\n#define HEAD (1<<0)\n#define JOB_UNRESOLVED unknown_call()\n#define JOB_REMOVED 9\n#undef JOB_REMOVED\n/datum/job\n\tvar/list/aliases = list(\"A\", \"B\")\n/obj/item\n\tvar/icon = 'icons/foo.dmi'\n\tvar/slot_flags = ITEM_SLOT_HEAD | ITEM_SLOT_MASK\n";
    let fixture = Fixture::new(source);
    std::fs::write(
        fixture.0.join("SpacemanDMM.toml"),
        "environment = \"tgstation.dme\"\n",
    )
    .unwrap();
    let export = fixture.export();
    let constants = export["constants"]
        .as_array()
        .expect("qualified constant catalog");
    let lookup = |name: &str| {
        constants
            .iter()
            .find(|constant| constant["name"] == name)
            .unwrap()
    };
    assert_eq!(lookup("ITEM_SLOT_HEAD")["value"], 64);
    assert_eq!(lookup("ITEM_SLOT_HEAD")["expression"], "(1<<6)");
    assert_eq!(
        source_bytes(&lookup("ITEM_SLOT_HEAD")["source"], source),
        b"(1<<6)"
    );
    assert_eq!(lookup("ITEM_SLOT_MASK")["value"], 32);
    assert_eq!(lookup("JOB_CREW_MEMBER")["value"], 4);
    assert_eq!(lookup("JOB_ALIAS")["value"], 6);
    assert_eq!(lookup("ACCESS_COMMAND")["value"], "command");
    assert_eq!(lookup("PAYCHECK_COMMAND")["value"], 100);
    assert_eq!(lookup("DEPARTMENT_COMMAND")["value"], "Command");
    assert_eq!(lookup("HEAD")["value"], 1);
    assert_eq!(lookup("JOB_UNRESOLVED")["value_known"], false);
    assert!(!constants
        .iter()
        .any(|constant| constant["name"] == "JOB_REMOVED"));
    assert_eq!(
        field(definition(&export, "/obj/item"), "icon")["value"],
        "icons/foo.dmi"
    );
    assert_eq!(
        field(definition(&export, "/obj/item"), "slot_flags")["value"],
        96
    );
    assert_eq!(
        field(definition(&export, "/datum/job"), "aliases")["value"],
        serde_json::json!(["A", "B"])
    );
    assert_eq!(export["configuration"]["profile"], "spacemandmm-default");
    assert_eq!(
        export["configuration"]["command_line_defines"],
        serde_json::json!([])
    );
    assert_eq!(export["configuration"]["config_file"], "SpacemanDMM.toml");
    assert_eq!(
        export["configuration"]["when_compile_defined_at_end"],
        false
    );
    assert_eq!(
        export["configuration"]["builtin_defines"]["SPACEMAN_DMM"],
        1
    );
    assert_eq!(export["input_files"].as_array().unwrap().len(), 3);
}

#[test]
fn context_dependent_macro_aliases_remain_explicitly_unknown() {
    let fixture = Fixture::new(
        b"#define JOB_SOURCE __FILE__\n#define JOB_SOURCE_ALIAS JOB_SOURCE\n/datum/job\n",
    );
    let export = fixture.export();
    for name in ["JOB_SOURCE", "JOB_SOURCE_ALIAS"] {
        let constant = export["constants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|constant| constant["name"] == name)
            .unwrap();
        assert_eq!(constant["value_known"], false, "{constant}");
        assert_eq!(constant["value"], Value::Null);
    }
}

#[test]
fn ambient_globals_are_not_projected_as_inherited_instance_fields() {
    let source = b"var/const/ambient_global = 7\nvar/body_variation_flags = 9\n/proc/global_helper()\n\treturn 7\n/datum\n\tvar/inherited_member = 3\n/datum/proc/local_helper()\n\treturn inherited_member\n/datum/job\n\tvar/derived_member = ambient_global\n/obj/item\n\tvar/name = \"Item\"\n";
    let fixture = Fixture::new(source);
    let export = fixture.export();
    let job = definition(&export, "/datum/job");
    assert!(!job["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["name"] == "ambient_global"));
    assert!(!job["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["name"] == "body_variation_flags"));
    assert_eq!(field(job, "inherited_member")["owner_type"], "/datum");
    assert_eq!(field(job, "inherited_member")["value"], 3);
    assert_eq!(field(job, "derived_member")["value"], 7);
    assert!(!definition(&export, "/obj/item")["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["name"] == "body_variation_flags"));
    assert!(job["procedures"]
        .as_array()
        .unwrap()
        .iter()
        .any(|proc| proc["name"] == "local_helper"));
    assert!(!job["procedures"]
        .as_array()
        .unwrap()
        .iter()
        .any(|proc| proc["name"] == "global_helper"));
}
