use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

const STREAM_JSON_BYTES: usize = 64 * 1024;
const DIAGNOSTIC_JSON_BYTES: usize = 96 * 1024;
const METADATA_JSON_BYTES: usize = 96 * 1024;
const REPLY_JSON_BYTES: usize = 512 * 1024;

pub(super) struct ResponseOptions {
    include_output: bool,
    output_max_bytes: usize,
    diagnostic_limit: usize,
}

impl ResponseOptions {
    pub fn parse(args: &Value) -> Result<Self> {
        Ok(Self {
            include_output: match args.get("include_output") {
                None => true,
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| anyhow!("include_output must be a boolean"))?,
            },
            output_max_bytes: bounded_integer(args, "output_max_bytes", 8192, 1, 65536)?,
            diagnostic_limit: bounded_integer(args, "diagnostic_limit", 50, 0, 200)?,
        })
    }
}

fn bounded_integer(args: &Value, name: &str, default: u64, min: u64, max: u64) -> Result<usize> {
    let value = match args.get(name) {
        None => default,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow!("{name} must be an integer"))?,
    };
    anyhow::ensure!((min..=max).contains(&value), "{name} must be {min}..={max}");
    Ok(value as usize)
}

// serde_json uses these escapes for string contents. Count UTF-8 and JSON bytes
// separately, since a captured control byte can become six response bytes.
fn json_char_bytes(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
        '\0'..='\u{1f}' => 6,
        _ => ch.len_utf8(),
    }
}

fn bounded_text(text: &str, raw_limit: usize, json_limit: usize, tail: bool) -> &str {
    let mut raw_bytes = 0;
    let mut json_bytes = 2; // quotes
    let mut add = |ch: char| {
        if raw_bytes + ch.len_utf8() > raw_limit || json_bytes + json_char_bytes(ch) > json_limit {
            return false;
        }
        raw_bytes += ch.len_utf8();
        json_bytes += json_char_bytes(ch);
        true
    };
    if tail {
        for ch in text.chars().rev() {
            if !add(ch) {
                break;
            }
        }
        &text[text.len() - raw_bytes..]
    } else {
        for ch in text.chars() {
            if !add(ch) {
                break;
            }
        }
        &text[..raw_bytes]
    }
}

fn json_bytes(value: &Value) -> usize {
    // Values constructed here contain no fallible user-defined serializers.
    serde_json::to_vec(value)
        .expect("JSON value serialization")
        .len()
}

fn diagnostics(rows: Value, limit: usize) -> (Value, Value) {
    let rows = rows.as_array().expect("compiler diagnostic array");
    let mut returned = Vec::new();
    let mut bytes = 2;
    let mut truncated_messages = 0;
    for row in rows.iter().take(limit) {
        let mut row = row.clone();
        let message = row["message"]
            .as_str()
            .expect("compiler diagnostic message");
        let excerpt = bounded_text(message, 4096, 4096, false);
        let truncated = excerpt.len() != message.len();
        if truncated {
            let original_bytes = message.len();
            row["message"] = json!(excerpt);
            row["message_truncated"] = json!(true);
            row["message_utf8_bytes"] = json!(original_bytes);
        }
        let row_bytes = json_bytes(&row) + usize::from(!returned.is_empty());
        if bytes + row_bytes > DIAGNOSTIC_JSON_BYTES {
            break;
        }
        bytes += row_bytes;
        truncated_messages += usize::from(truncated);
        returned.push(row);
    }
    let summary = json!({
        "returned": returned.len(), "omitted": rows.len() - returned.len(),
        "truncated_messages": truncated_messages,
    });
    (json!(returned), summary)
}

fn remove_pointer(result: &mut Value, pointer: &str) -> Value {
    let (parent, name) = pointer.rsplit_once('/').expect("JSON pointer");
    result
        .pointer_mut(parent)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove(name)
        .unwrap()
}

fn bound_metadata(result: &mut Value) {
    let mut omissions = Map::new();
    // Paths are either exact or explicitly omitted; never manufacture a shortened
    // path. Keep artifact hashes/existence even when an artifact path is omitted.
    for pointer in [
        "/dme_argument",
        "/spawn_working_directory",
        "/dmb_path",
        "/compiler",
        "/working_directory",
        "/artifact_before/path",
        "/artifact_after/path",
        "/network_audit/warning",
    ] {
        if let Some(value) = result.pointer(pointer) {
            let bytes = json_bytes(value);
            if bytes > 8192 {
                remove_pointer(result, pointer);
                omissions.insert(
                    pointer.into(),
                    json!({"field_omitted":true,"json_bytes":bytes}),
                );
            }
        }
    }
    for pointer in [
        "/defines",
        "/provenance_reasons",
        "/network_audit/observations",
    ] {
        let Some(rows) = result.pointer_mut(pointer).and_then(Value::as_array_mut) else {
            continue;
        };
        let total = rows.len();
        let mut bytes = 2;
        let keep = rows
            .iter()
            .take_while(|row| {
                bytes += json_bytes(row) + 1;
                bytes <= 16 * 1024
            })
            .count();
        rows.truncate(keep);
        if keep != total {
            omissions.insert(
                pointer.into(),
                json!({"total":total,"returned":keep,"omitted":total-keep}),
            );
        }
    }
    // Bound the combined metadata too, including repeated long paths. Only these
    // optional fields may be removed; outcome, hashes and provenance status stay.
    while json_bytes(result) > METADATA_JSON_BYTES - 4096 {
        let largest = [
            "dme_argument",
            "spawn_working_directory",
            "dmb_path",
            "compiler",
            "working_directory",
            "defines",
            "provenance_reasons",
            "network_audit",
        ]
        .into_iter()
        .filter_map(|key| result.get(key).map(|value| (key, json_bytes(value))))
        .max_by_key(|(_, bytes)| *bytes);
        let Some((key, bytes)) = largest else { break };
        result.as_object_mut().unwrap().remove(key);
        omissions.insert(
            format!("/{key}"),
            json!({"field_omitted":true,"json_bytes":bytes}),
        );
    }
    if !omissions.is_empty() {
        result["response_omissions"] = Value::Object(omissions);
    }
}

pub(super) fn format(mut result: Value, options: ResponseOptions) -> Result<String> {
    let mut output = Map::new();
    let mut output_summary = Map::new();
    for name in ["stdout", "stderr"] {
        let value = result.as_object_mut().unwrap().remove(name).unwrap();
        let text = value.as_str().expect("compiler output string");
        let excerpt = if options.include_output {
            bounded_text(text, options.output_max_bytes, STREAM_JSON_BYTES, true)
        } else {
            ""
        };
        output_summary.insert(name.into(), json!({
            "included": options.include_output, "available_utf8_bytes": text.len(),
            "returned_utf8_bytes": excerpt.len(), "omitted_utf8_bytes": text.len() - excerpt.len(),
        }));
        if options.include_output {
            output.insert(name.into(), json!(excerpt));
        }
    }
    let mut diagnostic_summary = json!({
        "capture_complete": result["stdout_truncated_bytes"] == 0 && result["stderr_truncated_bytes"] == 0,
        "scope": "captured_output",
    });
    let mut diagnostic_rows = Map::new();
    for name in ["errors", "warnings"] {
        let rows = result.as_object_mut().unwrap().remove(name).unwrap();
        diagnostic_summary[name] = json!(rows.as_array().unwrap().len());
        let (rows, summary) = diagnostics(rows, options.diagnostic_limit);
        diagnostic_summary[format!("{name}_detail")] = summary;
        diagnostic_rows.insert(name.into(), rows);
    }
    bound_metadata(&mut result);
    result.as_object_mut().unwrap().extend(output);
    result.as_object_mut().unwrap().extend(diagnostic_rows);
    result["output_summary"] = Value::Object(output_summary);
    result["diagnostic_summary"] = diagnostic_summary;
    let text = serde_json::to_string(&result)?;
    debug_assert!(text.len() <= REPLY_JSON_BYTES, "compiler response budget");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_budgets_preserve_outcome_with_large_metadata_and_diagnostics() {
        let diagnostic = json!({"file":"fixture.dm","line":7,"severity":"error","message":"\u{1}🛰".repeat(2000)});
        let mut result = json!({
            "success":false,"compiler_succeeded":false,"dmb_exists":true,"dmb_updated":true,
            "artifact_before":{"path":"p".repeat(9000),"exists":false,"sha256":null},
            "artifact_after":{"path":"p".repeat(9000),"exists":true,"sha256":"abc","size":19},
            "build_record_id":"record","provenance_status":"stale","retained_dmb_sha256":"abc",
            "stdout":"\u{1}".repeat(524288),"stderr":"🛰\n".repeat(100000),
            "stdout_truncated_bytes":0,"stderr_truncated_bytes":0,
            "errors":vec![diagnostic.clone();200],"warnings":vec![diagnostic;200],
            "defines":vec!["x".repeat(1000);100],"provenance_reasons":vec!["y".repeat(1000);100],
            "network_audit":{"observations":vec![json!({"local_endpoint":"x".repeat(1000)});256]},
        });
        for field in [
            "dme_argument",
            "spawn_working_directory",
            "dmb_path",
            "compiler",
            "working_directory",
        ] {
            result[field] = json!("p".repeat(8000));
        }
        let text = format(
            result,
            ResponseOptions::parse(&json!({"output_max_bytes":65536,"diagnostic_limit":200}))
                .unwrap(),
        )
        .unwrap();
        assert!(text.len() <= REPLY_JSON_BYTES);
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["success"], false);
        assert_eq!(body["dmb_exists"], true);
        assert_eq!(body["artifact_after"]["sha256"], "abc");
        assert_eq!(body["artifact_after"]["size"], 19);
        assert!(body["artifact_after"].get("path").is_none());
        assert_eq!(
            body["response_omissions"]["/artifact_after/path"]["field_omitted"],
            true
        );
        assert_eq!(body["provenance_status"], "stale");
        assert_eq!(body["build_record_id"], "record");
        for name in ["errors", "warnings"] {
            let count = body[name].as_array().unwrap().len();
            assert!((1..200).contains(&count));
            assert_eq!(body["diagnostic_summary"][name], 200);
            let detail = &body["diagnostic_summary"][format!("{name}_detail")];
            assert_eq!(detail["returned"], count);
            assert_eq!(detail["omitted"], 200 - count);
            assert_eq!(detail["truncated_messages"], count);
        }
    }
}
