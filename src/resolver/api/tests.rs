use super::*;
use crate::mutation::{NodeDepthShift, NodeOffsetShift, ParentChange};
use serde_json::json;

fn request(kind: MutationKind) -> MutationRequest {
    MutationRequest {
        schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
        request_id: None,
        transaction_id: None,
        kind,
        target: MutationTarget::default(),
        node: None,
        mode: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    }
}

#[test]
fn direct_insert_trace_ops_replay_nested_subtree() {
    let before_tree =
        ComponentTree::from_value(&json!({ "id": "root", "componentType": "root" })).unwrap();
    let after_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "a",
            "componentType": "container",
            "children": [{ "id": "b", "componentType": "text_content", "content": "nested" }]
        }]
    }))
    .unwrap();
    let mut report = OperationReport::empty();
    report.inserted_ids = vec!["a".to_string()];
    let mut req = request(MutationKind::Insert);
    req.target.parent_id = Some("root".to_string());

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op.kind, TraceOpKind::InsertComponent { .. }))
            .count(),
        2
    );
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, .. } if key == "child_ids"
    )));
    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_insert_trace_ops_replay_into_explicit_empty_children() {
    let before_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": []
    }))
    .unwrap();
    let after_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{ "id": "a", "componentType": "text_content", "content": "inserted" }]
    }))
    .unwrap();
    let mut report = OperationReport::empty();
    report.inserted_ids = vec!["a".to_string()];
    let mut req = request(MutationKind::Insert);
    req.target.parent_id = Some("root".to_string());

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_insert_trace_ops_replay_after_existing_sibling() {
    let before_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{ "id": "first", "componentType": "text_content", "content": "first" }]
    }))
    .unwrap();
    let after_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [
            { "id": "first", "componentType": "text_content", "content": "first" },
            { "id": "second", "componentType": "text_content", "content": "second" }
        ]
    }))
    .unwrap();
    let mut report = OperationReport::empty();
    report.inserted_ids = vec!["second".to_string()];
    report.shifted_offsets = vec![];
    let mut req = request(MutationKind::Insert);
    req.target.parent_id = Some("root".to_string());

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_update_trace_ops_replay_content_and_aux() {
    let before_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{ "id": "a", "componentType": "text_content", "content": "old" }]
    }))
    .unwrap();
    let after_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "a",
            "componentType": "text_content",
            "content": "new",
            "aux": { "tone": "clear" }
        }]
    }))
    .unwrap();
    let mut report = OperationReport::empty();
    report.replaced_ids = vec!["a".to_string()];
    let mut req = request(MutationKind::Update);
    req.target.component_id = Some("a".to_string());
    req.content = Some("new".to_string());
    req.attrs.insert("tone".to_string(), json!("clear"));

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert!(
        ops.iter()
            .any(|op| matches!(op.kind, TraceOpKind::SetContent { .. }))
    );
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetAttr { key, .. } if key == "tone"
    )));
    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_replace_trace_ops_replay_full_subtree() {
    let before_tree = ComponentTree::from_value(&json!({
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "a",
                "componentType": "container",
                "content": "old",
                "children": [{ "id": "old_child", "componentType": "text_content", "content": "old child" }]
            }]
        }))
        .unwrap();
    let after_tree = ComponentTree::from_value(&json!({
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "a",
                "componentType": "container",
                "content": "new",
                "children": [{ "id": "new_child", "componentType": "text_content", "content": "new child" }]
            }]
        }))
        .unwrap();
    let mut report = OperationReport::empty();
    report.replaced_ids = vec!["a".to_string()];
    let mut req = request(MutationKind::Update);
    req.target.component_id = Some("a".to_string());
    req.node = Some(
            Component::from_component_value(&json!({
                "id": "a",
                "componentType": "container",
                "content": "new",
                "children": [{ "id": "new_child", "componentType": "text_content", "content": "new child" }]
            }))
            .expect("component"),
        );

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert!(
        ops.iter()
            .any(|op| matches!(op.kind, TraceOpKind::DeleteComponent) && op.component_id == "a")
    );
    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op.kind, TraceOpKind::InsertComponent { .. }))
            .count(),
        2
    );
    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_delete_trace_ops_replay_component_tombstone() {
    let before_tree = ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{ "id": "a", "componentType": "text_content", "content": "bye" }]
    }))
    .unwrap();
    let after_tree =
        ComponentTree::from_value(&json!({ "id": "root", "componentType": "root" })).unwrap();
    let mut report = OperationReport::empty();
    report.removed_ids = vec!["a".to_string()];
    let mut req = request(MutationKind::Delete);
    req.target.component_id = Some("a".to_string());

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op.kind, TraceOpKind::DeleteComponent))
            .count(),
        1
    );
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, .. } if key == "child_ids"
    )));
    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_delete_trace_ops_replay_selector_multi_delete_and_compaction() {
    let before_tree = ComponentTree::from_value(&json!({
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "parent",
                "componentType": "message_container",
                "children": [
                    { "id": "delete_a", "componentType": "text_content", "offset": 0, "pact_depth": 1, "content": "a" },
                    {
                        "id": "delete_b",
                        "componentType": "message_container",
                        "offset": 1,
                        "pact_depth": 1,
                        "children": [{ "id": "delete_b_child", "componentType": "text_content", "content": "child" }]
                    },
                    { "id": "keep", "componentType": "text_content", "offset": 2, "pact_depth": 3, "content": "keep" }
                ]
            }]
        }))
        .unwrap();
    let after_tree = ComponentTree::from_value(&json!({
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "parent",
                "componentType": "message_container",
                "children": [
                    { "id": "keep", "componentType": "text_content", "offset": 0, "pact_depth": 1, "content": "keep" }
                ]
            }]
        }))
        .unwrap();
    let mut report = OperationReport::empty();
    report.removed_ids = vec![
        "delete_a".to_string(),
        "delete_b".to_string(),
        "delete_b_child".to_string(),
    ];
    report.shifted_offsets = vec![NodeOffsetShift {
        component_id: "keep".to_string(),
        parent_id: Some("parent".to_string()),
        from_offset: 2,
        to_offset: 0,
    }];
    report.shifted_depths = vec![NodeDepthShift {
        component_id: "keep".to_string(),
        from_depth: 3,
        to_depth: 1,
    }];
    let mut req = request(MutationKind::Delete);
    req.target.selector = Some("[data-prune=true]".to_string());

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op.kind, TraceOpKind::DeleteComponent))
            .count(),
        2,
        "descendants of deleted subtrees are removed implicitly"
    );
    assert!(
        !ops.iter().any(|op| {
            op.component_id == "delete_b_child" && matches!(op.kind, TraceOpKind::DeleteComponent)
        }),
        "child tombstones are redundant when the parent subtree is deleted"
    );
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, value }
            if key == "offset" && value.as_i64() == Some(0)
    )));
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, value }
            if key == "pact_depth" && value.as_i64() == Some(1)
    )));
    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}

#[test]
fn direct_move_trace_ops_replay_reparent_and_order() {
    let before_tree = ComponentTree::from_value(&json!({
            "id": "root",
            "componentType": "root",
            "children": [
                {
                    "id": "left",
                    "componentType": "message_container",
                    "children": [{ "id": "moved", "componentType": "text_content", "offset": 0, "pact_depth": 0, "content": "move me" }]
                },
                {
                    "id": "right",
                    "componentType": "message_container",
                    "children": [{ "id": "existing", "componentType": "text_content", "offset": 1, "pact_depth": 2, "content": "there" }]
                }
            ]
        }))
        .unwrap();
    let after_tree = ComponentTree::from_value(&json!({
            "id": "root",
            "componentType": "root",
            "children": [
                { "id": "left", "componentType": "message_container" },
                {
                    "id": "right",
                    "componentType": "message_container",
                    "children": [
                        { "id": "moved", "componentType": "text_content", "parent_id": "right", "offset": 0, "pact_depth": 2, "content": "move me" },
                        { "id": "existing", "componentType": "text_content", "offset": 2, "pact_depth": 2, "content": "there" }
                    ]
                }
            ]
        }))
        .unwrap();
    let mut report = OperationReport::empty();
    report.parent_changes = vec![ParentChange {
        component_id: "moved".to_string(),
        from_parent_id: Some("left".to_string()),
        to_parent_id: Some("right".to_string()),
    }];
    report.shifted_offsets = vec![
        NodeOffsetShift {
            component_id: "existing".to_string(),
            parent_id: Some("right".to_string()),
            from_offset: 1,
            to_offset: 2,
        },
        NodeOffsetShift {
            component_id: "moved".to_string(),
            parent_id: Some("right".to_string()),
            from_offset: 0,
            to_offset: 0,
        },
    ];
    report.shifted_depths = vec![NodeDepthShift {
        component_id: "moved".to_string(),
        from_depth: 0,
        to_depth: 2,
    }];
    let mut req = request(MutationKind::Move);
    req.target.component_id = Some("moved".to_string());
    req.target.parent_id = Some("right".to_string());
    req.target.offset = Some(0);
    req.target.depth = Some(2);

    let ops = mutation_result_trace_ops(&req, &report, &before_tree, &after_tree, false)
        .unwrap()
        .expect("direct ops");

    assert!(
        ops.iter()
            .any(|op| matches!(op.kind, TraceOpKind::MoveComponent { .. }))
    );
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, .. } if key == "parent_id"
    )));
    assert!(ops.iter().any(|op| matches!(
        &op.kind,
        TraceOpKind::SetField { key, .. } if key == "pact_depth"
    )));
    assert!(trace_ops_replay_to(
        &before_tree.project_value(),
        &after_tree.project_value(),
        &ops
    ));
}
