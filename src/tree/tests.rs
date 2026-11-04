use super::*;

fn sample_tree() -> Value {
    serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "pact_depth": 0,
            "children": [{
                "id": "text",
                "componentType": "text_content",
                "offset": 0,
                "content": "hello"
            }]
        }]
    })
}

#[test]
fn component_tree_round_trips_projection_and_indexes_relationships() {
    let tree = ComponentTree::from_value(&sample_tree()).expect("component tree");

    assert_eq!(tree.root_id(), "root");
    assert_eq!(tree.parent_id("turn"), Some("root"));
    assert_eq!(tree.parent_id("text"), Some("turn"));
    assert_eq!(tree.child_ids("root"), &["turn".to_string()]);
    assert_eq!(tree.child_ids("turn"), &["text".to_string()]);
    assert_eq!(
        tree.component("text").and_then(Component::content),
        Some("hello")
    );
    let projected = tree.project_value();
    let reparsed = ComponentTree::from_value(&projected).expect("reparsed tree");
    assert_eq!(reparsed.root_id(), "root");
    assert_eq!(
        reparsed.component("text").and_then(Component::content),
        Some("hello")
    );
}

#[test]
fn component_tree_indexes_private_lookup_buckets() {
    let tree = ComponentTree::from_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "role": "assistant",
            "tags": ["chat:assistant"],
            "children": [{
                "id": "call",
                "componentType": "tool_call",
                "tool_call_id": "tc_1",
                "tool_name": "shell",
                "parameters": {},
                "key": "slot:shell",
                "tags": ["tool:shell"],
                "ttl": 2,
                "cad": 1,
                "lifecycle": {"state": "render_requested"}
            }, {
                "id": "result",
                "componentType": "tool_result",
                "tool_call_id": "tc_1",
                "success": true,
                "result": "ok"
            }]
        }]
    }))
    .expect("component tree");

    assert_eq!(tree.ids_by_type("tool_call"), &["call".to_string()]);
    assert_eq!(tree.ids_by_role("assistant"), &["turn".to_string()]);
    assert_eq!(tree.id_by_key("slot:shell"), Some("call"));
    assert_eq!(tree.ids_by_key("slot:shell"), &["call".to_string()]);
    assert_eq!(tree.ids_by_tag("tool:shell"), &["call".to_string()]);
    assert_eq!(tree.ttl_candidate_ids(), &["call".to_string()]);
    assert_eq!(tree.cad_candidate_ids(), &["call".to_string()]);
    assert_eq!(
        tree.ids_by_lifecycle_state(LifecycleState::RenderRequested),
        &["call".to_string()]
    );
    let pair = tree.tool_pair_ids("tc_1");
    assert_eq!(pair.call_ids, vec!["call".to_string()]);
    assert_eq!(pair.result_ids, vec!["result".to_string()]);
}

#[test]
fn subtree_index_update_extends_every_private_bucket() {
    let mut tree =
        ComponentTree::from_value(&serde_json::json!({"id": "root", "componentType": "root"}))
            .expect("root tree");
    let mut call = Component::from_component_value(&serde_json::json!({
        "id": "call",
        "componentType": "tool_call",
        "tool_call_id": "tc_subtree",
        "tool_name": "shell",
        "parameters": {},
        "role": "assistant",
        "key": "slot:subtree",
        "tags": ["tool:subtree"],
        "ttl": 1,
        "cad": 1,
        "lifecycle": {"state": "render_requested"}
    }))
    .expect("call");
    call.parent_id = Some("root".to_string());
    tree.root.children.push(call);
    tree.index_subtree_at_path(Some("root".to_string()), vec![0])
        .expect("index subtree");

    assert_eq!(tree.ids_by_type("tool_call"), &["call".to_string()]);
    assert_eq!(tree.ids_by_role("assistant"), &["call".to_string()]);
    assert_eq!(tree.ids_by_key("slot:subtree"), &["call".to_string()]);
    assert_eq!(tree.ids_by_tag("tool:subtree"), &["call".to_string()]);
    assert_eq!(tree.ttl_candidate_ids(), &["call".to_string()]);
    assert_eq!(tree.cad_candidate_ids(), &["call".to_string()]);
    assert_eq!(
        tree.ids_by_lifecycle_state(LifecycleState::RenderRequested),
        &["call".to_string()]
    );
    assert_eq!(
        tree.tool_pair_ids("tc_subtree").call_ids,
        vec!["call".to_string()]
    );
}

fn two_turn_tree() -> ComponentTree {
    ComponentTree::from_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_a",
            "componentType": "message_turn",
            "pact_depth": 0,
            "children": [{
                "id": "text_a",
                "componentType": "text_content",
                "offset": 0,
                "content": "alpha"
            }]
        }, {
            "id": "turn_b",
            "componentType": "message_turn",
            "pact_depth": 0,
            "children": [{
                "id": "text_b",
                "componentType": "text_content",
                "offset": 0,
                "content": "beta"
            }]
        }]
    }))
    .expect("two-turn tree")
}

#[test]
fn edit_closure_error_leaves_live_tree_and_index_unchanged() {
    let mut tree = two_turn_tree();
    let before_tree = tree.clone();
    let before_index = tree.index.clone();

    let result: PactResult<()> = tree.edit(|root| {
        root.children.clear();
        root.children.push(
            Component::from_component_value(&serde_json::json!({
                "id": "ghost",
                "componentType": "text_content",
                "offset": 0,
                "content": "should never commit"
            }))
            .expect("ghost component"),
        );
        Err(PactError::invalid("deliberate edit failure"))
    });

    assert!(result.is_err(), "edit must surface the closure error");

    assert_eq!(tree.root, before_tree.root, "live root must be unchanged");
    assert_eq!(tree.index, before_index, "live index must be unchanged");

    assert!(tree.contains("turn_a"));
    assert!(tree.contains("turn_b"));
    assert!(!tree.contains("ghost"));

    assert_eq!(
        tree.index,
        ComponentTreeIndex::try_derive(&tree.root).expect("derive")
    );
}

#[test]
fn edit_introducing_duplicate_id_returns_err_and_does_not_mutate() {
    let mut tree = two_turn_tree();
    let before_tree = tree.clone();
    let before_index = tree.index.clone();

    let result: PactResult<()> = tree.edit(|root| {
        let dup = Component::from_component_value(&serde_json::json!({
            "id": "text_a",
            "componentType": "text_content",
            "offset": 1,
            "content": "duplicate"
        }))
        .expect("duplicate component");
        root.children[0].children.push(dup);
        Ok(())
    });

    let err = result.expect_err("duplicate id must be rejected");
    assert!(
        format!("{err}").contains("duplicate component id"),
        "error should report the duplicate id, got: {err}"
    );
    assert_eq!(tree.root, before_tree.root, "live root must be unchanged");
    assert_eq!(tree.index, before_index, "live index must be unchanged");
    assert_eq!(
        tree.index,
        ComponentTreeIndex::try_derive(&tree.root).expect("derive")
    );
}

#[test]
fn successful_edit_commits_root_and_index_together() {
    let mut tree = two_turn_tree();

    tree.edit(|root| {
        let new_text = Component::from_component_value(&serde_json::json!({
            "id": "text_c",
            "componentType": "text_content",
            "offset": 1,
            "content": "gamma"
        }))
        .expect("new text");
        root.children[0].children.push(new_text);
        Ok(())
    })
    .expect("edit succeeds");

    assert!(tree.contains("text_c"), "new component is indexed");
    assert_eq!(tree.parent_id("text_c"), Some("turn_a"));
    assert_eq!(
        tree.index,
        ComponentTreeIndex::try_derive(&tree.root).expect("derive"),
        "committed index must equal a from-scratch re-derive"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "incoherent")]
fn debug_assert_index_coherent_fires_when_index_skips_maintenance() {
    let mut tree = two_turn_tree();

    tree.root.children.push(
        Component::from_component_value(&serde_json::json!({
            "id": "orphan",
            "componentType": "text_content",
            "offset": 0,
            "content": "drift"
        }))
        .expect("orphan component"),
    );

    tree.debug_assert_index_coherent();
}
