use super::*;
use crate::components::{
    ComponentInternalRefs, ComponentLifecycle, ComponentLifecyclePolicy, InternalRef,
    LifecycleState,
};
use std::time::Instant;

const DELETE_RESULT_SCHEMA: &str = "pact.delete_result.v1";

#[derive(Clone, Debug, PartialEq)]
struct DeleteResult {
    schema: String,
    target_ids: Vec<String>,
    deleted_ids: Vec<String>,
    tree: Value,
}

fn apply_mutation_value(
    mut tree: Value,
    mut request: MutationRequest,
) -> PactResult<MutationApplyResult> {
    let preserve_projection_shape = request.kind == MutationKind::Delete;
    let original_tree = preserve_projection_shape.then(|| tree.clone());
    let mut component_tree = ComponentTree::from_value(&tree)?;
    let report = apply_mutation_tree(&mut component_tree, &mut request)?;
    tree = if let Some(mut original_tree) = original_tree {
        let removed_ids: BTreeSet<String> = report.removed_ids.iter().cloned().collect();
        let mut deleted_ids = Vec::new();
        delete_ids_recursive(&mut original_tree, &removed_ids, &mut deleted_ids);
        let _ = compact_after_delete(&mut original_tree);
        original_tree
    } else {
        component_tree.project_value()
    };
    validate_operation_report(&report)?;
    Ok(MutationApplyResult {
        schema: MUTATION_APPLY_RESULT_SCHEMA.to_string(),
        request,
        report,
        tree,
    })
}

fn delete_value(mut tree: Value, selector: &str) -> PactResult<DeleteResult> {
    let mut component_tree = ComponentTree::from_value(&tree)?;
    let target = MutationTarget {
        selector: Some(selector.to_string()),
        ..MutationTarget::default()
    };
    let result = prune_tree(&mut component_tree, &target)?;
    tree = component_tree.project_value();

    Ok(DeleteResult {
        schema: DELETE_RESULT_SCHEMA.to_string(),
        target_ids: result.target_ids,
        deleted_ids: result.deleted_ids,
        tree,
    })
}

fn ids(value: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    visit_nodes(value, &mut |node| {
        if let Some(id) = component_id(node) {
            out.insert(id);
        }
    });
    out
}

fn large_mutation_tree(width: usize) -> Value {
    let stable_children: Vec<Value> = (0..width)
        .map(|index| {
            serde_json::json!({
                "id": format!("stable-{index:05}"),
                "componentType": "text_content",
                "offset": index as i32,
                "content": format!("stable {index}")
            })
        })
        .collect();
    serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "target",
                "componentType": "message_container",
                "offset": 0,
                "children": []
            },
            {
                "id": "stable",
                "componentType": "message_container",
                "offset": 1,
                "children": stable_children
            }
        ]
    })
}

fn append_sequence(mut tree: Value, count: usize, full_sort_after_each: bool) -> Value {
    if !full_sort_after_each {
        let mut component_tree = ComponentTree::from_value(&tree).expect("component tree");
        for index in 0..count {
            let mut request = MutationRequest {
                schema: mutation_request_schema(),
                request_id: None,
                transaction_id: None,
                kind: MutationKind::Update,
                target: MutationTarget {
                    component_id: Some("target".to_string()),
                    ..MutationTarget::default()
                },
                node: Some(
                    Component::from_component_value(&serde_json::json!({
                        "id": format!("new-{index:04}"),
                        "componentType": "text_content",
                        "content": format!("new {index}")
                    }))
                    .expect("node"),
                ),
                mode: Some("append".to_string()),
                content: None,
                content_ref: None,
                fields: BTreeMap::new(),
                attrs: BTreeMap::new(),
            };
            apply_mutation_tree(&mut component_tree, &mut request).expect("append mutation");
        }
        return component_tree.project_value();
    }
    for index in 0..count {
        let request = MutationRequest {
            schema: mutation_request_schema(),
            request_id: None,
            transaction_id: None,
            kind: MutationKind::Update,
            target: MutationTarget {
                component_id: Some("target".to_string()),
                ..MutationTarget::default()
            },
            node: Some(
                Component::from_component_value(&serde_json::json!({
                    "id": format!("new-{index:04}"),
                    "componentType": "text_content",
                    "content": format!("new {index}")
                }))
                .expect("node"),
            ),
            mode: Some("append".to_string()),
            content: None,
            content_ref: None,
            fields: BTreeMap::new(),
            attrs: BTreeMap::new(),
        };
        let mut result = apply_mutation_value(tree, request).expect("append mutation");
        if full_sort_after_each {
            sort_tree_children(&mut result.tree);
        }
        tree = result.tree;
    }
    tree
}

#[test]
fn mutation_bench_1k_on_5k_tree_avoids_full_tree_sort() {
    let base = large_mutation_tree(5_000);
    let start = Instant::now();
    let optimized = append_sequence(base.clone(), 1_000, false);
    let optimized_elapsed = start.elapsed();

    let start = Instant::now();
    let baseline = append_sequence(base, 1_000, true);
    let baseline_elapsed = start.elapsed();

    eprintln!(
        "mutation_bench_1k_on_5k_tree optimized={optimized_elapsed:?} eager_full_sort={baseline_elapsed:?}"
    );
    assert_eq!(ids(&optimized), ids(&baseline));
    assert_eq!(
        optimized["children"][0]["children"]
            .as_array()
            .map(Vec::len),
        Some(1_000)
    );

    assert!(
        baseline_elapsed.as_nanos() > optimized_elapsed.as_nanos().saturating_mul(2),
        "native component-tree mutations should beat eager full JSON re-parse + full-tree sort; optimized={optimized_elapsed:?}, eager_full_sort={baseline_elapsed:?}"
    );
}

#[test]
fn delete_tool_call_deletes_paired_tool_result() {
    let pact = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "pact_depth": 0,
            "children": [{
                "id": "container",
                "componentType": "message_container",
                "children": [
                    {
                        "id": "call",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "toolu_123",
                        "tool_name": "probe"
                    },
                    {
                        "id": "result",
                        "componentType": "tool_result",
                        "type": "tool_result",
                        "tool_call_id": "toolu_123",
                        "tool_name": "probe",
                        "content": "ok"
                    },
                    {
                        "id": "text",
                        "componentType": "text_content",
                        "content": "keep"
                    }
                ]
            }]
        }]
    });

    let result = delete_value(pact, "#call").expect("delete succeeds");
    let remaining = ids(&result.tree);

    assert_eq!(result.target_ids, vec!["call", "result"]);
    assert_eq!(result.deleted_ids, vec!["call", "result"]);
    assert!(!remaining.contains("call"));
    assert!(!remaining.contains("result"));
    assert!(remaining.contains("text"));
}

#[test]
fn delete_tool_result_deletes_paired_tool_call() {
    let pact = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "pact_depth": 0,
            "children": [{
                "id": "container",
                "componentType": "message_container",
                "children": [
                    {
                        "id": "call",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "toolu_abc",
                        "tool_name": "tool_alpha_close"
                    },
                    {
                        "id": "result",
                        "componentType": "tool_result",
                        "type": "tool_result",
                        "tool_call_id": "toolu_abc",
                        "tool_name": "tool_alpha_close"
                    }
                ]
            }]
        }]
    });

    let result = delete_value(pact, "#result").expect("delete succeeds");
    let remaining = ids(&result.tree);

    assert_eq!(result.target_ids, vec!["call", "result"]);
    assert_eq!(result.deleted_ids, vec!["call", "result"]);
    assert!(!remaining.contains("call"));
    assert!(!remaining.contains("result"));
}

#[test]
fn validates_insert_mutation_request_shape() {
    let request_json = serde_json::json!({
        "kind": "insert",
        "target": {
            "parent_id": "container",
            "offset": 1
        },
        "node": {
            "id": "text",
            "componentType": "text_content",
            "type": "text_content",
            "content": "hello"
        }
    })
    .to_string();

    let request = mutation_request_from_json(&request_json).expect("valid request");

    assert_eq!(request.schema, MUTATION_REQUEST_SCHEMA);
    assert_eq!(request.kind, MutationKind::Insert);
    assert_eq!(request.target.parent_id.as_deref(), Some("container"));
    assert_eq!(
        request.node.as_ref().map(|node| node.id.as_str()),
        Some("text")
    );
}

#[test]
fn rejects_insert_without_node() {
    let request_json = serde_json::json!({
        "kind": "insert",
        "target": {
            "parent_id": "container",
            "offset": 1
        }
    })
    .to_string();

    let error = mutation_request_from_json(&request_json).expect_err("rejects missing node");

    assert!(error.to_string().contains("requires a node"));
}

#[test]
fn validates_operation_report_shape() {
    let report_json = serde_json::json!({
        "request_id": "req1",
        "transaction_id": "txn1",
        "inserted_ids": ["text"],
        "shifted_offsets": [{
            "component_id": "old",
            "parent_id": "container",
            "from_offset": 1,
            "to_offset": 2
        }]
    })
    .to_string();

    let report = operation_report_from_json(&report_json).expect("valid report");

    assert_eq!(report.schema, OPERATION_REPORT_SCHEMA);
    assert_eq!(report.inserted_ids, vec!["text"]);
    assert_eq!(report.shifted_offsets[0].component_id, "old");
}

#[test]
fn rejects_empty_operation_report() {
    let error = operation_report_from_json(r#"{"schema":"pact.operation_report.v1"}"#)
        .expect_err("empty report rejected");

    assert!(error.to_string().contains("must describe a change"));
}

#[test]
fn apply_insert_mutation_shifts_sibling_offsets_and_reports() {
    let pact = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "container",
            "componentType": "message_container",
            "children": [{
                "id": "old",
                "componentType": "text_content",
                "offset": 0,
                "content": "old"
            }]
        }]
    });
    let request = MutationRequest {
        schema: mutation_request_schema(),
        request_id: Some("req-insert".to_string()),
        transaction_id: Some("txn-insert".to_string()),
        kind: MutationKind::Insert,
        target: MutationTarget {
            parent_id: Some("container".to_string()),
            offset: Some(0),
            ..MutationTarget::default()
        },
        node: Some(
            Component::from_component_value(&serde_json::json!({
                "id": "new",
                "componentType": "text_content",
                "content": "new"
            }))
            .expect("node"),
        ),
        mode: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    };

    let result = apply_mutation_value(pact, request).expect("applies insert");
    let children = result.tree["children"][0]["children"]
        .as_array()
        .expect("children");

    assert_eq!(result.report.inserted_ids, vec!["new"]);
    assert_eq!(result.report.shifted_offsets[0].component_id, "old");
    assert_eq!(children[0]["id"], "new");
    assert_eq!(children[0]["offset"], 0);
    assert_eq!(children[1]["id"], "old");
    assert_eq!(children[1]["offset"], 1);
}

fn insert_request(parent: &str, offset: i32, id: &str) -> MutationRequest {
    MutationRequest {
        schema: mutation_request_schema(),
        request_id: None,
        transaction_id: None,
        kind: MutationKind::Insert,
        target: MutationTarget {
            parent_id: Some(parent.to_string()),
            offset: Some(offset),
            ..MutationTarget::default()
        },
        node: Some(
            Component::from_component_value(&serde_json::json!({
                "id": id,
                "componentType": "text_content",
                "content": id
            }))
            .expect("node"),
        ),
        mode: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    }
}

fn lifecycle_marker(label: &str) -> ComponentLifecycle {
    ComponentLifecycle {
        state: LifecycleState::RenderRequested,
        revision: 42,
        policy: ComponentLifecyclePolicy {
            ttl: Some(3),
            cad: Some(2),
        },
        internal_refs: ComponentInternalRefs {
            state: Some(InternalRef {
                id: format!("{label}_state"),
                kind: "state".to_string(),
                field: Some("lifecycle".to_string()),
                aux: BTreeMap::new(),
            }),
            projection: Some(InternalRef {
                id: format!("{label}_projection"),
                kind: "projection".to_string(),
                field: None,
                aux: BTreeMap::new(),
            }),
        },
    }
}

fn lifecycle_component(id: &str) -> Component {
    let mut component = Component::from_component_value(&serde_json::json!({
        "id": id,
        "componentType": "text_content",
        "content": id
    }))
    .expect("component");
    component.lifecycle = lifecycle_marker(id);
    component
}

fn mutation_request(
    kind: MutationKind,
    target: MutationTarget,
    node: Option<Component>,
    mode: Option<&str>,
) -> MutationRequest {
    MutationRequest {
        schema: mutation_request_schema(),
        request_id: None,
        transaction_id: None,
        kind,
        target,
        node,
        mode: mode.map(str::to_string),
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    }
}

#[test]
fn native_mutations_preserve_component_lifecycle_without_projection_repair() {
    let root = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "container",
            "componentType": "message_container",
            "children": []
        }]
    });
    let mut tree = ComponentTree::from_value(&root).expect("tree");

    let inserted = lifecycle_component("inserted");
    let inserted_lifecycle = inserted.lifecycle.clone();
    let mut insert = mutation_request(
        MutationKind::Insert,
        MutationTarget {
            parent_id: Some("container".to_string()),
            offset: Some(0),
            ..MutationTarget::default()
        },
        Some(inserted),
        None,
    );
    apply_mutation_tree(&mut tree, &mut insert).expect("insert");
    assert_eq!(
        tree.component("inserted").unwrap().lifecycle,
        inserted_lifecycle
    );

    let appended = lifecycle_component("appended");
    let appended_lifecycle = appended.lifecycle.clone();
    let mut append = mutation_request(
        MutationKind::Update,
        MutationTarget {
            component_id: Some("container".to_string()),
            ..MutationTarget::default()
        },
        Some(appended),
        Some("append"),
    );
    apply_mutation_tree(&mut tree, &mut append).expect("append");
    assert_eq!(
        tree.component("appended").unwrap().lifecycle,
        appended_lifecycle
    );

    let before_update = tree.component("inserted").unwrap().lifecycle.clone();
    let mut update = mutation_request(
        MutationKind::Update,
        MutationTarget {
            component_id: Some("inserted".to_string()),
            ..MutationTarget::default()
        },
        None,
        Some("in_place"),
    );
    update.content = Some("updated content".to_string());
    apply_mutation_tree(&mut tree, &mut update).expect("update");
    assert_eq!(tree.component("inserted").unwrap().lifecycle, before_update);

    let replacement = lifecycle_component("inserted");
    let replacement_lifecycle = replacement.lifecycle.clone();
    let mut replace = mutation_request(
        MutationKind::Update,
        MutationTarget {
            component_id: Some("inserted".to_string()),
            ..MutationTarget::default()
        },
        Some(replacement),
        None,
    );
    apply_mutation_tree(&mut tree, &mut replace).expect("replace");
    assert_eq!(
        tree.component("inserted").unwrap().lifecycle,
        replacement_lifecycle
    );

    let projected = tree.project_value();
    assert_eq!(projected["children"][0]["children"][0]["ttl"], 3);
    assert_eq!(projected["children"][0]["children"][0]["cad"], 2);
    assert!(
        projected["children"][0]["children"][0]["lifecycle"]
            .get("internal_refs")
            .is_none(),
        "normal projection must keep internal_refs inspect-only"
    );
    let reparsed = ComponentTree::from_value(&projected).expect("reparsed tree");
    assert_eq!(
        reparsed.component("inserted").unwrap().lifecycle.policy,
        replacement_lifecycle.policy
    );
    assert_eq!(
        reparsed.component("inserted").unwrap().lifecycle.state,
        replacement_lifecycle.state
    );
    assert!(
        reparsed
            .component("inserted")
            .unwrap()
            .lifecycle
            .internal_refs
            .is_empty(),
        "internal refs are intentionally not restored from normal projection"
    );

    let appended_before_prune = tree.component("appended").unwrap().lifecycle.clone();
    let mut delete = mutation_request(
        MutationKind::Delete,
        MutationTarget {
            component_id: Some("inserted".to_string()),
            ..MutationTarget::default()
        },
        None,
        None,
    );
    apply_mutation_tree(&mut tree, &mut delete).expect("delete");
    assert!(tree.component("inserted").is_none());
    assert_eq!(
        tree.component("appended").unwrap().lifecycle,
        appended_before_prune
    );
}

#[test]
fn native_mutations_refresh_private_component_tree_index_buckets() {
    let mut tree = ComponentTree::from_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "container",
            "componentType": "message_container",
            "children": [{
                "id": "indexed",
                "componentType": "text_content",
                "content": "old",
                "key": "slot:old",
                "tags": ["old"],
                "ttl": 2
            }, {
                "id": "tool_parent",
                "componentType": "message_turn",
                "children": [{
                    "id": "call_indexed",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_indexed",
                    "tool_name": "shell",
                    "parameters": {}
                }, {
                    "id": "result_indexed",
                    "componentType": "tool_result",
                    "tool_call_id": "tc_indexed",
                    "success": true,
                    "result": "ok"
                }]
            }]
        }]
    }))
    .expect("tree");

    let mut replacement = Component::from_component_value(&serde_json::json!({
        "id": "indexed",
        "componentType": "tool_call",
        "tool_call_id": "tc_replaced",
        "tool_name": "python",
        "parameters": {},
        "key": "slot:new",
        "tags": ["new"],
        "ttl": 5,
        "cad": 1
    }))
    .expect("replacement");
    replacement.lifecycle.state = LifecycleState::RenderRequested;

    let mut replace = mutation_request(
        MutationKind::Update,
        MutationTarget {
            component_id: Some("indexed".to_string()),
            ..MutationTarget::default()
        },
        Some(replacement),
        None,
    );
    apply_mutation_tree(&mut tree, &mut replace).expect("replace");

    assert!(tree.ids_by_type("text_content").is_empty());
    assert_eq!(
        tree.ids_by_type("tool_call"),
        &["indexed".to_string(), "call_indexed".to_string()]
    );
    assert_eq!(tree.id_by_key("slot:new"), Some("indexed"));
    assert!(tree.id_by_key("slot:old").is_none());
    assert_eq!(tree.ids_by_tag("new"), &["indexed".to_string()]);
    assert_eq!(tree.ttl_candidate_ids(), &["indexed".to_string()]);
    assert_eq!(tree.cad_candidate_ids(), &["indexed".to_string()]);
    assert_eq!(
        tree.ids_by_lifecycle_state(LifecycleState::RenderRequested),
        &["indexed".to_string()]
    );
    assert_eq!(
        tree.tool_pair_ids("tc_replaced").call_ids,
        vec!["indexed".to_string()]
    );

    let mut delete = mutation_request(
        MutationKind::Delete,
        MutationTarget {
            component_id: Some("tool_parent".to_string()),
            ..MutationTarget::default()
        },
        None,
        None,
    );
    apply_mutation_tree(&mut tree, &mut delete).expect("delete subtree");

    assert!(tree.component("call_indexed").is_none());
    assert!(tree.tool_pair_ids("tc_indexed").call_ids.is_empty());
    assert!(tree.tool_pair_ids("tc_indexed").result_ids.is_empty());
    assert_eq!(tree.ids_by_type("tool_call"), &["indexed".to_string()]);
}

#[test]
fn apply_move_mutation_reparents_with_offset_and_depth() {
    let mut tree = ComponentTree::from_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "left",
                "componentType": "message_container",
                "pact_depth": 0,
                "children": [{
                    "id": "moved",
                    "componentType": "text_content",
                    "offset": 0,
                    "pact_depth": 0,
                    "content": "move me"
                }]
            },
            {
                "id": "right",
                "componentType": "message_container",
                "pact_depth": 2,
                "children": [{
                    "id": "existing",
                    "componentType": "text_content",
                    "offset": 0,
                    "pact_depth": 2,
                    "content": "there"
                }]
            }
        ]
    }))
    .expect("tree");
    let mut request = mutation_request(
        MutationKind::Move,
        MutationTarget {
            component_id: Some("moved".to_string()),
            parent_id: Some("right".to_string()),
            offset: Some(0),
            depth: Some(2),
            ..MutationTarget::default()
        },
        None,
        None,
    );

    let report = apply_mutation_tree(&mut tree, &mut request).expect("move");

    assert_eq!(report.parent_changes.len(), 1);
    assert_eq!(
        report.parent_changes[0].from_parent_id.as_deref(),
        Some("left")
    );
    assert_eq!(
        report.parent_changes[0].to_parent_id.as_deref(),
        Some("right")
    );
    assert_eq!(tree.parent_id("moved"), Some("right"));
    let moved = tree.component("moved").expect("moved component");
    assert_eq!(moved.offset, 0);
    assert_eq!(moved.pact_depth, Some(2));
    assert!(tree.child_ids("left").is_empty());
    assert_eq!(
        tree.child_ids("right"),
        &["moved".to_string(), "existing".to_string()]
    );
}

fn empty_anchor_tree(anchor_id: &str, node_type: &str, pact_depth: i32) -> Value {
    serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": anchor_id,
            "componentType": node_type,
            "pact_depth": pact_depth,
            "offset": 0,
            "children": []
        }]
    })
}

#[test]
fn insert_negative_offset_into_empty_parent_bands_above_content() {
    let pact = empty_anchor_tree("system", "system_header", -1);
    let result = apply_mutation_value(pact, insert_request("system", -1, "band_above"))
        .expect("applies insert");
    let children = result.tree["children"][0]["children"]
        .as_array()
        .expect("children");
    assert_eq!(children[0]["id"], "band_above");
    assert_eq!(children[0]["offset"], -1);
}

#[test]
fn insert_zero_offset_into_empty_parent_takes_content_slot() {
    let pact = empty_anchor_tree("system", "system_header", -1);
    let result =
        apply_mutation_value(pact, insert_request("system", 0, "content")).expect("applies insert");
    let children = result.tree["children"][0]["children"]
        .as_array()
        .expect("children");
    assert_eq!(children[0]["id"], "content");
    assert_eq!(children[0]["offset"], 0);
}

#[test]
fn insert_bands_above_and_below_existing_content() {
    let pact = empty_anchor_tree("system", "system_header", -1);
    let pact = apply_mutation_value(pact, insert_request("system", 0, "content"))
        .expect("content insert")
        .tree;
    let pact = apply_mutation_value(pact, insert_request("system", -1, "above"))
        .expect("above insert")
        .tree;
    let result =
        apply_mutation_value(pact, insert_request("system", 1, "below")).expect("below insert");
    let mut offsets: Vec<(String, i64)> = result.tree["children"][0]["children"]
        .as_array()
        .expect("children")
        .iter()
        .map(|child| {
            (
                child["id"].as_str().unwrap_or_default().to_string(),
                child["offset"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    offsets.sort_by_key(|(_, offset)| *offset);
    assert_eq!(offsets[0], ("above".to_string(), -1));
    assert_eq!(offsets[1], ("content".to_string(), 0));
    assert_eq!(offsets[2], ("below".to_string(), 1));
}

#[test]
fn apply_update_mutation_updates_content_and_reports_replacement() {
    let pact = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text",
            "componentType": "text_content",
            "content": "before"
        }]
    });
    let request = MutationRequest {
        schema: mutation_request_schema(),
        request_id: None,
        transaction_id: None,
        kind: MutationKind::Update,
        target: MutationTarget {
            component_id: Some("text".to_string()),
            ..MutationTarget::default()
        },
        node: None,
        mode: Some("in_place".to_string()),
        content: Some("after".to_string()),
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    };

    let result = apply_mutation_value(pact, request).expect("applies update");

    assert_eq!(result.report.replaced_ids, vec!["text"]);
    assert_eq!(result.tree["children"][0]["content"], "after");
}

#[test]
fn apply_update_append_mutation_appends_under_target_and_reports_insert() {
    let pact = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "container",
            "componentType": "message_container",
            "children": [{
                "id": "old",
                "componentType": "text_content",
                "offset": 0,
                "content": "old"
            }]
        }]
    });
    let request = MutationRequest {
        schema: mutation_request_schema(),
        request_id: Some("req-append".to_string()),
        transaction_id: Some("txn-append".to_string()),
        kind: MutationKind::Update,
        target: MutationTarget {
            component_id: Some("container".to_string()),
            ..MutationTarget::default()
        },
        node: Some(
            Component::from_component_value(&serde_json::json!({
                "id": "new",
                "componentType": "text_content",
                "content": "new"
            }))
            .expect("node"),
        ),
        mode: Some("append".to_string()),
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    };

    let result = apply_mutation_value(pact, request).expect("applies append update");
    let children = result.tree["children"][0]["children"]
        .as_array()
        .expect("children");

    assert_eq!(result.report.inserted_ids, vec!["new"]);
    assert_eq!(result.report.parent_changes[0].component_id, "new");
    assert_eq!(children[0]["id"], "old");
    assert_eq!(children[0]["offset"], 0);
    assert_eq!(children[1]["id"], "new");
    assert_eq!(children[1]["offset"], 1);
}

#[test]
fn apply_delete_mutation_uses_same_report_contract() {
    let pact = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text",
            "componentType": "text_content",
            "content": "delete"
        }]
    });
    let request = MutationRequest {
        schema: mutation_request_schema(),
        request_id: None,
        transaction_id: None,
        kind: MutationKind::Delete,
        target: MutationTarget {
            component_id: Some("text".to_string()),
            ..MutationTarget::default()
        },
        node: None,
        mode: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    };

    let result = apply_mutation_value(pact, request).expect("applies delete");

    assert_eq!(result.report.removed_ids, vec!["text"]);
    assert!(!ids(&result.tree).contains("text"));
}

fn sort_tree_children(value: &mut Value) {
    let value_node_type = node_type(value);
    let value_node_type = value_node_type.as_deref();
    let is_root = value_node_type == Some("root");
    let is_sequence = is_conversation_history_region(value);
    let Some(children) = value
        .as_object_mut()
        .and_then(|object| object.get_mut("children"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if is_root {
        children.sort_by_cached_key(root_child_sort_key);
    } else if is_sequence {
        children.sort_by_cached_key(sequence_child_sort_key);
    } else {
        sort_children(children);
    }
    for child in children {
        sort_tree_children(child);
    }
}

fn delete_ids_recursive(
    value: &mut Value,
    target_ids: &BTreeSet<String>,
    deleted_ids: &mut Vec<String>,
) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) else {
        return;
    };

    let mut kept = Vec::with_capacity(children.len());
    for mut child in children.drain(..) {
        let delete_child = component_id(&child).is_some_and(|id| target_ids.contains(&id));
        if delete_child {
            collect_subtree_ids(&child, deleted_ids);
        } else {
            delete_ids_recursive(&mut child, target_ids, deleted_ids);
            kept.push(child);
        }
    }
    *children = kept;
}

fn compact_after_delete(tree: &mut Value) -> (Vec<NodeOffsetShift>, Vec<NodeDepthShift>) {
    let mut shifted_offsets = Vec::new();
    compact_child_offsets(tree, &mut shifted_offsets, true);
    let shifted_depths = compact_historical_depths(tree);
    (shifted_offsets, shifted_depths)
}

fn compact_child_offsets(
    value: &mut Value,
    shifted_offsets: &mut Vec<NodeOffsetShift>,
    is_root: bool,
) {
    let parent_id = component_id(value);
    let is_sequence = is_conversation_history_region(value);
    let skip_offset_compaction = is_root || is_sequence;
    let Some(children) = value
        .as_object_mut()
        .and_then(|object| object.get_mut("children"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    for child in children.iter_mut() {
        compact_child_offsets(child, shifted_offsets, false);
    }

    if skip_offset_compaction {
        sort_child_list(is_root, is_sequence, children);
        return;
    }

    let mut positive: Vec<(usize, i32)> = children
        .iter()
        .enumerate()
        .filter_map(|(index, child)| {
            let offset = node_offset(child)?;
            (offset >= 0).then_some((index, offset))
        })
        .collect();
    positive.sort_by_key(|(_index, offset)| *offset);
    for (next, (index, from_offset)) in positive.into_iter().enumerate() {
        let to_offset = i32::try_from(next).unwrap_or(i32::MAX);
        if from_offset != to_offset {
            set_node_offset(&mut children[index], to_offset);
            if let Some(component_id) = component_id(&children[index]) {
                shifted_offsets.push(NodeOffsetShift {
                    component_id,
                    parent_id: parent_id.clone(),
                    from_offset,
                    to_offset,
                });
            }
        }
    }

    let mut negative: Vec<(usize, i32)> = children
        .iter()
        .enumerate()
        .filter_map(|(index, child)| {
            let offset = node_offset(child)?;
            (offset < 0).then_some((index, offset))
        })
        .collect();
    negative.sort_by_key(|right| std::cmp::Reverse(right.1));
    for (next, (index, from_offset)) in negative.into_iter().enumerate() {
        let to_offset = -1 - i32::try_from(next).unwrap_or(i32::MAX);
        if from_offset != to_offset {
            set_node_offset(&mut children[index], to_offset);
            if let Some(component_id) = component_id(&children[index]) {
                shifted_offsets.push(NodeOffsetShift {
                    component_id,
                    parent_id: parent_id.clone(),
                    from_offset,
                    to_offset,
                });
            }
        }
    }

    sort_children(children);
}

fn sort_child_list(is_root: bool, is_sequence: bool, children: &mut [Value]) {
    if is_root {
        children.sort_by_cached_key(root_child_sort_key);
    } else if is_sequence {
        children.sort_by_cached_key(sequence_child_sort_key);
    } else {
        sort_children(children);
    }
}

fn compact_historical_depths(tree: &mut Value) -> Vec<NodeDepthShift> {
    let Some(history_children) = conversation_history_children_mut(tree) else {
        return Vec::new();
    };

    let mut kept = Vec::with_capacity(history_children.len());
    for child in history_children.drain(..) {
        if is_empty_historical_turn(&child) {
            continue;
        }
        kept.push(child);
    }
    kept.sort_by_key(|node| node_depth(node).unwrap_or(i32::MAX));

    let mut shifted_depths = Vec::new();
    let base_depth = kept
        .first()
        .and_then(node_depth)
        .map(|depth| if depth > 1 { depth - 1 } else { 1 })
        .unwrap_or(1);
    for (index, child) in kept.iter_mut().enumerate() {
        let to_depth = base_depth + i32::try_from(index).unwrap_or(i32::MAX);
        if let Some(from_depth) = node_depth(child)
            && from_depth != to_depth
        {
            if let Some(component_id) = component_id(child) {
                shifted_depths.push(NodeDepthShift {
                    component_id,
                    from_depth,
                    to_depth,
                });
            }
            set_subtree_depth(child, to_depth);
        }
    }
    *history_children = kept;
    shifted_depths
}

fn conversation_history_children_mut(tree: &mut Value) -> Option<&mut Vec<Value>> {
    let children = tree.as_object_mut()?.get_mut("children")?.as_array_mut()?;
    for child in children {
        if is_conversation_history_region(child) {
            return child.as_object_mut()?.get_mut("children")?.as_array_mut();
        }
    }
    None
}

fn is_empty_historical_turn(value: &Value) -> bool {
    let Some(depth) = node_depth(value) else {
        return false;
    };
    if depth <= 0 {
        return false;
    }
    let Some(children) = value.get("children").and_then(Value::as_array) else {
        return true;
    };
    children.iter().all(is_empty_container_like)
}

fn is_empty_container_like(value: &Value) -> bool {
    let node_type = value.get("componentType").and_then(Value::as_str);
    let Some(children) = value.get("children").and_then(Value::as_array) else {
        return matches!(node_type, Some("message_container"));
    };
    children.is_empty() && matches!(node_type, Some("message_container"))
}

fn set_subtree_depth(value: &mut Value, depth: i32) {
    if let Some(object) = value.as_object_mut() {
        object.insert("pact_depth".to_string(), Value::Number(depth.into()));
        object.insert("depth".to_string(), Value::Number(depth.into()));
        if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                set_subtree_depth(child, depth);
            }
        }
    }
}

fn set_node_offset(value: &mut Value, offset: i32) {
    if let Some(object) = value.as_object_mut() {
        object.insert("offset".to_string(), Value::Number(offset.into()));
    }
}

fn collect_subtree_ids(value: &Value, out: &mut Vec<String>) {
    if let Some(id) = component_id(value) {
        out.push(id);
    }
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            collect_subtree_ids(child, out);
        }
    }
}

#[test]
fn derived_turn_depths_rank_root_turns_from_the_newest_and_follow_subtrees() {
    let pact = serde_json::json!({
        "id": "root", "componentType": "root", "children": [
            {"id": "system", "componentType": "system_header", "pact_depth": -1, "offset": 0, "children": [
                {"id": "sys_text", "componentType": "text_content", "content": "s", "pact_depth": -1, "offset": 0}
            ]},
            {"id": "t1", "componentType": "message_turn", "role": "user", "pact_depth": 0, "offset": 1, "children": [
                {"id": "p1", "componentType": "text_content", "content": "a", "pact_depth": 0, "offset": 0}
            ]},
            {"id": "t2", "componentType": "message_turn", "role": "assistant", "pact_depth": 0, "offset": 2, "children": [
                {"id": "p2", "componentType": "text_content", "content": "b", "pact_depth": 0, "offset": 0}
            ]},
            {"id": "t3", "componentType": "message_turn", "role": "user", "pact_depth": 0, "offset": 3, "aux": {"deleted_at": 4}, "children": [
                {"id": "p3", "componentType": "text_content", "content": "c", "pact_depth": 0, "offset": 0}
            ]},
            {"id": "t4", "componentType": "message_turn", "role": "assistant", "pact_depth": 0, "offset": 4, "children": [
                {"id": "p4", "componentType": "text_content", "content": "d", "pact_depth": 0, "offset": 0}
            ]}
        ]
    });
    let mut tree = ComponentTree::from_value(&pact).expect("tree");
    let ranked = tree
        .edit(|root| Ok(derive_turn_depths_component(root)))
        .expect("edit");
    assert_eq!(ranked, 3);
    let depth = |id: &str| tree.component(id).expect(id).pact_depth;
    assert_eq!(depth("system"), Some(-1));
    assert_eq!(depth("sys_text"), Some(-1));
    assert_eq!(depth("t1"), Some(2));
    assert_eq!(depth("p1"), Some(2));
    assert_eq!(depth("t2"), Some(1));
    assert_eq!(depth("p2"), Some(1));
    assert_eq!(depth("t3"), Some(0), "an archived turn is not ranked");
    assert_eq!(depth("t4"), Some(0));
    assert_eq!(depth("p4"), Some(0));
}
