use super::*;
use crate::store::new_ledger_json;
use crate::{Component, ContextTree, ProviderThreadOptions};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pactree-{label}-{}-{nonce}.pact",
        std::process::id()
    ))
}

fn exported_records_fixture() -> String {
    let source = temp_path("conversion-source");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn-1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "children": [{
                "id": "text-1",
                "componentType": "text_content",
                "type": "text_content",
                "content": "lossless playback content that must survive conversion"
            }]
        }]
    });
    let mut ledger: Value = serde_json::from_str(
        &new_ledger_json(&tree.to_string(), Some("main"), 8).expect("new ledger"),
    )
    .expect("ledger json");
    ledger["event_index"]["transaction_traces"]["tx-1"] = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx-1",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":0},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":0},
        "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":1},
        "post_response_ref": {"root_id":"root","branch_id":"main","revision":1},
        "alteration_ids": ["insert"],
        "replay_trace": {"from_rev":0,"to_rev":1,"ops":[]},
        "aux": {"events": [{"op_kind":"message", "aux":{"role":"assistant"}}]}
    });
    ledger["branches"]["main"]["head_revision"] = json!(1);
    ledger["branches"]["main"]["current"]["head_revision"] = json!(1);
    file_create_from_ledger_json(&source, &ledger.to_string(), 8).expect("create source");
    let scanned = scan_file(&fs::read(&source).expect("read source"), true);
    fs::remove_file(&source).expect("remove source");
    serde_json::to_string(
        &scanned
            .records
            .iter()
            .map(|record| {
                json!({
                    "sequence": record.sequence,
                    "kind": record.kind,
                    "payload_bytes": record.payload_bytes,
                    "payload": record.payload,
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("export records")
}

#[test]
fn exported_records_convert_to_verified_appendable_pactbin2() {
    let output = temp_path("conversion-output");
    let fixture = exported_records_fixture();
    crate::tests::instrumentation::reset_head_metadata_snapshot_decode_count();
    let report_json =
        file_convert_exported_records_json(&fixture, &output).expect("convert exported records");
    let report: Value = serde_json::from_str(&report_json).expect("report json");

    assert_eq!(&fs::read(&output).expect("read output")[..8], b"PACTBIN2");
    assert_eq!(report["verified"], true);
    assert_eq!(report["playback_row_count"], 1);
    assert!(report["content_record_count"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(
        report["source_playback_digest"],
        report["output_playback_digest"]
    );
    assert_eq!(
        report["source_content_digest"],
        report["output_content_digest"]
    );
    assert_eq!(
        report["output_file_bytes"],
        fs::metadata(&output).expect("output metadata").len()
    );
    let layout = &report["physical_layout"];
    let accounted = [
        "envelope_bytes",
        "canonical_bytes",
        "checkpoint_bytes",
        "content_bytes",
        "facet_bytes",
        "directory_bytes",
        "allocator_bytes",
        "commit_bytes",
    ]
    .into_iter()
    .map(|field| layout[field].as_u64().expect("layout byte count"))
    .sum::<u64>();
    assert_eq!(accounted, report["output_file_bytes"]);
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        &output,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )
    .expect("open converted snapshot");
    assert_eq!(
        snapshot.generation(),
        1,
        "cold conversion must collapse obsolete physical index generations"
    );
    assert_eq!(
        crate::tests::instrumentation::head_metadata_snapshot_decode_count(),
        0,
        "conversion must never clone and deserialize a ledger only to build index metadata"
    );
    let converted_ledger = crate::pact_file::open_ledger(&output).expect("open converted ledger");
    assert_eq!(
        converted_ledger.branches["main"].base_ref.revision,
        Some(0),
        "converted replay history must stay anchored to its revision-zero base"
    );
    assert!(
        converted_ledger
            .project_branch("main", Some(1), 1024)
            .is_ok(),
        "converted history must remain independently projectable"
    );

    let mut tree = ContextTree::open_file(&output, None).expect("open converted tree");
    tree.insert(
        Component::from_component_value(&json!({
            "id": "after-conversion",
            "componentType": "text_content",
            "content": "ordinary append after cold conversion"
        }))
        .expect("append component"),
        "root",
    )
    .expect("insert after conversion");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal after conversion");
    let reopened = ContextTree::open_file(&output, None).expect("reopen appended tree");
    assert_eq!(reopened.head().expect("appended head").head_revision, 2);
    assert_eq!(
        reopened
            .project_component("root:after-conversion")
            .expect("project appended component")["content"],
        "ordinary append after cold conversion"
    );
    fs::remove_file(output).expect("remove output");
}

#[test]
fn cold_converter_reuses_ledger_for_head_metadata() {
    let root = json!({
        "id": "cold_metadata_reuse",
        "componentType": "root",
        "children": []
    });
    let ledger: crate::store::ContextTreeLedger = serde_json::from_str(
        &new_ledger_json(&root.to_string(), Some("main"), 1024).expect("new ledger"),
    )
    .expect("ledger");
    let output = temp_path("cold-metadata-reuse");

    crate::tests::instrumentation::reset_head_metadata_snapshot_decode_count();
    let _staged = write_cold_v2_baseline(&output, &ledger).expect("cold baseline");

    assert_eq!(
        crate::tests::instrumentation::head_metadata_snapshot_decode_count(),
        0,
        "cold conversion must not clone and deserialize its ledger JSON for metadata"
    );
}

#[test]
fn exported_record_conversion_refuses_existing_destination() {
    let output = temp_path("conversion-existing");
    fs::write(&output, b"existing").expect("seed destination");

    let error = file_convert_exported_records_json(&exported_records_fixture(), &output)
        .expect_err("existing destination must be refused");

    assert!(error.to_string().contains("already exists"));
    assert_eq!(fs::read(&output).expect("read destination"), b"existing");
    fs::remove_file(output).expect("remove output");
}

#[test]
fn imported_trace_externalizes_tool_parameters_and_drops_derived_child_ids() {
    let large_command = "printf imported-parameter ".repeat(128);
    let replay = crate::transaction_trace::TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: vec![
            crate::transaction_trace::TraceOp::new(
                "call",
                crate::transaction_trace::TraceOpKind::InsertComponent {
                    parent_id: "root".to_string(),
                    index: 0,
                    component: json!({
                        "id": "call",
                        "componentType": "tool_call",
                        "tool_call_id": "tc-1",
                        "tool_name": "shell",
                        "parameters": {"command": large_command},
                        "child_ids": []
                    }),
                },
            ),
            crate::transaction_trace::TraceOp::new(
                "root",
                crate::transaction_trace::TraceOpKind::SetField {
                    key: "child_ids".to_string(),
                    value: json!(["call"]),
                },
            ),
        ],
    };
    let mut content = crate::store::ContentStore::default();

    let normalized = super::conversion::normalize_imported_replay_trace(replay, &mut content, 64)
        .expect("normalize imported trace");

    assert!(!normalized.ops.iter().any(|op| matches!(
        &op.kind,
        crate::transaction_trace::TraceOpKind::SetField { key, .. } if key == "child_ids"
    )));
    let component = normalized
        .ops
        .iter()
        .find_map(|op| match &op.kind {
            crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } => {
                Some(component)
            }
            _ => None,
        })
        .expect("inserted component");
    assert!(component.get("parameters").is_none());
    assert!(component.get("parameters_ref").is_some());
    assert_eq!(content.records.len(), 1);
}

#[test]
fn imported_active_placeholder_is_not_promoted_to_message_turn() {
    let replay = crate::transaction_trace::TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: vec![crate::transaction_trace::TraceOp::new(
            "active",
            crate::transaction_trace::TraceOpKind::InsertComponent {
                parent_id: "root".to_string(),
                index: 0,
                component: json!({
                    "id": "active",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "user",
                    "created_at_ns": 0,
                    "creation_index": 0,
                    "children": [],
                    "aux": {"pact_lifecycle": "active"}
                }),
            },
        )],
    };
    let mut content = crate::store::ContentStore::default();

    let normalized =
        normalize_imported_replay_trace(replay, &mut content, 1024).expect("normalize placeholder");
    let component = match &normalized.ops[0].kind {
        crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } => component,
        _ => panic!("expected insert"),
    };

    assert_eq!(component["componentType"], "active_message");
    assert_eq!(component["type"], "active_message");
    assert_eq!(component["role"], "user");
}

#[test]
fn imported_revision_zero_active_placeholder_leaves_message_indexes() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "created_at_ns": 0,
            "creation_index": 0,
            "children": [],
            "aux": {"pact_lifecycle": "active"}
        }]
    });
    let ledger: crate::store::ContextTreeLedger = serde_json::from_str(
        &new_ledger_json(&tree.to_string(), Some("main"), 1024).expect("ledger"),
    )
    .expect("ledger json");
    let mut view = ledger
        .project_branch("main", Some(0), 1024)
        .expect("revision zero");

    normalize_imported_initial_placeholder(&mut view).expect("normalize baseline");

    assert_eq!(
        view.tree
            .component("active")
            .expect("active placeholder")
            .component_type(),
        "active_message"
    );
    assert!(
        !view
            .tree
            .index
            .by_type
            .get("message_turn")
            .is_some_and(|ids| ids.iter().any(|id| id == "active"))
    );
    assert!(!view.message_order.iter().any(|id| id == "active"));
}

#[test]
fn v2_trace_facade_uses_revision_order_instead_of_transaction_id_order() {
    let path = temp_path("trace-revision-order");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": []
    });
    let mut ledger: Value = serde_json::from_str(
        &new_ledger_json(&tree.to_string(), Some("main"), 8).expect("new ledger"),
    )
    .expect("ledger json");
    for (transaction_id, from_rev, to_rev) in [("tx_main_10", 1_u64, 2_u64), ("tx_main_2", 0, 1)] {
        ledger["event_index"]["transaction_traces"][transaction_id] = json!({
            "schema": "pact.transaction_trace.v1",
            "root_id": "root",
            "branch_id": "main",
            "transaction_id": transaction_id,
            "authored_ref": {"root_id":"root","branch_id":"main","revision":from_rev},
            "pre_request_ref": {"root_id":"root","branch_id":"main","revision":from_rev},
            "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":to_rev},
            "alteration_ids": [],
            "replay_trace": {"from_rev":from_rev,"to_rev":to_rev,"ops":[]},
            "aux": {"events": [{"op_kind":"message"}]}
        });
    }
    file_create_from_ledger_json(&path, &ledger.to_string(), 8).expect("create source");

    let scanned = scan_file(&fs::read(&path).expect("read source"), true);
    let transaction_ids = scanned
        .records
        .iter()
        .filter(|record| record.kind == "trace")
        .filter_map(|record| record.payload["transaction_id"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(transaction_ids, ["tx_main_2", "tx_main_10"]);
    fs::remove_file(path).expect("remove source");
}

#[test]
fn legacy_binary_trace_decode_skips_derived_ops_before_allocating_history() {
    let mut ops = (0..1024)
        .map(|depth| {
            json!({
                "component_id": format!("node-{depth}"),
                "op": "set_field",
                "key": "pact_depth",
                "value": depth,
            })
        })
        .collect::<Vec<_>>();
    ops.push(json!({
        "component_id": "text",
        "op": "insert_component",
        "parent_id": "root",
        "index": 0,
        "component": {
            "id": "text",
            "componentType": "text_content",
            "content": "keep me"
        }
    }));
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx-1",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":0},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":0},
        "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":1},
        "alteration_ids": [],
        "replay_trace": {"from_rev":0,"to_rev":1,"ops":ops},
        "aux": {}
    });
    let encoded =
        super::page_codec::encode_record_payload(super::layout::RecordKind::Trace, &trace)
            .expect("encode pre-v0.1 trace");

    let decoded = super::page_codec::decode_trace_record_payload_filtered(
        &encoded,
        &super::SymbolTable::default(),
    )
    .expect("decode filtered trace");

    let ops = decoded["replay_trace"]["ops"]
        .as_array()
        .expect("decoded ops");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["op"], "insert_component");
}
