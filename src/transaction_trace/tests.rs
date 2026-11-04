use super::*;
use serde_json::json;

fn assert_round_trip(before: &Value, after: &Value) -> Vec<TraceOp> {
    let trace = TransactionTrace::diff(0, 1, before, after);
    let replayed = trace.replay(before).expect("replay should succeed");
    assert_eq!(
        replayed,
        *after,
        "replay mismatch\nledger: {}\n got: {}\nwant: {}",
        serde_json::to_string_pretty(&trace.ops).unwrap(),
        serde_json::to_string_pretty(&replayed).unwrap(),
        serde_json::to_string_pretty(after).unwrap(),
    );
    trace.ops
}

fn root(children: Value) -> Value {
    json!({ "id": "root", "componentType": "root", "children": children })
}

#[test]
fn unchanged_tree_produces_no_ops() {
    let tree = root(json!([{ "id": "a", "componentType": "text", "content": "hi" }]));
    let ops = assert_round_trip(&tree, &tree);
    assert!(ops.is_empty(), "expected no ops, got {ops:?}");
}

#[test]
fn insert_node_replay_equals_after() {
    let before = root(json!([{ "id": "a", "componentType": "text", "content": "a" }]));
    let after = root(json!([
        { "id": "a", "componentType": "text", "content": "a" },
        { "id": "b", "componentType": "text", "content": "b" },
    ]));
    let ops = assert_round_trip(&before, &after);
    assert!(
        ops
            .iter()
            .any(|d| matches!(d.kind, TraceOpKind::InsertComponent { .. }) && d.component_id == "b"),
        "expected InsertComponent for b, got {ops:?}"
    );

    assert!(
        !ops
            .iter()
            .any(|d| d.component_id == "a" && matches!(d.kind, TraceOpKind::InsertComponent { .. })),
        "node a should not be re-inserted"
    );
}

#[test]
fn insert_nested_subtree() {
    let before = root(json!([{ "id": "a", "componentType": "container", "children": [] }]));
    let after = root(json!([{
        "id": "a", "componentType": "container",
        "children": [{
            "id": "b", "componentType": "container",
            "children": [{ "id": "c", "componentType": "text", "content": "deep" }]
        }]
    }]));
    let ops = assert_round_trip(&before, &after);

    let insert_ids: Vec<&str> = ops
        .iter()
        .filter(|d| matches!(d.kind, TraceOpKind::InsertComponent { .. }))
        .map(|d| d.component_id.as_str())
        .collect();
    assert_eq!(insert_ids, vec!["b", "c"]);
}

#[test]
fn delete_node_emits_only_tombstone() {
    let before = root(json!([
        { "id": "a", "componentType": "text", "content": "a" },
        { "id": "b", "componentType": "text", "content": "b" },
    ]));
    let after = root(json!([{ "id": "a", "componentType": "text", "content": "a" }]));
    let ops = assert_round_trip(&before, &after);
    assert_eq!(ops.len(), 1, "expected a single tombstone, got {ops:?}");
    assert!(matches!(ops[0].kind, TraceOpKind::DeleteComponent));
    assert_eq!(ops[0].component_id, "b");
}

#[test]
fn delete_subtree_emits_single_tombstone() {
    let before = root(json!([{
        "id": "a", "componentType": "container",
        "children": [{ "id": "b", "componentType": "text", "content": "b" }]
    }]));
    let after = root(json!([]));
    let ops = assert_round_trip(&before, &after);

    assert_eq!(ops.len(), 1, "got {ops:?}");
    assert_eq!(ops[0].component_id, "a");
}

#[test]
fn set_and_unset_attr() {
    let before = root(json!([{
        "id": "a", "componentType": "tool_call",
        "aux": { "tool_name": "shell", "stale": true }
    }]));
    let after = root(json!([{
        "id": "a", "componentType": "tool_call",
        "aux": { "tool_name": "shell", "status": "ok" }
    }]));
    let ops = assert_round_trip(&before, &after);
    assert!(
        ops.iter()
            .any(|d| matches!(&d.kind, TraceOpKind::SetAttr { key, .. } if key == "status"))
    );
    assert!(
        ops.iter()
            .any(|d| matches!(&d.kind, TraceOpKind::UnsetAttr { key } if key == "stale"))
    );

    assert!(
        !ops.iter()
            .any(|d| matches!(&d.kind, TraceOpKind::SetAttr { key, .. } if key == "tool_name"))
    );
}

#[test]
fn set_content_ref_replay_equals_after() {
    let before = root(json!([{
        "id": "a", "componentType": "text", "content_ref": { "id": "cnt_old" }
    }]));
    let after = root(json!([{
        "id": "a", "componentType": "text", "content_ref": { "id": "cnt_new" }
    }]));
    let ops = assert_round_trip(&before, &after);
    assert!(
        ops.iter()
            .any(|d| matches!(d.kind, TraceOpKind::SetContent { .. }))
    );
}

#[test]
fn replace_content_range_for_inline_edit() {
    let before = root(json!([{ "id": "a", "componentType": "text", "content": "hello world" }]));
    let after =
        root(json!([{ "id": "a", "componentType": "text", "content": "hello brave world" }]));
    let ops = assert_round_trip(&before, &after);
    let range = ops
        .iter()
        .find_map(|d| match &d.kind {
            TraceOpKind::ReplaceContentRange {
                start,
                end,
                replacement,
            } => Some((*start, *end, replacement.clone())),
            _ => None,
        })
        .expect("expected a ReplaceContentRange op");

    assert_eq!(range.0, 6);
    assert_eq!(range.1, 6);
    assert_eq!(range.2, "brave ");
}

#[test]
fn full_content_rewrite_uses_set_content() {
    let before = root(json!([{ "id": "a", "componentType": "text", "content": "abc" }]));
    let after = root(json!([{ "id": "a", "componentType": "text", "content": "xyz" }]));
    let ops = assert_round_trip(&before, &after);

    assert!(
        ops.iter()
            .any(|d| matches!(d.kind, TraceOpKind::SetContent { .. }))
    );
}

#[test]
fn set_lifecycle() {
    let before = root(json!([{ "id": "a", "componentType": "turn", "lifecycle": "active" }]));
    let after = root(json!([{ "id": "a", "componentType": "turn", "lifecycle": "frozen" }]));
    let ops = assert_round_trip(&before, &after);
    assert!(
        ops.iter()
            .any(|d| matches!(&d.kind, TraceOpKind::SetLifecycle { value } if value == "frozen"))
    );
}

#[test]
fn reorder_siblings_replays_exact_order() {
    let before = root(json!([
        { "id": "a", "componentType": "text" },
        { "id": "b", "componentType": "text" },
        { "id": "c", "componentType": "text" },
    ]));
    let after = root(json!([
        { "id": "c", "componentType": "text" },
        { "id": "a", "componentType": "text" },
        { "id": "b", "componentType": "text" },
    ]));
    let ops = assert_round_trip(&before, &after);
    assert!(
        ops.iter()
            .any(|d| matches!(d.kind, TraceOpKind::MoveComponent { .. }))
    );
}

#[test]
fn move_node_across_parents() {
    let before = root(json!([
        { "id": "p1", "componentType": "container", "children": [
            { "id": "x", "componentType": "text", "content": "keep" }
        ]},
        { "id": "p2", "componentType": "container", "children": [] },
    ]));
    let after = root(json!([
        { "id": "p1", "componentType": "container", "children": [] },
        { "id": "p2", "componentType": "container", "children": [
            { "id": "x", "componentType": "text", "content": "keep" }
        ]},
    ]));
    let ops = assert_round_trip(&before, &after);

    assert!(ops.iter().any(|d| d.component_id == "x"
        && matches!(&d.kind, TraceOpKind::MoveComponent { parent_id, .. } if parent_id == "p2")));
    assert!(
        !ops.iter()
            .any(|d| d.component_id == "x" && matches!(d.kind, TraceOpKind::DeleteComponent))
    );
}

#[test]
fn move_node_out_of_deleted_subtree_replays() {
    let before = root(json!([
        { "id": "p", "componentType": "container", "children": [
            { "id": "x", "componentType": "text", "content": "survivor" }
        ]},
        { "id": "q", "componentType": "container", "children": [] },
    ]));
    let after = root(json!([
        { "id": "q", "componentType": "container", "children": [
            { "id": "x", "componentType": "text", "content": "survivor" }
        ]},
    ]));
    let ops = assert_round_trip(&before, &after);
    assert!(ops.iter().any(|d| d.component_id == "x"
        && matches!(&d.kind, TraceOpKind::MoveComponent { parent_id, .. } if parent_id == "q")));
    assert!(
        ops.iter()
            .any(|d| d.component_id == "p" && matches!(d.kind, TraceOpKind::DeleteComponent)),
        "p must be tombstoned"
    );
}

#[test]
fn move_node_into_newly_inserted_parent() {
    let before = root(json!([
        { "id": "x", "componentType": "text", "content": "keep" },
    ]));
    let after = root(json!([
        { "id": "p", "componentType": "container", "children": [
            { "id": "x", "componentType": "text", "content": "keep" }
        ]},
    ]));
    assert_round_trip(&before, &after);
}

#[test]
fn canonical_normalization_drops_derived_depth_churn() {
    let before = root(json!([{
        "id": "turn_1",
        "componentType": "message_turn",
        "depth": 1,
        "pact_depth": 1,
        "children": [{
            "id": "text_1",
            "componentType": "text",
            "content": "same",
            "depth": 1,
            "pact_depth": 1
        }]
    }]));
    let after = root(json!([{
        "id": "turn_1",
        "componentType": "message_turn",
        "depth": 2,
        "pact_depth": 2,
        "children": [{
            "id": "text_1",
            "componentType": "text",
            "content": "same",
            "depth": 2,
            "pact_depth": 2
        }]
    }]));

    let raw = TransactionTrace::diff(0, 1, &before, &after);
    assert!(
        raw.ops.iter().any(|op| matches!(
            &op.kind,
            TraceOpKind::SetField { key, .. } if key == "depth" || key == "pact_depth"
        )),
        "raw projected surfaces demonstrate the old depth churn"
    );

    let normalized = TransactionTrace::diff(
        0,
        1,
        &normalize_for_transaction_trace(&before),
        &normalize_for_transaction_trace(&after),
    );
    assert!(
        normalized.ops.is_empty(),
        "canonical trace must not persist derived depth churn: {normalized:#?}"
    );
}

#[test]
fn canonical_normalization_drops_derived_child_id_churn() {
    let before = root(json!([{
        "id": "turn_1",
        "componentType": "message_turn",
        "child_ids": ["text_1"],
        "children": [{
            "id": "text_1",
            "componentType": "text",
            "content": "same",
            "child_ids": []
        }]
    }]));
    let mut after = before.clone();
    after["child_ids"] = json!(["turn_1"]);
    after["children"][0]["child_ids"] = json!(["text_1", "derived_only"]);

    let raw = TransactionTrace::diff(0, 1, &before, &after);
    assert!(raw.ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, .. } if key == "child_ids"
    )));

    let normalized = TransactionTrace::diff(
        0,
        1,
        &normalize_for_transaction_trace(&before),
        &normalize_for_transaction_trace(&after),
    );
    assert!(
        normalized.ops.is_empty(),
        "canonical trace must not persist derived child-id churn: {normalized:#?}"
    );
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

#[test]
fn decode_transaction_trace_rejects_forged_huge_counts_without_panic() {
    let mut huge_symbols = b"PTTR".to_vec();
    huge_symbols.push(2);
    huge_symbols.extend(varint(0));
    huge_symbols.extend(varint(1));
    huge_symbols.extend(varint(u64::MAX));
    assert!(decode_transaction_trace(&huge_symbols).is_err());

    let mut huge_ops = b"PTTR".to_vec();
    huge_ops.push(2);
    huge_ops.extend(varint(0));
    huge_ops.extend(varint(1));
    huge_ops.extend(varint(0));
    huge_ops.extend(varint(u64::MAX));
    assert!(decode_transaction_trace(&huge_ops).is_err());
}

#[test]
fn unpack_transaction_traces_rejects_forged_huge_count_without_panic() {
    let forged = varint(u64::MAX);
    assert!(unpack_transaction_traces(&forged).is_err());
}

#[test]
fn combined_edit_insert_update_delete() {
    let before = root(json!([
        { "id": "a", "componentType": "text", "content": "alpha", "attrs": { "k": 1 } },
        { "id": "b", "componentType": "text", "content": "bravo" },
    ]));
    let after = root(json!([
        { "id": "a", "componentType": "text", "content": "alpha!", "attrs": { "k": 2 } },
        { "id": "c", "componentType": "text", "content": "charlie" },
    ]));
    assert_round_trip(&before, &after);
}

fn assert_binary_round_trip(before: &Value, after: &Value) -> Vec<u8> {
    let trace = TransactionTrace::diff(0, 1, before, after);
    let bytes = encode_transaction_trace(&trace);
    let decoded = decode_transaction_trace(&bytes).expect("decode");
    assert_eq!(trace, decoded, "binary round-trip changed the trace");
    assert_eq!(
        decoded.replay(before).expect("replay decoded"),
        *after,
        "decoded trace must still replay to the after tree"
    );
    bytes
}

#[test]
fn binary_encoding_round_trips_all_op_kinds() {
    let before = root(json!([
        { "id": "a", "componentType": "text", "content": "hello world", "role": "user",
          "attrs": { "keep": 1, "stale": true }, "lifecycle": "active" },
        { "id": "b", "componentType": "text", "content_ref": { "id": "cnt_x" } },
        { "id": "gone", "componentType": "text" },
    ]));
    let after = root(json!([
        { "id": "a", "componentType": "text", "content": "hello brave world", "role": "assistant",
          "attrs": { "keep": 1, "fresh": "y" }, "lifecycle": "frozen" },
        { "id": "b", "componentType": "text", "content_ref": { "id": "cnt_z" } },
        { "id": "c", "componentType": "text", "content": "new node" },
    ]));
    assert_binary_round_trip(&before, &after);
}

#[test]
fn well_known_field_ids_are_binary_tags_not_serialized() {
    let before = root(json!([{ "id": "a", "componentType": "text", "role": "user" }]));
    let after = root(json!([{ "id": "a", "componentType": "text", "role": "assistant" }]));
    let trace = TransactionTrace::diff(0, 1, &before, &after);
    let bytes = encode_transaction_trace(&trace);

    assert!(
        !bytes.windows(4).any(|w| w == b"role"),
        "well-known field id should be a tag, not serialized: {bytes:?}"
    );
    assert!(
        !bytes.windows(4).any(|w| w == b"root"),
        "well-known root id should be a tag, not serialized"
    );

    assert_eq!(decode_transaction_trace(&bytes).unwrap(), trace);
    assert_eq!(trace.replay(&before).unwrap(), after);
}

#[test]
fn inserted_nodes_are_structural_refs_not_json_blobs() {
    let before = root(json!([]));
    let after = root(json!([{
        "id": "n1",
        "componentType": "text_content",
        "content": "hello",
        "attrs": { "role": "user" }
    }]));
    let trace = TransactionTrace::diff(0, 1, &before, &after);
    let inserted = trace
        .ops
        .iter()
        .find_map(|op| match &op.kind {
            TraceOpKind::InsertComponent { component, .. } => Some(component),
            _ => None,
        })
        .expect("expected an inserted node");
    let encoded_blob = serde_json::to_string(inserted).expect("node JSON");
    let bytes = encode_transaction_trace(&trace);

    assert!(
        !bytes
            .windows(encoded_blob.len())
            .any(|window| window == encoded_blob.as_bytes()),
        "InsertComponent should not serialize the full childless node JSON blob"
    );
    assert_eq!(decode_transaction_trace(&bytes).unwrap(), trace);
}

#[test]
fn inserted_container_preserves_empty_children_slot_in_binary() {
    let before = root(json!([]));
    let after = root(json!([{
        "id": "parent",
        "componentType": "container",
        "children": [{ "id": "child", "componentType": "text", "content": "leaf" }]
    }]));

    assert_binary_round_trip(&before, &after);
}

#[test]
fn binary_encoding_handles_empty_ledger() {
    let tree = root(json!([{ "id": "a", "componentType": "text" }]));
    let bytes = assert_binary_round_trip(&tree, &tree);
    let decoded = decode_transaction_trace(&bytes).unwrap();
    assert!(decoded.is_empty());
}

#[test]
fn decode_rejects_corrupt_input() {
    assert!(decode_transaction_trace(b"").is_err());
    assert!(decode_transaction_trace(b"NOPE\x01").is_err());
    let mut good = encode_transaction_trace(&TransactionTrace::diff(
        0,
        1,
        &root(json!([{ "id": "a" }])),
        &root(json!([{ "id": "a" }, { "id": "b" }])),
    ));
    good[4] = 99;
    assert!(decode_transaction_trace(&good).is_err());
}

#[test]
fn binary_ledger_is_smaller_than_whole_node_snapshot() {
    let mut before_children = Vec::new();
    let mut after_children = Vec::new();
    for i in 0..50 {
        let node = json!({
            "id": format!("n{i}"),
            "componentType": "text_content",
            "content": format!("this is the content body of node number {i} with some length"),
            "attrs": { "index": i }
        });
        before_children.push(node.clone());
        if i == 7 {
            let mut mutated = node.clone();
            mutated["content"] =
                json!("this is the EDITED content body of node number 7 with some length");
            after_children.push(mutated);
        } else {
            after_children.push(node);
        }
    }
    let before = root(Value::Array(before_children));
    let after = root(Value::Array(after_children));

    let trace = TransactionTrace::diff(0, 1, &before, &after);
    assert_eq!(trace.replay(&before).unwrap(), after);
    let binary = encode_transaction_trace(&trace);
    let json_ledger = serde_json::to_vec(&trace).unwrap();
    let whole_node_snapshot = serde_json::to_vec(&after["children"][7]).unwrap();

    assert!(
        binary.len() < json_ledger.len(),
        "binary ({}) should beat JSON trace ({})",
        binary.len(),
        json_ledger.len()
    );
    assert!(
        binary.len() < whole_node_snapshot.len(),
        "binary trace ({}) should beat a whole-node snapshot ({})",
        binary.len(),
        whole_node_snapshot.len()
    );
}

#[test]
fn ledger_json_round_trips_through_serde() {
    let before = root(json!([{ "id": "a", "componentType": "text", "content": "x" }]));
    let after = root(json!([
        { "id": "a", "componentType": "text", "content": "xy" },
        { "id": "b", "componentType": "text", "content": "new" },
    ]));
    let trace = TransactionTrace::diff(3, 4, &before, &after);
    let encoded = serde_json::to_string(&trace).unwrap();
    let decoded: TransactionTrace = serde_json::from_str(&encoded).unwrap();
    assert_eq!(trace, decoded);
    assert_eq!(decoded.from_rev, 3);
    assert_eq!(decoded.to_rev, 4);
    assert_eq!(decoded.replay(&before).unwrap(), after);
}

#[test]
fn trace_op_variants_round_trip_through_serde() {
    let ops = vec![
        TraceOp::new(
            "a",
            TraceOpKind::InsertComponent {
                parent_id: "root".to_string(),
                index: 0,
                component: json!({ "id": "a", "componentType": "text" }),
            },
        ),
        TraceOp::new("a", TraceOpKind::DeleteComponent),
        TraceOp::new(
            "a",
            TraceOpKind::MoveComponent {
                parent_id: "root".to_string(),
                index: 1,
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::SetField {
                key: "role".to_string(),
                value: json!("assistant"),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::UnsetField {
                key: "role".to_string(),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::SetAttr {
                key: "tool_name".to_string(),
                value: json!("search"),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::UnsetAttr {
                key: "tool_name".to_string(),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::SetContent {
                content: Some("body".to_string()),
                content_ref: Some(json!({ "sha256": "abc" })),
                content_preview: Some("bo".to_string()),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::ReplaceContentRange {
                start: 0,
                end: 1,
                replacement: "B".to_string(),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::SetLifecycle {
                value: json!("live"),
            },
        ),
    ];
    for op in ops {
        op.validate().expect("variant should validate");
        let encoded = serde_json::to_string(&op).expect("serialize op");
        assert!(!encoded.contains("node_delta"));
        assert!(!encoded.contains("insert_node"));
        let decoded: TraceOp = serde_json::from_str(&encoded).expect("deserialize op");
        assert_eq!(decoded, op);
    }
}

#[test]
fn component_change_helpers_lower_to_canonical_trace_ops() {
    let changes = vec![
        (
            ComponentChange::SetField {
                key: "role".to_string(),
                value: json!("assistant"),
            },
            TraceOpKind::SetField {
                key: "role".to_string(),
                value: json!("assistant"),
            },
        ),
        (
            ComponentChange::UnsetField {
                key: "role".to_string(),
            },
            TraceOpKind::UnsetField {
                key: "role".to_string(),
            },
        ),
        (
            ComponentChange::SetAux {
                key: "tone".to_string(),
                value: json!("clear"),
            },
            TraceOpKind::SetAttr {
                key: "tone".to_string(),
                value: json!("clear"),
            },
        ),
        (
            ComponentChange::UnsetAux {
                key: "tone".to_string(),
            },
            TraceOpKind::UnsetAttr {
                key: "tone".to_string(),
            },
        ),
        (
            ComponentChange::SetContent {
                content: None,
                content_ref: Some(json!({ "id": "cnt_1" })),
                content_preview: Some("preview".to_string()),
            },
            TraceOpKind::SetContent {
                content: None,
                content_ref: Some(json!({ "id": "cnt_1" })),
                content_preview: Some("preview".to_string()),
            },
        ),
        (
            ComponentChange::SetLifecycle {
                value: json!({ "visible": true }),
            },
            TraceOpKind::SetLifecycle {
                value: json!({ "visible": true }),
            },
        ),
    ];

    for (change, expected) in changes {
        let encoded = serde_json::to_string(&change).expect("serialize change");
        let decoded: ComponentChange = serde_json::from_str(&encoded).expect("deserialize change");
        assert_eq!(decoded.into_trace_op_kind(), expected);
    }
}

#[test]
fn trace_op_validation_rejects_incomplete_ops() {
    assert!(
        TraceOp::new("", TraceOpKind::DeleteComponent)
            .validate()
            .is_err()
    );
    assert!(
        TraceOp::new(
            "a",
            TraceOpKind::InsertComponent {
                parent_id: "".to_string(),
                index: 0,
                component: json!({ "id": "a", "componentType": "text" }),
            },
        )
        .validate()
        .is_err()
    );
    assert!(
        TraceOp::new(
            "a",
            TraceOpKind::InsertComponent {
                parent_id: "root".to_string(),
                index: 0,
                component: json!({ "id": "other", "componentType": "text" }),
            },
        )
        .validate()
        .is_err()
    );
    assert!(
        TraceOp::new(
            "a",
            TraceOpKind::SetField {
                key: "".to_string(),
                value: json!(true),
            },
        )
        .validate()
        .is_err()
    );
    assert!(
        TraceOp::new(
            "a",
            TraceOpKind::ReplaceContentRange {
                start: 2,
                end: 1,
                replacement: String::new(),
            },
        )
        .validate()
        .is_err()
    );
    assert!(
        TransactionTrace {
            from_rev: 2,
            to_rev: 1,
            ops: Vec::new(),
        }
        .validate()
        .is_err()
    );
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 16
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

fn collect_ids(node: &Value, out: &mut Vec<String>) {
    if let Some(id) = node.get("id").and_then(Value::as_str)
        && id != "root"
    {
        out.push(id.to_string());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_ids(child, out);
        }
    }
}

fn find_mut<'a>(node: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    if node.get("id").and_then(Value::as_str) == Some(id) {
        return Some(node);
    }
    let children = node.get_mut("children")?.as_array_mut()?;
    for child in children {
        if let Some(found) = find_mut(child, id) {
            return Some(found);
        }
    }
    None
}

fn random_edit(tree: &mut Value, rng: &mut Lcg, next_id: &mut usize) {
    let mut ids = Vec::new();
    collect_ids(tree, &mut ids);
    ids.push("root".to_string());
    let target = ids[rng.below(ids.len())].clone();

    match rng.below(5) {
        0 => {
            let new_id = format!("n{}", *next_id);
            *next_id += 1;
            let node = json!({ "id": new_id, "componentType": "text", "content": format!("c{}", rng.below(100)) });
            if let Some(parent) = find_mut(tree, &target) {
                let arr = parent
                    .as_object_mut()
                    .unwrap()
                    .entry("children")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap();
                let at = rng.below(arr.len() + 1);
                arr.insert(at, node);
            }
        }
        1 if target != "root" => {
            super::detach_subtree(tree, &target);
        }
        2 => {
            if let Some(node) = find_mut(tree, &target) {
                node.as_object_mut()
                    .unwrap()
                    .insert("role".to_string(), json!(format!("r{}", rng.below(3))));
            }
        }
        3 => {
            if let Some(node) = find_mut(tree, &target) {
                node.as_object_mut().unwrap().insert(
                    "content".to_string(),
                    json!(format!("text-{}", rng.below(1000))),
                );
            }
        }
        _ => {
            if let Some(node) = find_mut(tree, &target) {
                let attrs = node
                    .as_object_mut()
                    .unwrap()
                    .entry("attrs")
                    .or_insert_with(|| json!({}));
                attrs
                    .as_object_mut()
                    .unwrap()
                    .insert(format!("k{}", rng.below(3)), json!(rng.below(50)));
            }
        }
    }
}

#[test]
fn fuzz_random_edits_round_trip() {
    let mut rng = Lcg(0x9E3779B97F4A7C15);
    for trial in 0..2000 {
        let before = root(json!([
            { "id": "a", "componentType": "container", "children": [
                { "id": "a1", "componentType": "text", "content": "x" },
                { "id": "a2", "componentType": "text", "content": "y" },
            ]},
            { "id": "b", "componentType": "container", "children": [] },
        ]));
        let mut after = before.clone();
        let mut next_id = 0usize;
        let edits = 1 + rng.below(8);
        for _ in 0..edits {
            random_edit(&mut after, &mut rng, &mut next_id);
        }
        let trace = TransactionTrace::diff(0, 1, &before, &after);
        let replayed = trace.replay(&before).unwrap_or_else(|err| {
            panic!("trial {trial}: replay error {err}\nledger: {:?}", trace.ops)
        });
        assert_eq!(
            replayed,
            after,
            "trial {trial} mismatch\nledger: {}\nbefore: {}",
            serde_json::to_string_pretty(&trace.ops).unwrap(),
            serde_json::to_string_pretty(&before).unwrap(),
        );
    }
}

#[test]
fn generic_field_change_is_not_whole_node() {
    let before =
        root(json!([{ "id": "a", "componentType": "text", "role": "user", "content": "x" }]));
    let after =
        root(json!([{ "id": "a", "componentType": "text", "role": "assistant", "content": "x" }]));
    let ops = assert_round_trip(&before, &after);
    assert_eq!(ops.len(), 1, "got {ops:?}");
    assert!(matches!(&ops[0].kind, TraceOpKind::SetField { key, value }
        if key == "role" && value == "assistant"));
}

#[test]
fn owned_replay_matches_borrowed_and_preserves_borrowed_error_input() {
    let before = json!({"id":"root", "componentType":"root", "children":[]});
    let after = json!({"id":"root", "componentType":"root", "children":[
        {"id":"n", "componentType":"text_content", "content":"hello"}
    ]});
    let trace = TransactionTrace::diff(0, 1, &before, &after);
    assert_eq!(
        trace.replay(&before).unwrap(),
        trace.replay_owned(before.clone()).unwrap()
    );
    let mut invalid = trace;
    invalid.from_rev = 2;
    invalid.to_rev = 1;
    let retained = before.clone();
    let borrowed_error = invalid.replay(&before).unwrap_err();
    let owned_error = invalid.replay_owned(before.clone()).unwrap_err();
    assert_eq!(borrowed_error.to_string(), owned_error.to_string());
    assert_eq!(borrowed_error.kind(), owned_error.kind());
    assert_eq!(before, retained);
}

fn replay_reference(before: &Value, ops: &[TraceOp]) -> PactResult<Value> {
    replay_with_index(
        before.clone(),
        ops,
        PathIndex {
            enabled: false,
            paths: None,
        },
    )
}

#[test]
fn indexed_replay_matches_the_reference_replay_including_duplicate_ids() {
    let before = json!({"id":"root","componentType":"root","children":[
        {"id":"a","componentType":"message_turn","pact_depth":0,"children":[
            {"id":"dup","componentType":"text_content","content":"first"}]},
        {"id":"dup","componentType":"text_content","content":"second"}
    ]});
    let ops = vec![
        TraceOp::new(
            "dup",
            TraceOpKind::SetField {
                key: "pact_depth".into(),
                value: json!(1),
            },
        ),
        TraceOp::new(
            "b",
            TraceOpKind::InsertComponent {
                parent_id: "root".into(),
                index: 2,
                component: json!({"componentType":"message_turn","pact_depth":0}),
            },
        ),
        TraceOp::new(
            "a",
            TraceOpKind::SetField {
                key: "pact_depth".into(),
                value: json!(1),
            },
        ),
        TraceOp::new(
            "root",
            TraceOpKind::SetField {
                key: "child_ids".into(),
                value: json!(["b", "a", "dup"]),
            },
        ),
        TraceOp::new(
            "dup",
            TraceOpKind::SetAttr {
                key: "note".into(),
                value: json!("x"),
            },
        ),
        TraceOp::new("a", TraceOpKind::DeleteComponent),
        TraceOp::new(
            "dup",
            TraceOpKind::SetField {
                key: "content".into(),
                value: json!("third"),
            },
        ),
    ];
    let reference = replay_reference(&before, &ops).expect("reference");
    let indexed = replay_owned(before.clone(), &ops).expect("indexed");
    assert_eq!(reference, indexed);

    let missing = vec![TraceOp::new(
        "nope",
        TraceOpKind::SetField {
            key: "k".into(),
            value: json!(1),
        },
    )];
    let reference_error = replay_reference(&before, &missing).unwrap_err();
    let indexed_error = replay_owned(before.clone(), &missing).unwrap_err();
    assert_eq!(reference_error.to_string(), indexed_error.to_string());
}
