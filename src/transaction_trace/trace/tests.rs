use super::super::TransactionTrace;
use super::*;
use serde_json::{Value, json};

fn root(children: Value) -> Value {
    json!({ "id": "root", "componentType": "root", "children": children })
}

#[test]
fn trace_summarizes_each_revision_from_the_ledger() {
    let r0 = root(json!([]));
    let r1 = root(json!([{ "id": "a", "componentType": "text", "content": "one" }]));
    let r2 = root(json!([{ "id": "a", "componentType": "text", "content": "two" }]));
    let r3 = root(json!([{ "id": "b", "componentType": "text", "content": "b" }]));
    let traces = vec![
        TransactionTrace::diff(0, 1, &r0, &r1),
        TransactionTrace::diff(1, 2, &r1, &r2),
        TransactionTrace::diff(2, 3, &r2, &r3),
    ];

    let trace = derive_trace(&traces);
    assert_eq!(trace.len(), 3);

    assert_eq!(trace[0].to_rev, 1);
    assert_eq!(trace[0].changed_component_ids, vec!["a".to_string()]);
    assert_eq!(trace[0].op_counts.get("insert_component"), Some(&1));

    assert_eq!(trace[1].changed_component_ids, vec!["a".to_string()]);
    assert!(
        trace[1].op_counts.contains_key("set_content")
            || trace[1].op_counts.contains_key("replace_content_range")
    );

    assert!(trace[2].changed_component_ids.contains(&"a".to_string()));
    assert!(trace[2].changed_component_ids.contains(&"b".to_string()));
    assert_eq!(trace[2].op_counts.get("delete_component"), Some(&1));
    assert_eq!(trace[2].op_counts.get("insert_component"), Some(&1));
}

#[test]
fn trace_is_json_serializable_for_debug() {
    let before = root(json!([{ "id": "a", "componentType": "text", "content": "x" }]));
    let after = root(json!([{ "id": "a", "componentType": "text", "content": "xy" }]));
    let trace = derive_trace(&[TransactionTrace::diff(0, 1, &before, &after)]);
    let json = serde_json::to_string(&trace).expect("serialize");
    let decoded: Vec<RevisionTrace> = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, trace);
}
