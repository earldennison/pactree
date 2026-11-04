use super::{CrossContextMessage, PactContextRef, send_context_message, text_message_part};
use crate::error::PactError;
use crate::file_format::{
    PactForkMode, file_create_from_ledger_json, file_fork_json, file_open_ledger,
};
use crate::store::ContextTreeLedger;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("xcmsg_{name}_{nanos}"));
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn open_ledger(path: &Path) -> ContextTreeLedger {
    file_open_ledger(path).expect("decode ledger")
}

fn source_rev0_json() -> String {
    let tree = json!({
        "id": "root_a",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "attrs": {"cycle": 1},
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "attrs": {"cycle": 1},
                "content": "base content from A"
            }]
        }]
    });
    crate::store::new_ledger_json(&tree.to_string(), Some("main"), 8).expect("source rev0 ledger")
}

fn find_node<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if node.get("id").and_then(Value::as_str) == Some(id) {
        return Some(node);
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = find_node(child, id) {
            return Some(found);
        }
    }
    None
}

fn find_cross_context_provenance(node: &Value) -> Option<&Value> {
    if let Some(provenance) = node
        .get("aux")
        .and_then(|aux| aux.get("cross_context_message"))
        .or_else(|| {
            node.get("attrs")
                .and_then(|attrs| attrs.get("cross_context_message"))
        })
        .or_else(|| node.get("cross_context_message"))
    {
        return Some(provenance);
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = find_cross_context_provenance(child) {
            return Some(found);
        }
    }
    None
}

fn add_local_trace(path: &Path, node_id: &str, text: &str) {
    use crate::components::Component;
    use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
    use crate::resolver::PactResolver;
    use crate::trace::BoundaryKind;

    let ledger = open_ledger(path);
    let branch_id = ledger.current_branch_id.clone();
    let root_id = ledger.root_id.clone();
    let component = Component::from_component_value(&json!({
        "id": node_id,
        "componentType": "text_content",
        "type": "text_content",
        "content": text,
    }))
    .expect("local node component");
    let op = PactOperation::Append {
        envelope: PactOperationEnvelope {
            branch_id: branch_id.clone(),
            source: SourceActor::Host,
            source_id: Some("local_activity".to_string()),
            host_timestamp_ns: None,
            idempotency_key: None,
            aux: BTreeMap::new(),
        },
        target_component_id: Some(root_id),
        target_selector: None,
        offset: None,
        component,
    };
    let mut resolver = PactResolver::from_ledger(ledger);
    let outcome = resolver
        .resolve(vec![op], &branch_id, BoundaryKind::Authored)
        .expect("resolve local trace");
    let candidate = outcome
        .commit_candidate
        .expect("local trace commit candidate");
    resolver.seal(&candidate).expect("seal local trace");
    let mutated_json =
        serde_json::to_string(&resolver.into_ledger()).expect("encode mutated ledger");
    file_create_from_ledger_json(path, &mutated_json, 8).expect("rewrite local trace file");
}

fn project_head_tree(path: &Path) -> Value {
    let json =
        crate::file_format::file_project_tree_json(path, None, None).expect("project head tree");
    serde_json::from_str(&json).expect("decode projected tree")
}

#[test]
fn o_local_traces_do_not_appear_in_a_by_projection() {
    let dir = unique_dir("no_leak");
    let a = dir.join("a.pact");
    let o = dir.join("o.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&a, &source_json, 8).expect("create A");
    let base_root_id = open_ledger(&a).root_id;

    file_fork_json(
        &a,
        Some(&o),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork O over A");

    let a_bytes_before = fs::read(&a).expect("read A before");

    add_local_trace(&o, "o_local_text", "O-only local note");

    let a_bytes_after = fs::read(&a).expect("read A after");
    assert_eq!(
        a_bytes_before, a_bytes_after,
        "O-local traces must never mutate A's file"
    );

    let a_tree = project_head_tree(&a);
    assert!(
        find_node(&a_tree, "text_1").is_some(),
        "A still has its own content"
    );
    assert!(
        find_node(&a_tree, "o_local_text").is_none(),
        "O-local trace must NOT appear in A by projection"
    );

    let o_ledger = open_ledger(&o);
    assert!(
        o_ledger.base.is_some(),
        "O references A as base (visibility)"
    );
    assert_eq!(
        o_ledger
            .base
            .as_ref()
            .map(|base| base.base_ref.root_id.as_str()),
        Some(base_root_id.as_str()),
    );
    let o_tree = project_head_tree(&o);
    assert!(
        find_node(&o_tree, "o_local_text").is_some(),
        "O's own context carries the O-local trace"
    );

    assert!(
        find_node(&o_tree, "text_1").is_none(),
        "O does not copy A's content into its local ledger (base is reference, not copy)"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn cross_context_message_o_to_a_appears_in_a_with_provenance() {
    let dir = unique_dir("o_to_a");
    let a = dir.join("a.pact");
    let o = dir.join("o.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&a, &source_json, 8).expect("create A");
    let a_root_id = open_ledger(&a).root_id;

    file_fork_json(
        &a,
        Some(&o),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork O over A");
    let o_root_id = open_ledger(&o).root_id;

    let a_tree_before = project_head_tree(&a);
    assert!(
        find_cross_context_provenance(&a_tree_before).is_none(),
        "A has no delivered message before the op"
    );

    let part = text_message_part("hello from O").expect("part");
    let report = send_context_message(CrossContextMessage {
        from_context: PactContextRef::at_path(o_root_id.clone(), &o),
        to_context: PactContextRef::at_path(a_root_id.clone(), &a),
        parts: vec![part],
        source_trace_id: Some("trace_o_42".to_string()),
        source_tool_call_id: None,
        created_at: Some("2026-06-05T00:00:00Z".to_string()),
        aux: BTreeMap::new(),
    })
    .expect("send O -> A");

    assert_eq!(report.from_root_id, o_root_id);
    assert_eq!(report.to_root_id, a_root_id);
    assert_eq!(report.source_trace_id.as_deref(), Some("trace_o_42"));
    assert!(!report.delivery_trace_id.is_empty());

    let a_tree_after = project_head_tree(&a);
    let delivered = find_node(&a_tree_after, &report.delivered_component_id)
        .expect("delivered message turn is in A");
    assert!(
        find_node(delivered, &report.delivered_component_id).is_some(),
        "delivered turn present"
    );
    let provenance = find_cross_context_provenance(&a_tree_after)
        .expect("delivered message carries cross-context provenance");
    assert_eq!(provenance["from_context"]["root_id"], json!(o_root_id));
    assert_eq!(provenance["to_context"]["root_id"], json!(a_root_id));
    assert_eq!(provenance["source_trace_id"], json!("trace_o_42"));
    assert_eq!(provenance["created_at"], json!("2026-06-05T00:00:00Z"));
    assert_eq!(
        provenance["delivery_trace_id"],
        json!(report.delivery_trace_id)
    );

    assert!(
        a_tree_after.to_string().contains("hello from O"),
        "delivered payload text is in A"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_to_o_and_o_to_a_both_work_through_same_op() {
    let dir = unique_dir("bidirectional");
    let a = dir.join("a.pact");
    let o = dir.join("o.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&a, &source_json, 8).expect("create A");
    let a_root_id = open_ledger(&a).root_id;

    file_fork_json(
        &a,
        Some(&o),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork O over A");
    let o_root_id = open_ledger(&o).root_id;

    let a_to_o = send_context_message(CrossContextMessage {
        from_context: PactContextRef::at_path(a_root_id.clone(), &a),
        to_context: PactContextRef::at_path(o_root_id.clone(), &o),
        parts: vec![text_message_part("from A to O").expect("part")],
        source_trace_id: None,
        source_tool_call_id: Some("toolcall_a_7".to_string()),
        created_at: Some("2026-06-05T00:00:01Z".to_string()),
        aux: BTreeMap::new(),
    })
    .expect("send A -> O");
    assert_eq!(a_to_o.from_root_id, a_root_id);
    assert_eq!(a_to_o.to_root_id, o_root_id);
    assert_eq!(a_to_o.source_tool_call_id.as_deref(), Some("toolcall_a_7"));

    let o_to_a = send_context_message(CrossContextMessage {
        from_context: PactContextRef::at_path(o_root_id.clone(), &o),
        to_context: PactContextRef::at_path(a_root_id.clone(), &a),
        parts: vec![text_message_part("from O to A").expect("part")],
        source_trace_id: None,
        source_tool_call_id: None,
        created_at: Some("2026-06-05T00:00:02Z".to_string()),
        aux: BTreeMap::new(),
    })
    .expect("send O -> A");
    assert_eq!(o_to_a.from_root_id, o_root_id);
    assert_eq!(o_to_a.to_root_id, a_root_id);

    let o_tree = project_head_tree(&o);
    assert!(
        o_tree.to_string().contains("from A to O"),
        "O received A's message"
    );
    assert!(
        find_node(&o_tree, &a_to_o.delivered_component_id).is_some(),
        "A->O delivered turn present in O"
    );

    let a_tree = project_head_tree(&a);
    assert!(
        a_tree.to_string().contains("from O to A"),
        "A received O's message"
    );
    assert!(
        find_node(&a_tree, &o_to_a.delivered_component_id).is_some(),
        "O->A delivered turn present in A"
    );

    assert!(
        !a_tree.to_string().contains("from A to O"),
        "A->O message must live in O only, never echoed back into A"
    );
    assert!(
        !o_tree.to_string().contains("from O to A"),
        "O->A message must live in A only, never echoed back into O"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn delivery_trace_id_is_deterministic() {
    let dir = unique_dir("deterministic");
    let a1 = dir.join("a1.pact");
    let a2 = dir.join("a2.pact");
    let o = dir.join("o.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&a1, &source_json, 8).expect("create A1");
    file_create_from_ledger_json(&a2, &source_json, 8).expect("create A2");
    let a_root_id = open_ledger(&a1).root_id;
    assert_eq!(
        a_root_id,
        open_ledger(&a2).root_id,
        "both targets share the same root identity"
    );

    file_fork_json(
        &a1,
        Some(&o),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork");
    let o_root_id = open_ledger(&o).root_id;

    let make_message = |target: &Path| CrossContextMessage {
        from_context: PactContextRef::at_path(o_root_id.clone(), &o),
        to_context: PactContextRef::at_path(a_root_id.clone(), target),
        parts: vec![text_message_part("same payload").expect("part")],
        source_trace_id: Some("trace_x".to_string()),
        source_tool_call_id: None,
        created_at: Some("2026-06-05T12:00:00Z".to_string()),
        aux: BTreeMap::new(),
    };

    let first = send_context_message(make_message(&a1)).expect("first delivery");

    let second = send_context_message(make_message(&a2)).expect("second delivery");
    assert_eq!(
        first.delivery_trace_id, second.delivery_trace_id,
        "identical delivery inputs are deterministic (no wall-clock)"
    );
    assert_eq!(
        first.delivered_component_id, second.delivered_component_id,
        "deterministic component id derives from the deterministic trace id"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn wrong_target_root_id_is_typed_mismatch() {
    let dir = unique_dir("wrong_target");
    let a = dir.join("a.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&a, &source_json, 8).expect("create A");

    let err = send_context_message(CrossContextMessage {
        from_context: PactContextRef::at_path("root_o", &a),

        to_context: PactContextRef::at_path("not_the_target_root", &a),
        parts: vec![text_message_part("oops").expect("part")],
        source_trace_id: None,
        source_tool_call_id: None,
        created_at: Some("2026-06-05T00:00:00Z".to_string()),
        aux: BTreeMap::new(),
    })
    .expect_err("wrong target root id must be rejected");
    assert!(
        matches!(err, PactError::ParentMismatch(_)),
        "wrong target identity must be a typed mismatch, got: {err:?}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn missing_target_path_is_typed_invalid() {
    let _ = file_open_ledger;
    let err = send_context_message(CrossContextMessage {
        from_context: PactContextRef {
            root_id: "root_o".to_string(),
            branch_id: None,
            head_ref: None,
            path: None,
        },
        to_context: PactContextRef {
            root_id: "root_a".to_string(),
            branch_id: None,
            head_ref: None,
            path: None,
        },
        parts: vec![text_message_part("oops").expect("part")],
        source_trace_id: None,
        source_tool_call_id: None,
        created_at: None,
        aux: BTreeMap::new(),
    })
    .expect_err("missing target path must be rejected");
    assert!(
        matches!(err, PactError::Invalid(_)),
        "missing target path must be typed invalid, got: {err:?}"
    );
}

#[test]
fn missing_target_file_surfaces_typed_error() {
    let dir = unique_dir("missing_file");
    let missing = dir.join("nonexistent.pact");
    let err = send_context_message(CrossContextMessage {
        from_context: PactContextRef::at_path("root_o", &missing),
        to_context: PactContextRef::at_path("root_a", &missing),
        parts: vec![text_message_part("oops").expect("part")],
        source_trace_id: None,
        source_tool_call_id: None,
        created_at: None,
        aux: BTreeMap::new(),
    })
    .expect_err("missing target file must be rejected");

    assert!(
        matches!(
            err,
            PactError::Storage(_) | PactError::CorruptFile(_) | PactError::NotFound(_)
        ),
        "missing target file must be a typed error, got: {err:?}"
    );

    let _ = fs::remove_dir_all(dir);
}
