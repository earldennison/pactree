use super::super::TransactionTrace;
use super::*;
use serde_json::{Value, json};

fn root(children: Value) -> Value {
    json!({ "id": "root", "componentType": "root", "children": children })
}

fn sample_stream() -> Vec<TransactionTrace> {
    let r0 = root(json!([]));
    let r1 = root(json!([{ "id": "a", "componentType": "text", "content": "one" }]));
    let r2 = root(json!([{ "id": "a", "componentType": "text", "content": "two" }]));
    let r3 = root(json!([{ "id": "b", "componentType": "text", "content": "b" }]));
    vec![
        TransactionTrace::diff(0, 1, &r0, &r1),
        TransactionTrace::diff(1, 2, &r1, &r2),
        TransactionTrace::diff(2, 3, &r2, &r3),
    ]
}

#[test]
fn index_records_touched_nodes_per_revision() {
    let traces = sample_stream();
    let index = TransactionTraceIndex::build(&traces);

    assert_eq!(
        index.touched_by_revision[&1],
        BTreeSet::from(["a".to_string()])
    );
    assert_eq!(
        index.touched_by_revision[&2],
        BTreeSet::from(["a".to_string()])
    );

    assert_eq!(
        index.touched_by_revision[&3],
        BTreeSet::from(["a".to_string(), "b".to_string()])
    );

    assert_eq!(index.revisions_by_node["a"], vec![1, 2, 3]);
    assert_eq!(index.revisions_by_node["b"], vec![3]);
    assert_eq!(index.inserted_at["a"], 1);
    assert_eq!(index.inserted_at["b"], 3);
    assert_eq!(index.deleted_at["a"], 3);
    assert!(!index.deleted_at.contains_key("b"));
}

#[test]
fn window_query_returns_only_touched_nodes() {
    let index = TransactionTraceIndex::build(&sample_stream());

    assert_eq!(
        index.touched_in_window(1, 2),
        BTreeSet::from(["a".to_string()])
    );

    assert_eq!(
        index.touched_in_window(0, 3),
        BTreeSet::from(["a".to_string(), "b".to_string()])
    );
}

#[test]
fn live_at_tracks_insert_and_delete() {
    let index = TransactionTraceIndex::build(&sample_stream());
    assert!(index.is_live_at("a", 1));
    assert!(index.is_live_at("a", 2));
    assert!(!index.is_live_at("a", 3), "a is tombstoned at rev 3");
    assert!(!index.is_live_at("b", 2), "b not inserted until rev 3");
    assert!(index.is_live_at("b", 3));
}

#[test]
fn type_and_membership_indexes_track_head_state() {
    let r0 = root(json!([]));
    let r1 = root(json!([
        { "id": "turn", "componentType": "message_turn", "children": [
            { "id": "t1", "componentType": "text" },
            { "id": "t2", "componentType": "text" },
        ]},
        { "id": "other", "componentType": "message_turn", "children": [] },
    ]));

    let r2 = root(json!([
        { "id": "turn", "componentType": "message_turn", "children": [
            { "id": "t1", "componentType": "text" },
        ]},
        { "id": "other", "componentType": "message_turn", "children": [
            { "id": "t2", "componentType": "text" },
        ]},
    ]));
    let index = TransactionTraceIndex::build(&[
        TransactionTrace::diff(0, 1, &r0, &r1),
        TransactionTrace::diff(1, 2, &r1, &r2),
    ]);

    assert_eq!(
        index.nodes_of_type("message_turn"),
        BTreeSet::from(["turn".to_string(), "other".to_string()])
    );
    assert_eq!(
        index.nodes_of_type("text"),
        BTreeSet::from(["t1".to_string(), "t2".to_string()])
    );

    assert_eq!(
        index.children_of("turn"),
        BTreeSet::from(["t1".to_string()])
    );
    assert_eq!(
        index.children_of("other"),
        BTreeSet::from(["t2".to_string()])
    );
    assert_eq!(
        index.children_of("root"),
        BTreeSet::from(["turn".to_string(), "other".to_string()])
    );
}

#[test]
fn is_live_at_handles_delete_then_reinsert_exactly() {
    let r0 = root(json!([]));
    let r1 = root(json!([{ "id": "x", "componentType": "text", "content": "first" }]));
    let r2 = root(json!([]));
    let r3 = root(json!([{ "id": "x", "componentType": "text", "content": "again" }]));
    let index = TransactionTraceIndex::build(&[
        TransactionTrace::diff(0, 1, &r0, &r1),
        TransactionTrace::diff(1, 2, &r1, &r2),
        TransactionTrace::diff(2, 3, &r2, &r3),
    ]);
    assert!(!index.is_live_at("x", 0), "x not yet inserted at rev 0");
    assert!(index.is_live_at("x", 1), "x alive after insert");
    assert!(
        !index.is_live_at("x", 2),
        "x dead during the deleted interval"
    );
    assert!(index.is_live_at("x", 3), "x alive again after re-insert");

    assert!(
        !index.deleted_at.contains_key("x"),
        "re-insert must clear x's tombstone"
    );
}

#[test]
fn is_live_at_tombstones_whole_deleted_subtree() {
    let r0 = root(json!([]));
    let r1 = root(json!([
        { "id": "turn", "componentType": "message_turn", "children": [
            { "id": "child", "componentType": "text", "children": [
                { "id": "grandchild", "componentType": "text" },
            ]},
        ]},
    ]));
    let r2 = root(json!([]));
    let index = TransactionTraceIndex::build(&[
        TransactionTrace::diff(0, 1, &r0, &r1),
        TransactionTrace::diff(1, 2, &r1, &r2),
    ]);

    for id in ["turn", "child", "grandchild"] {
        assert!(index.is_live_at(id, 1), "{id} should be live at rev 1");
        assert!(
            !index.is_live_at(id, 2),
            "{id} must be tombstoned with the deleted subtree at rev 2"
        );
        assert_eq!(index.deleted_at.get(id), Some(&2), "{id} deleted_at");
    }
}

#[test]
fn delete_removes_subtree_from_type_and_membership_indexes() {
    let r0 = root(json!([]));
    let r1 = root(json!([
        { "id": "turn", "componentType": "message_turn", "children": [
            { "id": "t1", "componentType": "text" },
            { "id": "t2", "componentType": "text" },
        ]},
    ]));

    let r2 = root(json!([]));
    let index = TransactionTraceIndex::build(&[
        TransactionTrace::diff(0, 1, &r0, &r1),
        TransactionTrace::diff(1, 2, &r1, &r2),
    ]);

    assert!(index.nodes_of_type("message_turn").is_empty());
    assert!(index.nodes_of_type("text").is_empty());
    assert!(index.children_of("turn").is_empty());
    assert!(index.children_of("root").is_empty());
}

#[test]
fn historical_selector_uses_touched_window_not_full_replay() {
    let mut trees = Vec::new();
    let mut children = Vec::new();
    for i in 0..50u64 {
        children.push(json!({ "id": format!("n{i}"), "componentType": "text", "content": "x" }));
        trees.push(root(Value::Array(children.clone())));
    }

    let mut edited = trees.last().unwrap().clone();
    edited["children"][7]["content"] = json!("EDITED");
    trees.push(edited);

    let mut traces = Vec::new();
    let mut prev = root(json!([]));
    for (i, tree) in trees.iter().enumerate() {
        traces.push(TransactionTrace::diff(i as u64, i as u64 + 1, &prev, tree));
        prev = tree.clone();
    }
    let index = TransactionTraceIndex::build(&traces);

    let window = index.touched_in_window(50, 51);
    assert_eq!(
        window,
        BTreeSet::from(["n7".to_string()]),
        "windowed query must touch only the edited node"
    );
    assert!(
        window.len() < index.nodes_by_type["text"].len(),
        "window ({}) must be far smaller than the full node set ({})",
        window.len(),
        index.nodes_by_type["text"].len()
    );
}

#[test]
fn index_is_a_pure_derived_cache() {
    let traces = sample_stream();

    assert_eq!(
        TransactionTraceIndex::build(&traces),
        TransactionTraceIndex::build(&traces)
    );

    let r0 = root(json!([]));
    let mut replayed = r0.clone();
    for trace in &traces {
        replayed = trace.replay(&replayed).expect("replay");
    }
    let _index = TransactionTraceIndex::build(&traces);
    let mut replayed_again = r0;
    for trace in &traces {
        replayed_again = trace.replay(&replayed_again).expect("replay");
    }
    assert_eq!(replayed, replayed_again);

    assert_eq!(
        replayed,
        root(json!([{ "id": "b", "componentType": "text", "content": "b" }]))
    );
}
