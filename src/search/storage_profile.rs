//! Read-only retained-search-storage probe for a real DreamMaker environment.
//! Run with MERIDIAN_SCALE_DME and --ignored --nocapture in a release build.
//! Payload counts exclude allocator metadata, hash buckets and the parser AST.

use super::{DocumentIds, Posting, SearchDocument};
use crate::result::ToolContent;
use crate::state::ServerState;
use crate::tools::{call_tool, ToolExecutionContext};
use crate::{CapabilityMode, PathPolicy};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::mem::size_of;

fn text_storage<'a>(strings: impl Iterator<Item = &'a str>) -> Value {
    let mut occurrences = 0;
    let mut logical_bytes = 0;
    let mut allocations = HashSet::new();
    let mut allocated_payload_bytes = 0;
    let mut unique_text = HashSet::new();
    for text in strings.filter(|text| !text.is_empty()) {
        occurrences += 1;
        logical_bytes += text.len();
        if allocations.insert((text.as_ptr(), text.len())) {
            allocated_payload_bytes += text.len();
        }
        unique_text.insert(text);
    }
    json!({
        "nonempty_occurrences": occurrences,
        "logical_bytes": logical_bytes,
        "distinct_allocation_payload_bytes": allocated_payload_bytes,
        "distinct_allocations": allocations.len(),
        "unique_text_bytes": unique_text.iter().map(|text| text.len()).sum::<usize>(),
        "unique_text_count": unique_text.len(),
    })
}

#[tokio::test]
#[ignore = "requires a full DreamMaker environment via MERIDIAN_SCALE_DME"]
async fn retained_search_storage_profile() {
    let dme = std::path::PathBuf::from(
        std::env::var("MERIDIAN_SCALE_DME").expect("set MERIDIAN_SCALE_DME to a .dme path"),
    );
    let root = dme.parent().unwrap().to_path_buf();
    let context = ToolExecutionContext::new(
        CapabilityMode::Analysis,
        PathPolicy::new(vec![root], Vec::new()).unwrap(),
    );
    let state = ServerState::new();
    let parsed = call_tool(
        &context,
        &state,
        "dm_parse_environment",
        json!({"dme_path": dme}),
    )
    .await
    .unwrap();
    assert_eq!(parsed.is_error, None);
    let ToolContent::Text { text } = &parsed.content[0];
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["success"], true);
    let snapshot = state.snapshot().await.unwrap();
    let index = &snapshot.search_index;
    let documents = &index.documents;
    let mut metadata: Vec<&str> = Vec::new();
    for document in documents {
        metadata.extend::<[&str; 4]>([
            &document.symbol,
            &document.name,
            &document.type_path,
            &document.file,
        ]);
        metadata.extend(document.implementation_owner.as_deref());
        metadata.extend(document.declaration_owner.as_deref());
        metadata.extend(document.parent.as_deref());
        metadata.extend(
            document
                .parameters
                .iter()
                .map(|parameter| -> &str { parameter }),
        );
    }
    let result = json!({
        "types": snapshot.total_types,
        "documents": documents.len(),
        "timings_ms": parsed["timings_ms"],
        "document_slot_bytes": size_of::<SearchDocument>(),
        "document_capacity": documents.capacity(),
        "document_capacity_bytes": documents.capacity() * size_of::<SearchDocument>(),
        "metadata_text": text_storage(metadata.into_iter()),
        "files": text_storage(documents.iter().map(|document| document.file.as_ref())),
        "names": text_storage(documents.iter().map(|document| document.name.as_ref())),
        "type_paths": text_storage(documents.iter().map(|document| document.type_path.as_ref())),
        "docs": text_storage(documents.iter().map(|document| document.docs.as_ref())),
        "source": text_storage(documents.iter().filter_map(|document| document.source.as_ref().map(|source| source.text.as_str()))),
        "posting_terms": index.postings.len(),
        "posting_count": index.postings.values().map(Vec::len).sum::<usize>(),
        "posting_capacity": index.postings.values().map(Vec::capacity).sum::<usize>(),
        "posting_capacity_bytes": index.postings.values().map(|rows| rows.capacity() * size_of::<Posting>()).sum::<usize>(),
        "exact_symbol_count": index.exact_symbols.len(),
        "exact_id_entry_bytes": size_of::<DocumentIds>(),
        "exact_symbol_inline_ids": index.exact_symbols.values().filter(|ids| matches!(ids, DocumentIds::Single(_))).count(),
        "exact_symbol_id_capacity_bytes": index.exact_symbols.values().map(|rows| rows.heap_capacity() * size_of::<usize>()).sum::<usize>(),
        "exact_name_count": index.exact_names.len(),
        "exact_name_inline_ids": index.exact_names.values().filter(|ids| matches!(ids, DocumentIds::Single(_))).count(),
        "exact_name_id_capacity_bytes": index.exact_names.values().map(|rows| rows.heap_capacity() * size_of::<usize>()).sum::<usize>(),
    });
    println!("SEARCH_STORAGE_PROFILE {result}");
}
