use super::{
    ComponentRecord, ContentStore, ContextTreeLedger, PactBranch, PactCommit, PactRef, PactStore,
    PactStoreStats, PactTransactionTrace, PactView, ProjectionTreeScope, ROOT_STORE_SCHEMA,
    TRANSACTION_TRACE_SCHEMA, checkpoint_store_json, decode_context_tree_ledger, fork_store_json,
    ledger_create_branch_at_node_json, ledger_create_branch_at_revision_json,
    ledger_create_branch_at_transaction_json, ledger_from_store_json, ledger_inspect_branch_json,
    ledger_inspect_json, ledger_project_branch_json, ledger_project_branch_json_with_scope,
    ledger_project_branch_tree_json, ledger_project_branch_tree_json_with_scope,
    ledger_record_transaction_trace_json, new_ledger_json, new_ledger_json_without_base,
    new_store_json, project_branch_archive_component_tree, project_branch_component_tree,
    project_store_json, transaction_trace_build_json,
};
use crate::components::{
    Component, ComponentInternalRefs, ComponentLifecycle, ComponentLifecyclePolicy, InternalRef,
    LifecycleState,
};
use crate::file_format::PactFileBuilder;
use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
use crate::resolver::PactResolver;
use crate::selector::select_json;
use crate::trace::BoundaryKind;
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn sample_tree() -> &'static str {
    r#"{
          "id": "root",
          "componentType": "root",
          "children": [
            {
              "id": "active",
              "componentType": "message_turn",
              "pact_depth": 0,
              "role": "user",
              "children": [
                {"id":"text","componentType":"text_content","content":"hello"}
              ]
            }
          ]
        }"#
}

fn find_node<'a>(node: &'a Value, component_id: &str) -> Option<&'a Value> {
    if node.get("id").and_then(Value::as_str) == Some(component_id) {
        return Some(node);
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = find_node(child, component_id) {
            return Some(found);
        }
    }
    None
}

fn node_exists(node: &Value, component_id: &str) -> bool {
    find_node(node, component_id).is_some()
}

fn transaction_trace(root_store: &ContextTreeLedger, transaction_id: &str) -> PactTransactionTrace {
    PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: root_store.root_id.clone(),
        branch_id: "main".to_string(),
        transaction_id: transaction_id.to_string(),
        authored_ref: PactRef {
            root_id: root_store.root_id.clone(),
            branch_id: "main".to_string(),
            revision: Some(0),
            transaction_id: Some(transaction_id.to_string()),
            component_id: None,
            content_ref: None,
        },
        pre_request_ref: PactRef {
            root_id: root_store.root_id.clone(),
            branch_id: "main".to_string(),
            revision: Some(0),
            transaction_id: Some(transaction_id.to_string()),
            component_id: None,
            content_ref: None,
        },
        post_request_alteration_ref: PactRef {
            root_id: root_store.root_id.clone(),
            branch_id: "main".to_string(),
            revision: Some(0),
            transaction_id: Some(transaction_id.to_string()),
            component_id: None,
            content_ref: None,
        },
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: Some(PactRef {
            root_id: root_store.root_id.clone(),
            branch_id: "main".to_string(),
            revision: Some(1),
            transaction_id: Some(transaction_id.to_string()),
            component_id: None,
            content_ref: None,
        }),
        alteration_ids: Vec::new(),
        commit: None,
        replay_trace: Some(crate::transaction_trace::TransactionTrace {
            from_rev: 0,
            to_rev: 1,
            ops: Vec::new(),
        }),
        aux: BTreeMap::new(),
    }
}

fn test_envelope(branch_id: &str) -> PactOperationEnvelope {
    PactOperationEnvelope {
        branch_id: branch_id.to_string(),
        source: SourceActor::Host,
        source_id: Some("store_test".to_string()),
        host_timestamp_ns: None,
        idempotency_key: None,
        aux: BTreeMap::new(),
    }
}

fn seal_ops(
    ledger: ContextTreeLedger,
    branch_id: &str,
    ops: Vec<PactOperation>,
) -> ContextTreeLedger {
    let mut resolver = PactResolver::from_ledger(ledger);
    let outcome = resolver
        .resolve(ops, branch_id, BoundaryKind::Authored)
        .expect("resolve native ops");
    let candidate = outcome.commit_candidate.expect("commit candidate");
    resolver.seal(&candidate).expect("seal native ops");
    resolver.into_ledger()
}

fn append_component_json(
    root_json: &str,
    branch_id: &str,
    parent_id: &str,
    component: Value,
) -> String {
    let ledger = decode_context_tree_ledger(root_json).expect("decode ledger");
    let component = Component::from_component_value(&component).expect("component");
    let op = PactOperation::Append {
        envelope: test_envelope(branch_id),
        target_component_id: Some(parent_id.to_string()),
        target_selector: None,
        offset: None,
        component,
    };
    let mutated = seal_ops(ledger, branch_id, vec![op]);
    serde_json::to_string(&mutated).expect("encode mutated ledger")
}

#[test]
fn cyclic_fork_lineage_errors_instead_of_stack_overflow() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [{"id":"text","componentType":"text_content","content":"seed"}]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let mut ledger: ContextTreeLedger = serde_json::from_str(&root_json).expect("ledger");

    let template = ledger.branches.get("main").expect("main").clone();
    let root_id = ledger.root_id.clone();
    let make_cyclic = |id: &str, other: &str| {
        let mut b = template.clone();
        b.branch_id = id.to_string();
        b.base_ref.branch_id = id.to_string();
        b.base_ref.content_ref = None;
        b.fork_ref = Some(PactRef {
            root_id: root_id.clone(),
            branch_id: other.to_string(),
            revision: Some(1),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        });
        b.head_revision = 1;
        b.transaction_traces = vec![crate::transaction_trace::TransactionTrace {
            from_rev: 0,
            to_rev: 1,
            ops: Vec::new(),
        }];
        b.current.tree.root.children.clear();
        b.current.tree.index = Default::default();
        b.current.tree.first_seen = BTreeMap::new();
        b.current.tree.tool_pairs = BTreeMap::new();
        b.current.tree.stats = PactStoreStats {
            node_count: 0,
            edge_count: 0,
            blob_count: 0,
            content_chars: 0,
        };
        b
    };
    let a = make_cyclic("cycle_a", "cycle_b");
    let bb = make_cyclic("cycle_b", "cycle_a");
    ledger.branches.insert("cycle_a".to_string(), a);
    ledger.branches.insert("cycle_b".to_string(), bb);

    let branch = ledger.branches.get("cycle_a").expect("cycle_a");
    let err = project_branch_component_tree(&ledger, branch, None)
        .expect_err("cyclic lineage must error");
    assert!(
        err.to_string().contains("cyclic fork lineage"),
        "got: {err}"
    );
}

#[test]
fn projection_uses_current_head_base_when_early_traces_are_pruned() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [{"id":"seeded","componentType":"text_content","content":"from base"}]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let mut ledger: ContextTreeLedger = serde_json::from_str(&root_json).expect("ledger");

    let branch = ledger.branches.get_mut("main").expect("main");
    let mut grown = branch.current.tree.root.clone();
    let active = grown
        .children
        .iter_mut()
        .find(|c| c.id == "active")
        .expect("active");
    let mut extra = active.children[0].clone();
    extra.id = "grown_at_rev_2".to_string();
    active.children.push(extra);
    branch.current.tree.root = grown;
    branch.current.head_revision = 2;
    branch.head_revision = 3;
    branch.transaction_traces = vec![crate::transaction_trace::TransactionTrace {
        from_rev: 2,
        to_rev: 3,
        ops: Vec::new(),
    }];

    let branch = ledger.branches.get("main").expect("main");
    let projected = project_branch_component_tree(&ledger, branch, Some(3)).expect("project");
    assert!(
        projected.contains("grown_at_rev_2"),
        "projection must start from the current-head snapshot, not the rev-0 base"
    );
}

#[test]
fn store_projects_base() {
    let store_json = new_store_json(sample_tree(), 8).expect("store");
    let projected = project_store_json(&store_json, None).expect("project");
    let tree: Value = serde_json::from_str(&projected).expect("tree");
    assert_eq!(tree["id"], "root");
}

#[test]
fn tag_selectors_match_seeded_canonical_fields() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [{
                "id": "text",
                "componentType": "text_content",
                "content": "hello",
                "tags": ["archived", "pager"],
                "ttl": 5,
                "key": "memory-slot"
            }]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let tree_json = project_store_json(&store_json, None).expect("project");
    let projected: Value = serde_json::from_str(&tree_json).expect("tree json");
    let text = &projected["children"][0]["children"][0];
    assert_eq!(text["tags"], json!(["archived", "pager"]));
    assert_eq!(text["ttl"], json!(5));
    assert_eq!(text["key"], json!("memory-slot"));

    let selected = select_json(&tree_json, "+archived").expect("tag selector");
    let matches: Vec<Value> = serde_json::from_str(&selected).expect("selector json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["id"], "text");
}

#[test]
fn component_record_preserves_lifecycle_state_and_internal_refs() {
    let mut component = Component::from_component_value(&json!({
        "id": "tracked",
        "componentType": "text_content",
        "content": "watch me"
    }))
    .expect("component");
    let mut state_aux = BTreeMap::new();
    state_aux.insert("scope".to_string(), json!("render"));
    component.lifecycle = ComponentLifecycle {
        state: LifecycleState::RenderRequested,
        revision: 42,
        policy: ComponentLifecyclePolicy {
            ttl: Some(3),
            cad: Some(2),
        },
        internal_refs: ComponentInternalRefs {
            state: Some(InternalRef {
                id: "state_ref".to_string(),
                kind: "component_state".to_string(),
                field: Some("state".to_string()),
                aux: state_aux,
            }),
            projection: Some(InternalRef {
                id: "projection_ref".to_string(),
                kind: "component_projection".to_string(),
                field: None,
                aux: BTreeMap::new(),
            }),
        },
    };

    let record = ComponentRecord::from_component_with_parent(&component, Some("root".to_string()));
    assert_eq!(record.ttl, Some(3));
    assert_eq!(record.cad, Some(2));
    assert_eq!(record.lifecycle.state, LifecycleState::RenderRequested);
    assert_eq!(record.lifecycle.revision, 42);
    assert!(record.lifecycle.policy.is_empty());
    assert_eq!(
        record
            .lifecycle
            .internal_refs
            .state
            .as_ref()
            .map(|value| &value.id),
        Some(&"state_ref".to_string())
    );
    assert_eq!(
        record
            .lifecycle
            .internal_refs
            .projection
            .as_ref()
            .map(|value| &value.id),
        Some(&"projection_ref".to_string())
    );

    let encoded = serde_json::to_value(&record).expect("record json");
    assert_eq!(encoded["ttl"], json!(3));
    assert_eq!(encoded["cad"], json!(2));
    assert_eq!(encoded["lifecycle"]["state"], json!("render_requested"));
    assert_eq!(encoded["lifecycle"]["revision"], json!(42));
    assert!(encoded["lifecycle"].get("policy").is_none());
    assert_eq!(
        encoded["lifecycle"]["internal_refs"]["state"]["id"],
        json!("state_ref")
    );
}

#[test]
fn root_store_projection_hydrates_content_ref() {
    let mut content = ContentStore::default();
    let record = content.put_inline("node.content", "fresh canonical text", 32);
    let content_id = record.id.clone();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text",
            "componentType": "text_content",
            "parent_id": "root",
            "offset": 0,
            "content_ref": {
                "id": content_id,
                "field": "content"
            }
        }]
    });
    let projection = super::build_projection(&tree, &BTreeMap::new(), 32).expect("projection");
    let view = PactView::from_projection(projection, "main".to_string(), 0);
    let root_store = ContextTreeLedger {
        history: Default::default(),
        schema: ROOT_STORE_SCHEMA.to_string(),
        root_id: "root".to_string(),
        parent: None,
        base: None,
        branches: BTreeMap::from([(
            "main".to_string(),
            PactBranch {
                branch_id: "main".to_string(),
                root_id: "root".to_string(),
                parent_branch_id: None,
                fork_ref: None,
                base_ref: PactRef {
                    root_id: "root".to_string(),
                    branch_id: "main".to_string(),
                    revision: Some(0),
                    transaction_id: None,
                    component_id: None,
                    content_ref: None,
                },
                head_revision: 0,
                lifecycle_tick: 0,
                transaction_traces: Vec::new(),
                current: view,
                aux: BTreeMap::new(),
            },
        )]),
        content,
        event_index: super::BranchEventIndex::default(),
        current_branch_id: "main".to_string(),
    };
    let root_json = serde_json::to_string(&root_store).expect("root json");
    let projected = ledger_project_branch_tree_json(&root_json, "main", None).expect("project");
    let tree: Value = serde_json::from_str(&projected).expect("tree json");

    assert_eq!(tree["children"][0]["content"], "fresh canonical text");
}

#[test]
fn store_indexes_tool_pairs_from_attrs() {
    let base = r#"{
          "id":"root",
          "componentType":"root",
          "children":[{
            "id":"turn",
            "componentType":"message_turn",
            "children":[{
              "id":"container",
              "componentType":"message_container",
              "children":[
                {
                  "id":"call",
                  "componentType":"tool_call",
                  "attrs":{
                    "tool_call_id":"toolu_attrs",
                    "tool_name":"shell_execute",
                    "parameters":{"command":"pwd"}
                  }
                },
                {
                  "id":"result",
                  "componentType":"tool_result",
                  "attrs":{
                    "tool_call_id":"toolu_attrs",
                    "tool_name":"shell_execute",
                    "success":true
                  }
                }
              ]
            }]
          }]
        }"#;
    let store_json = new_store_json(base, 8).expect("store");
    let store: PactStore = serde_json::from_str(&store_json).expect("store json");
    let pair = &store.current.tree.tool_pairs["toolu_attrs"];
    assert_eq!(pair.call_ids, vec!["call".to_string()]);
    assert_eq!(pair.result_ids, vec!["result".to_string()]);
    assert_eq!(
        store.current.component_records()["call"]
            .tool_name
            .as_deref(),
        Some("shell_execute")
    );
    assert_eq!(
        store.current.component_records()["call"]
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.get("command"))
            .and_then(Value::as_str),
        Some("pwd")
    );
}

#[test]
fn store_fork_projects_revision_as_new_base() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [{"id":"text","componentType":"text_content","content":"changed"}]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let forked_json = fork_store_json(&store_json, Some(0), 8).expect("fork");
    let forked: PactStore = serde_json::from_str(&forked_json).expect("forked store");
    assert_eq!(forked.head_revision, 0);
    let projected = project_store_json(&forked_json, None).expect("project");
    assert!(projected.contains("changed"));
}

#[test]
fn repeated_large_content_reuses_existing_blob_ref() {
    let base = r#"{
          "id":"root",
          "componentType":"root",
          "children":[
            {"id":"a","componentType":"text_content","content":"same large content"},
            {"id":"b","componentType":"text_content","content":"same large content"}
          ]
        }"#;
    let store_json = new_store_json(base, 4).expect("store");
    let store: PactStore = serde_json::from_str(&store_json).expect("store json");

    assert_eq!(store.current.blobs.len(), 1);
    assert_eq!(
        store.current.component_records()["a"].content_ref,
        store.current.component_records()["b"].content_ref
    );
    assert!(
        store.current.blobs.contains_key(
            store.current.component_records()["b"]
                .content_ref
                .as_deref()
                .unwrap()
        )
    );
}

#[test]
fn content_store_reuses_exact_content_refs() {
    let mut store = ContentStore::default();
    let first = store.put_inline("provider.request.rows", "same exact payload", 8);
    let second = store.put_inline("provider.request.rows", "same exact payload", 8);
    let third = store.put_inline("provider.response.rows", "same exact payload", 8);

    assert_eq!(first.id, second.id);
    assert_ne!(first.id, third.id);
    assert_eq!(store.records.len(), 2);
    assert_eq!(first.summary().preview, "same exa");
}

#[test]
fn checkpoint_first_seen_tracks_base_components() {
    let store_json = new_store_json(sample_tree(), 8).expect("store");
    let checkpoint_json = checkpoint_store_json(&store_json, Some(0), 8).expect("checkpoint");
    let checkpoint: Value = serde_json::from_str(&checkpoint_json).expect("checkpoint json");

    let first_seen = &checkpoint["projection"]["tree"]["first_seen"];
    assert_eq!(first_seen["text"], json!(0));
    assert!(first_seen.get("missing").is_none());
}

#[test]
fn decode_store_rejects_removed_patch_history_field() {
    let store_json = new_store_json(sample_tree(), 8).expect("store");
    let mut store_value: Value = serde_json::from_str(&store_json).expect("store json");

    store_value[concat!("patch", "es")] = json!([{ "rev": 1 }]);
    let store_json = serde_json::to_string(&store_value).expect("store json string");

    let err = super::branch_projection::decode_store(&store_json)
        .expect_err("removed field must be rejected");
    assert!(
        err.to_string()
            .contains("removed patch-shaped mutation history"),
        "unexpected error: {err}"
    );
}

#[test]
fn decode_context_tree_ledger_rejects_removed_patch_history_field_on_branch() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let mut root_value: Value = serde_json::from_str(&root_json).expect("root json");
    root_value["branches"]["main"][concat!("patch", "es")] = json!([{ "rev": 1 }]);
    let root_json = serde_json::to_string(&root_value).expect("root json string");

    let err =
        decode_context_tree_ledger(&root_json).expect_err("removed branch field must be rejected");
    assert!(
        err.to_string()
            .contains("removed patch-shaped mutation history"),
        "unexpected error: {err}"
    );
}

#[test]
fn decode_context_tree_ledger_accepts_real_transaction_trace_history() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let updated_json = append_component_json(
        &root_json,
        "main",
        "active",
        json!({
            "id": "later",
            "componentType": "text_content",
            "type": "text_content",
            "content": "world"
        }),
    );

    let ledger = decode_context_tree_ledger(&updated_json).expect("decode trace ledger");
    let branch = ledger.branches.get("main").expect("main branch");
    assert!(
        !branch.transaction_traces.is_empty(),
        "native append must leave replayable transaction trace history"
    );
}

#[test]
fn root_store_wraps_single_branch_with_view_indexes() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");
    let branch = root_store.branches.get("main").expect("main branch");

    assert_eq!(root_store.schema, ROOT_STORE_SCHEMA);
    assert_eq!(root_store.current_branch_id, "main");
    assert_eq!(branch.root_id, "root");
    assert_eq!(branch.base_ref.revision, Some(0));
    assert!(branch.base_ref.content_ref.is_some());
    assert_eq!(root_store.content.records.len(), 1);
    assert_eq!(branch.current.branch_id, "main");
    assert_eq!(branch.current.message_order, vec!["active".to_string()]);
}

#[test]
fn native_ledger_prune_routes_through_resolver_mutation() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");

    let (root_store, report) = root_store
        .prune("main", Some("text".to_string()), None)
        .expect("native prune");

    assert_eq!(report["schema"], "pact.delete.v1");
    assert!(
        report["deleted_component_ids"]
            .as_array()
            .expect("deleted ids")
            .iter()
            .any(|id| id == "text")
    );
    let projected =
        project_branch_component_tree(&root_store, root_store.branches.get("main").unwrap(), None)
            .expect("project");
    assert!(!projected.contains("text"));
}

#[test]
fn native_ledger_project_and_inspect_methods_match_wrappers() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");

    let view = root_store
        .project_branch("main", None, 8)
        .expect("native project");
    assert_eq!(view.branch_id, "main");
    assert!(view.component_records().contains_key("text"));

    let scoped = root_store
        .project_branch_with_scope("main", None, 8, ProjectionTreeScope::Head)
        .expect("native scoped project");
    assert_eq!(scoped.head_revision, view.head_revision);

    let inspection = root_store.inspect();
    assert_eq!(inspection["branch_count"], 1);
    assert_eq!(inspection["branches"]["main"]["node_count"], 3);

    let branch = root_store.inspect_branch("main").expect("branch inspect");
    assert_eq!(branch["branch_id"], "main");
    assert_eq!(branch["message_count"], 1);
}

#[test]
fn root_store_without_base_projects_from_current_view() {
    let root_json =
        new_ledger_json_without_base(sample_tree(), Some("main"), 8).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");
    let branch = root_store.branches.get("main").expect("main branch");

    assert_eq!(branch.base_ref.revision, Some(0));
    assert!(branch.base_ref.content_ref.is_none());
    assert!(
        root_store
            .content
            .records
            .values()
            .all(|record| record.kind != "pact.branch.base")
    );

    let projected = ledger_project_branch_tree_json(&root_json, "main", Some(0)).expect("project");
    let tree: Value = serde_json::from_str(&projected).expect("tree");
    assert_eq!(tree["id"], "root");
    assert_eq!(tree["children"][0]["id"], "active");
    assert_eq!(tree["children"][0]["children"][0]["content"], "hello");
}

#[test]
fn first_commit_captures_exact_birth_tree_for_no_base_branch() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "system_original",
                "children": []
            },
            {
                "id": "active",
                "componentType": "message_turn",
                "pact_depth": 0,
                "role": "user",
                "children": []
            }
        ]
    });
    let root_json =
        new_ledger_json_without_base(&tree.to_string(), Some("main"), 8).expect("root store");
    let ledger = decode_context_tree_ledger(&root_json).expect("decode ledger");

    let first = PactOperation::Append {
        envelope: test_envelope("main"),
        target_component_id: Some("active".to_string()),
        target_selector: None,
        offset: None,
        component: Component::from_component_value(&json!({
            "id": "first_text",
            "componentType": "text_content",
            "content": "first"
        }))
        .expect("first component"),
    };
    let ledger = seal_ops(ledger, "main", vec![first]);
    let branch = ledger.branches.get("main").expect("main branch");
    let content_ref = branch
        .base_ref
        .content_ref
        .as_deref()
        .expect("first commit should capture the exact birth tree");
    assert_eq!(branch.base_ref.revision, Some(0));
    let base_json = ledger
        .content
        .records
        .get(content_ref)
        .and_then(|record| record.storage.inline.as_deref())
        .expect("inline branch base content");
    let base: Value = serde_json::from_str(base_json).expect("branch base json");
    assert_eq!(base["children"][0]["role"], "system_original");

    let second = PactOperation::Update {
        envelope: test_envelope("main"),
        target_component_id: Some("system".to_string()),
        target_selector: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::from([("role".to_string(), json!("system_mutated"))]),
        aux: BTreeMap::new(),
    };
    let ledger = seal_ops(ledger, "main", vec![second]);
    let root_json = serde_json::to_string(&ledger).expect("encode ledger");
    let rev1_json =
        ledger_project_branch_tree_json(&root_json, "main", Some(1)).expect("project rev1");
    let rev1: Value = serde_json::from_str(&rev1_json).expect("rev1 tree");
    assert_eq!(
        rev1["children"][0]["role"], "system_original",
        "revision 1 must replay from recorded birth, not a derived skeleton"
    );
    let head_json =
        ledger_project_branch_tree_json(&root_json, "main", None).expect("project head");
    let head: Value = serde_json::from_str(&head_json).expect("head tree");
    assert_eq!(head["children"][0]["role"], "system_mutated");
}

#[test]
fn first_commit_on_reference_fork_does_not_capture_birth_content() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");
    let before_content_count = root_store.content.records.len();
    let (forked, branch_id) = root_store
        .fork_reference_at(
            "main",
            None,
            Some("overlay_branch".to_string()),
            BTreeMap::new(),
        )
        .expect("reference fork");

    let append = PactOperation::Append {
        envelope: test_envelope(&branch_id),
        target_component_id: Some("active".to_string()),
        target_selector: None,
        offset: None,
        component: Component::from_component_value(&json!({
            "id": "overlay_local",
            "componentType": "text_content",
            "content": "local only"
        }))
        .expect("overlay component"),
    };
    let forked = seal_ops(forked, &branch_id, vec![append]);
    let branch = forked.branches.get(&branch_id).expect("overlay branch");

    assert!(branch.fork_ref.is_some());
    assert!(
        branch.base_ref.content_ref.is_none(),
        "reference forks must keep using the fork_ref compose arm after their first local commit"
    );
    assert_eq!(
        forked.content.records.len(),
        before_content_count,
        "overlay first commit must not add a branch-base content record"
    );
    let projected = project_branch_component_tree(&forked, branch, None).expect("project overlay");
    assert!(projected.contains("text"), "parent content remains visible");
    assert!(
        projected.contains("overlay_local"),
        "local content remains visible"
    );
}

#[test]
fn first_commit_birth_base_replays_init_node_moved_under_trace_parent() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "message_turn",
                "pact_depth": 0,
                "role": "user",
                "children": [
                    {
                        "id": "seed_note",
                        "componentType": "text_content",
                        "content": "born before traces"
                    }
                ]
            }
        ]
    });
    let root_json =
        new_ledger_json_without_base(&tree.to_string(), Some("main"), 8).expect("root store");
    let ledger = decode_context_tree_ledger(&root_json).expect("decode ledger");

    let first = PactOperation::Append {
        envelope: test_envelope("main"),
        target_component_id: Some("active".to_string()),
        target_selector: None,
        offset: None,
        component: Component::from_component_value(&json!({
            "id": "trace_parent",
            "componentType": "message_turn",
            "role": "assistant",
            "children": []
        }))
        .expect("trace-created parent"),
    };
    let mut ledger = seal_ops(ledger, "main", vec![first]);
    {
        let branch = ledger.branches.get_mut("main").expect("main branch");
        assert!(
            branch.base_ref.content_ref.is_some(),
            "first commit should capture the pre-trace birth tree"
        );
        branch.transaction_traces.push(TransactionTrace {
            from_rev: 1,
            to_rev: 2,
            ops: vec![TraceOp::new(
                "seed_note",
                TraceOpKind::MoveComponent {
                    parent_id: "trace_parent".to_string(),
                    index: 0,
                },
            )],
        });
        branch.head_revision = 2;
    }
    let branch = ledger.branches.get("main").expect("main branch");
    let rev1 = project_branch_component_tree(&ledger, branch, Some(1))
        .expect("project rev1")
        .project_value();
    let active = find_node(&rev1, "active").expect("active node at rev1");
    assert!(
        find_node(active, "seed_note").is_some(),
        "revision 1 keeps the birth-time component in its original parent"
    );
    assert!(
        find_node(active, "trace_parent").is_some(),
        "revision 1 contains the trace-created move target"
    );

    let rev2 = project_branch_component_tree(&ledger, branch, Some(2))
        .expect("project rev2")
        .project_value();
    let parent = find_node(&rev2, "trace_parent").expect("trace parent at rev2");
    assert!(
        find_node(parent, "seed_note").is_some(),
        "revision 2 must replay the birth-time component move from the recorded birth tree"
    );
}

#[test]
fn first_commit_birth_base_survives_file_round_trip() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "file_birth",
                "children": []
            },
            {
                "id": "active",
                "componentType": "message_turn",
                "pact_depth": 0,
                "role": "user",
                "children": []
            }
        ]
    });
    let root_json =
        new_ledger_json_without_base(&tree.to_string(), Some("main"), 8).expect("root store");
    let ledger = decode_context_tree_ledger(&root_json).expect("decode ledger");
    let first = PactOperation::Append {
        envelope: test_envelope("main"),
        target_component_id: Some("active".to_string()),
        target_selector: None,
        offset: None,
        component: Component::from_component_value(&json!({
            "id": "first_text",
            "componentType": "text_content",
            "content": "first"
        }))
        .expect("first component"),
    };
    let ledger = seal_ops(ledger, "main", vec![first]);
    let bytes = PactFileBuilder::from_ledger(&ledger).expect("write pact file");
    let temp = tempfile::NamedTempFile::new().expect("temporary Pact file");
    std::fs::write(temp.path(), bytes).expect("write Pact bytes");
    let reopened = crate::pact_file::open_ledger(temp.path()).expect("read pact file");
    let branch = reopened.branches.get("main").expect("main branch");
    assert!(
        branch.base_ref.content_ref.is_some(),
        "new files must persist the captured branch birth base without a version bump"
    );
    let rev0 = project_branch_component_tree(&reopened, branch, Some(0))
        .expect("project reopened rev0")
        .project_value();
    assert_eq!(rev0["children"][0]["role"], "file_birth");
}

#[test]
fn root_store_append_reexternalizes_current_head_content_refs() {
    let long_body = "abcdefghijklmnopqrstuvwxyz".repeat(4);
    let root = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "role": "user",
            "children": [{
                "id": "large_text",
                "componentType": "text_content",
                "content": long_body
            }]
        }]
    });
    let root_json =
        new_ledger_json_without_base(&root.to_string(), Some("main"), 8).expect("root store");

    let updated = append_component_json(
        &root_json,
        "main",
        "root",
        json!({
            "id": "next",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "content": "hi"
        }),
    );
    let root_store: ContextTreeLedger = serde_json::from_str(&updated).expect("root json");
    let branch = root_store.branches.get("main").expect("main branch");
    let records = branch.current.component_records();
    let node = records.get("large_text").expect("large text node");
    let content_ref = node.content_ref.as_deref().expect("content ref");

    assert!(
        content_ref.starts_with("cnt_"),
        "current head must store durable content refs, got {content_ref}"
    );
    assert!(!content_ref.starts_with("blob_"));
    assert!(root_store.content.records.contains_key(content_ref));

    let projected = ledger_project_branch_tree_json(&updated, "main", None).expect("project");
    assert!(projected.contains(&long_body));
}

#[test]
fn root_store_without_base_rejects_non_head_projection() {
    let root_json =
        new_ledger_json_without_base(sample_tree(), Some("main"), 8).expect("root store");
    let err = ledger_project_branch_tree_json(&root_json, "main", Some(1))
        .expect_err("no-base branches cannot replay non-head revisions");

    assert!(err.to_string().contains("has no base content ref"));
}

#[test]
fn root_store_import_externalizes_large_tool_payload_slots() {
    let large_arg = "parameter value ".repeat(16);
    let large_result = "result value ".repeat(16);
    let large_error = "error value ".repeat(16);
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "call",
                "componentType": "tool_call",
                "type": "tool_call",
                "tool_call_id": "tc_1",
                "tool_name": "shell",
                "parameters": {"command": large_arg}
            },
            {
                "id": "result",
                "componentType": "tool_result",
                "type": "tool_result",
                "tool_call_id": "tc_1",
                "tool_name": "shell",
                "success": true,
                "result": {"stdout": large_result}
            },
            {
                "id": "error",
                "componentType": "tool_result",
                "type": "tool_result",
                "tool_call_id": "tc_2",
                "tool_name": "shell",
                "success": false,
                "error_message": large_error
            }
        ]
    });
    let root_json =
        new_ledger_json_without_base(&tree.to_string(), Some("main"), 32).expect("root store");
    let store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");
    let nodes = &store.branches["main"].current.component_records();

    assert!(nodes["call"].parameters.is_none());
    assert!(nodes["call"].parameters_ref.is_some());
    assert!(nodes["result"].result.is_none());
    assert!(nodes["result"].result_ref.is_some());
    assert!(nodes["error"].error_message.is_none());
    assert!(nodes["error"].error_ref.is_some());

    let projected = ledger_project_branch_tree_json(&root_json, "main", None).expect("project");
    let projected: Value = serde_json::from_str(&projected).expect("projected json");
    let call = find_node(&projected, "call").expect("call node");
    let result = find_node(&projected, "result").expect("result node");
    let error = find_node(&projected, "error").expect("error node");
    assert_eq!(call["parameters"]["command"], large_arg);
    assert_eq!(result["result"]["stdout"], large_result);
    assert_eq!(error["error_message"], large_error);
}

#[test]
fn root_store_import_preserves_arbitrary_json_tool_parameters() {
    let parameters = json!({
        "cmd": "printf '%s\\n' \"$PATH\"",
        "nested": [{"a": true}, {"b": [1, 2, 3]}, {"quote": "\"'\\n"}],
        "nullish": null
    });
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "call",
            "componentType": "tool_call",
            "type": "tool_call",
            "tool_call_id": "tc_json",
            "tool_name": "shell",
            "parameters": parameters
        }]
    });
    let root_json =
        new_ledger_json_without_base(&tree.to_string(), Some("main"), 16).expect("root store");
    let projected = ledger_project_branch_tree_json(&root_json, "main", None).expect("project");
    let projected: Value = serde_json::from_str(&projected).expect("projected json");

    assert_eq!(projected["children"][0]["parameters"], parameters);
}

#[test]
fn root_store_can_upgrade_single_branch_store_shape() {
    let store_json = new_store_json(sample_tree(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");

    assert!(root_store.branches.contains_key("main"));
    assert_eq!(
        root_store.branches["main"].current.component_records()["text"]
            .content
            .as_deref(),
        Some("hello")
    );
}

#[test]
fn root_store_forks_at_revision_with_lineage() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [{"id":"text","componentType":"text_content","content":"changed"}]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let before: ContextTreeLedger = serde_json::from_str(&root_json).expect("root before fork");
    let before_content_count = before.content.records.len();
    let forked_json = ledger_create_branch_at_revision_json(
        &root_json,
        "main",
        Some(0),
        Some("branch_b"),
        Some(r#"{"branch_kind":"derived","view_kind":"review"}"#),
        8,
    )
    .expect("fork root store");
    let forked: ContextTreeLedger = serde_json::from_str(&forked_json).expect("forked root");
    let derived_branch = forked.branches.get("branch_b").expect("derived branch");

    assert_eq!(forked.current_branch_id, "branch_b");
    assert_eq!(derived_branch.parent_branch_id.as_deref(), Some("main"));
    assert_eq!(
        derived_branch.fork_ref.as_ref().and_then(|r| r.revision),
        Some(0)
    );
    assert_eq!(derived_branch.head_revision, 0);
    assert_eq!(
        derived_branch.current.component_records()["text"]
            .content
            .as_deref(),
        Some("changed")
    );
    assert_eq!(derived_branch.aux["view_kind"], "review");
    assert!(derived_branch.base_ref.content_ref.is_none());
    assert_eq!(forked.content.records.len(), before_content_count);
    let projected =
        ledger_project_branch_tree_json(&forked_json, "branch_b", None).expect("project soft fork");
    assert!(projected.contains("changed"));
}

#[test]
fn root_store_forks_at_node_first_seen_revision() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [
                {"id":"text","componentType":"text_content","content":"hello"},
                {"id":"later","componentType":"text_content","offset":1,"content":"world"}
            ]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let forked_json =
        ledger_create_branch_at_node_json(&root_json, "main", "later", Some("node_fork"), None, 8)
            .expect("fork at node");
    let forked: ContextTreeLedger = serde_json::from_str(&forked_json).expect("forked root");
    let branch = forked.branches.get("node_fork").expect("node fork");

    assert_eq!(branch.fork_ref.as_ref().and_then(|r| r.revision), Some(0));
    assert!(branch.current.component_records().contains_key("later"));
}

#[test]
fn native_ledger_fork_methods_create_lineage_without_json_wrapper() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [
                {"id":"text","componentType":"text_content","content":"hello"},
                {"id":"later","componentType":"text_content","offset":1,"content":"world"}
            ]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");

    let (root_store, revision_branch_id) = root_store
        .fork_at_revision(
            "main",
            Some(0),
            Some("native_revision_fork".to_string()),
            BTreeMap::from([("view_kind".to_string(), json!("review"))]),
            8,
        )
        .expect("native revision fork");
    assert_eq!(revision_branch_id, "native_revision_fork");
    let branch = root_store
        .branches
        .get("native_revision_fork")
        .expect("revision fork");
    assert_eq!(branch.parent_branch_id.as_deref(), Some("main"));
    assert_eq!(branch.fork_ref.as_ref().and_then(|r| r.revision), Some(0));
    assert_eq!(branch.aux["view_kind"], "review");
    assert!(branch.current.component_records().contains_key("later"));

    let (root_store, component_branch_id) = root_store
        .fork_at_component(
            "main",
            "later",
            Some("native_component_fork".to_string()),
            BTreeMap::new(),
            8,
        )
        .expect("native component fork");
    assert_eq!(component_branch_id, "native_component_fork");
    let branch = root_store
        .branches
        .get("native_component_fork")
        .expect("component fork");
    assert_eq!(branch.fork_ref.as_ref().and_then(|r| r.revision), Some(0));
    assert!(branch.current.component_records().contains_key("later"));
}

#[test]
fn root_store_records_and_forks_at_transaction_boundary() {
    let store_json = new_store_json(sample_tree(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");
    let trace = transaction_trace(&root_store, "txn_1");
    let trace_json = serde_json::to_string(&trace).expect("trace json");
    let root_json = ledger_record_transaction_trace_json(&root_json, &trace_json)
        .expect("record transaction trace");
    let forked_json = ledger_create_branch_at_transaction_json(
        &root_json,
        "main",
        "txn_1",
        Some("branch_txn"),
        Some(r#"{"branch_kind":"overlay","view_kind":"review"}"#),
        8,
    )
    .expect("fork at transaction");
    let forked: ContextTreeLedger = serde_json::from_str(&forked_json).expect("forked root");
    let derived_branch = forked.branches.get("branch_txn").expect("derived branch");

    assert!(forked.event_index.transaction_traces.contains_key("txn_1"));
    assert_eq!(derived_branch.parent_branch_id.as_deref(), Some("main"));
    assert_eq!(
        derived_branch.fork_ref.as_ref().and_then(|r| r.revision),
        Some(1)
    );
    assert_eq!(
        derived_branch
            .fork_ref
            .as_ref()
            .and_then(|r| r.transaction_id.as_deref()),
        Some("txn_1")
    );
    assert_eq!(
        derived_branch.current.component_records()["text"]
            .content
            .as_deref(),
        Some("hello")
    );
    assert_eq!(derived_branch.aux["view_kind"], "review");
}

#[test]
fn native_ledger_records_trace_and_forks_at_transaction() {
    let store_json = new_store_json(sample_tree(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let root_store: ContextTreeLedger = serde_json::from_str(&root_json).expect("root json");
    let trace = transaction_trace(&root_store, "txn_native");

    let root_store = root_store
        .record_transaction_trace(trace)
        .expect("record native trace");
    assert!(
        root_store
            .event_index
            .transaction_traces
            .contains_key("txn_native")
    );
    assert_eq!(
        root_store.event_index.by_transaction["txn_native"],
        vec!["main".to_string()]
    );

    let (root_store, branch_id) = root_store
        .fork_at_transaction(
            "main",
            "txn_native",
            Some("native_txn_fork".to_string()),
            BTreeMap::from([("view_kind".to_string(), json!("review"))]),
            8,
        )
        .expect("native transaction fork");

    assert_eq!(branch_id, "native_txn_fork");
    let branch = root_store
        .branches
        .get("native_txn_fork")
        .expect("transaction fork");
    assert_eq!(
        branch
            .fork_ref
            .as_ref()
            .and_then(|r| r.transaction_id.as_deref()),
        Some("txn_native")
    );
    assert_eq!(branch.aux["view_kind"], "review");
    assert_eq!(
        branch.current.component_records()["text"]
            .content
            .as_deref(),
        Some("hello")
    );
}

#[test]
fn root_store_projects_and_projects_branch_revisions() {
    let root_json = new_ledger_json(sample_tree(), Some("main"), 8).expect("root store");
    let ledger = decode_context_tree_ledger(&root_json).expect("decode ledger");
    let op = PactOperation::Update {
        envelope: test_envelope("main"),
        target_component_id: Some("text".to_string()),
        target_selector: None,
        content: Some("changed".to_string()),
        content_ref: None,
        fields: BTreeMap::new(),
        aux: BTreeMap::new(),
    };
    let mutated = seal_ops(ledger, "main", vec![op]);
    let root_json = serde_json::to_string(&mutated).expect("encode mutated ledger");

    let old_tree_json =
        ledger_project_branch_tree_json(&root_json, "main", Some(0)).expect("project");
    let old_tree: Value = serde_json::from_str(&old_tree_json).expect("tree json");
    let view_json = ledger_project_branch_json(&root_json, "main", None, 8).expect("project");
    let view: PactView = serde_json::from_str(&view_json).expect("view json");

    assert_eq!(old_tree["children"][0]["children"][0]["content"], "hello");
    assert_eq!(view.branch_id, "main");
    assert_eq!(view.head_revision, 1);
    assert_eq!(
        view.component_records()["text"].content.as_deref(),
        Some("changed")
    );
}

#[test]
fn archive_scope_projects_deleted_nodes_as_tombstones() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [
                {"id":"text","componentType":"text_content","content":"hello"},
                {
                    "id":"gone_parent",
                    "componentType":"container",
                    "offset":1,
                    "children":[
                        {"id":"gone_child","componentType":"text_content","content":"archived"}
                    ]
                }
            ]
        }]
    });
    let root_json = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    let ledger = decode_context_tree_ledger(&root_json).expect("decode ledger");
    let (ledger, _report) = ledger
        .prune("main", Some("gone_parent".to_string()), None)
        .expect("native delete");
    let root_json = serde_json::to_string(&ledger).expect("encode mutated ledger");

    let head_json =
        ledger_project_branch_tree_json(&root_json, "main", None).expect("head project");
    let archive_json = ledger_project_branch_tree_json_with_scope(
        &root_json,
        "main",
        None,
        ProjectionTreeScope::Archive,
    )
    .expect("archive project");
    let head: Value = serde_json::from_str(&head_json).expect("head json");
    let archive: Value = serde_json::from_str(&archive_json).expect("archive json");

    assert!(!node_exists(&head, "gone_parent"));

    let archived_parent = find_node(&archive, "gone_parent").expect("archived parent");
    assert_eq!(archived_parent["pact_lifecycle"], json!("deleted"));
    assert_eq!(archived_parent["visible_in_head"], json!(false));
    assert_eq!(
        archived_parent["children"][0]["pact_lifecycle"],
        json!("deleted")
    );
    assert_eq!(
        archived_parent["children"][0]["visible_in_head"],
        json!(false)
    );

    let active = find_node(&archive, "text").expect("active text");
    assert_eq!(active["pact_lifecycle"], json!("active"));
    assert_eq!(active["visible_in_head"], json!(true));
}

#[test]
fn archive_scope_projection_includes_deleted_nodes_and_latest_replacements() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": [
                {"id":"text","componentType":"text_content","content":"latest"},
                {"id":"gone","componentType":"text_content","offset":1,"content":"deleted"}
            ]
        }]
    });
    let root_json = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    let ledger = decode_context_tree_ledger(&root_json).expect("decode ledger");
    let (ledger, _report) = ledger
        .prune("main", Some("gone".to_string()), None)
        .expect("native delete");
    let root_json = serde_json::to_string(&ledger).expect("encode mutated ledger");

    let head_view_json =
        ledger_project_branch_json(&root_json, "main", None, 8).expect("head project");
    let archive_view_json = ledger_project_branch_json_with_scope(
        &root_json,
        "main",
        None,
        8,
        ProjectionTreeScope::Archive,
    )
    .expect("archive project");
    let head_view: Value = serde_json::from_str(&head_view_json).expect("head view");
    let archive_view: Value = serde_json::from_str(&archive_view_json).expect("archive view");

    assert!(head_view["components"].get("gone").is_none());
    assert!(archive_view["components"].get("gone").is_some());
    assert_eq!(
        archive_view["components"]["gone"]["aux"]["pact_lifecycle"],
        json!("deleted")
    );
    assert_eq!(
        archive_view["components"]["gone"]["aux"]["visible_in_head"],
        json!(false)
    );
    assert_eq!(
        archive_view["components"]["text"]["content"],
        json!("latest")
    );
}

#[test]
fn archive_projection_does_not_double_apply_traces_from_current_head_base() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "message_turn",
            "pact_depth": 0,
            "role": "user",
            "children": []
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let mut ledger: ContextTreeLedger = serde_json::from_str(&root_json).expect("ledger");

    let branch = ledger.branches.get_mut("main").expect("main");
    branch.base_ref.content_ref = None;
    branch.fork_ref = None;
    branch.head_revision = 1;
    branch.current.head_revision = 1;

    let mut inserted = json!({
        "id": "inserted_1",
        "componentType": "text_content",
        "content": "once"
    });
    let active = branch
        .current
        .tree
        .root
        .children
        .iter_mut()
        .find(|c| c.id == "active")
        .expect("active");
    active
        .children
        .push(Component::from_component_value(&inserted).expect("component"));

    inserted["parent_id"] = json!("active");
    branch.transaction_traces = vec![crate::transaction_trace::TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: vec![crate::transaction_trace::TraceOp::new(
            "inserted_1",
            crate::transaction_trace::TraceOpKind::InsertComponent {
                parent_id: "active".to_string(),
                index: 0,
                component: inserted,
            },
        )],
    }];

    let branch = ledger.branches.get("main").expect("main");
    let (archive, _first_seen) =
        project_branch_archive_component_tree(&ledger, branch, Some(1)).expect("archive");
    let count = archive
        .root()
        .children
        .iter()
        .find(|c| c.id == "active")
        .map(|c| c.children.iter().filter(|n| n.id == "inserted_1").count())
        .unwrap_or(0);

    assert_eq!(
        count, 1,
        "inserted_1 must appear exactly once, not double-applied"
    );
}

#[test]
fn trace_replay_batches_ops_into_a_single_tree_edit() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active", "componentType": "message_turn",
            "pact_depth": 0, "role": "user", "children": []
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let mut ledger: ContextTreeLedger = serde_json::from_str(&root_json).expect("ledger");

    let branch = ledger.branches.get_mut("main").expect("main");
    branch.base_ref.content_ref = None;
    branch.fork_ref = None;
    branch.head_revision = 1;

    branch.current.head_revision = 0;
    let ops = (0..4)
        .map(|i| {
            crate::transaction_trace::TraceOp::new(
                format!("n{i}"),
                crate::transaction_trace::TraceOpKind::InsertComponent {
                    parent_id: "active".to_string(),
                    index: i,
                    component: json!({
                        "id": format!("n{i}"),
                        "componentType": "text_content",
                        "content": format!("c{i}")
                    }),
                },
            )
        })
        .collect();
    branch.transaction_traces = vec![crate::transaction_trace::TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops,
    }];

    let branch = ledger.branches.get("main").expect("main");
    let before = crate::tests::instrumentation::TREE_EDIT_CALLS.with(|calls| calls.get());
    let projected = project_branch_component_tree(&ledger, branch, Some(1)).expect("project");
    let edits = crate::tests::instrumentation::TREE_EDIT_CALLS.with(|calls| calls.get()) - before;

    assert!(projected.contains("n3"), "all four ops applied");
    assert!(
        edits <= 1,
        "replaying one trace with 4 ops must cost at most one tree edit, got {edits}"
    );
}

#[test]
fn set_field_on_parent_preserves_descendants_and_their_order() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active", "componentType": "message_turn",
            "pact_depth": 0, "role": "user",
            "children": [
                {"id":"k1","componentType":"text_content","content":"one"},
                {"id":"k2","componentType":"text_content","content":"two"}
            ]
        }]
    });
    let store_json = new_store_json(&tree.to_string(), 8).expect("store");
    let root_json = ledger_from_store_json(&store_json, Some("main")).expect("root store");
    let mut ledger: ContextTreeLedger = serde_json::from_str(&root_json).expect("ledger");

    let branch = ledger.branches.get_mut("main").expect("main");
    branch.base_ref.content_ref = None;
    branch.fork_ref = None;
    branch.head_revision = 1;
    branch.current.head_revision = 0;
    branch.transaction_traces = vec![crate::transaction_trace::TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: vec![crate::transaction_trace::TraceOp::new(
            "active",
            crate::transaction_trace::TraceOpKind::SetField {
                key: "role".to_string(),
                value: json!("assistant"),
            },
        )],
    }];

    let branch = ledger.branches.get("main").expect("main");
    let projected = project_branch_component_tree(&ledger, branch, Some(1)).expect("project");
    let active = projected
        .component("active")
        .expect("active survives the field op");
    assert_eq!(
        active
            .children
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["k1", "k2"],
        "children and their order survive a SetField on the parent"
    );
}

#[test]
fn root_store_inspects_root_and_branch_without_projecting_full_tree() {
    let root_json = ledger_from_store_json(
        &new_store_json(sample_tree(), 8).expect("store"),
        Some("main"),
    )
    .expect("root store");

    let root_summary_json = ledger_inspect_json(&root_json).expect("inspect root");
    let branch_summary_json =
        ledger_inspect_branch_json(&root_json, "main").expect("inspect branch");
    let root_summary: Value = serde_json::from_str(&root_summary_json).expect("root summary");
    let branch_summary: Value = serde_json::from_str(&branch_summary_json).expect("branch summary");

    assert_eq!(root_summary["branch_count"], 1);
    assert_eq!(root_summary["branches"]["main"]["message_count"], 1);
    assert_eq!(branch_summary["branch_id"], "main");
    assert_eq!(branch_summary["message_count"], 1);
}

#[test]
fn root_store_appends_to_selected_branch_only() {
    let pact = r#"{
          "id":"root",
          "componentType":"root",
          "children":[
            {"id":"source_msg","componentType":"message_turn","role":"user","content":"hello"}
          ]
        }"#;
    let root_json = new_ledger_json(pact, Some("main"), 8).expect("builds root store");
    let forked_json = ledger_create_branch_at_revision_json(
        &root_json,
        "main",
        Some(0),
        Some("branch_alpha_01"),
        Some(r#"{"branch_kind":"derived","view_kind":"review"}"#),
        8,
    )
    .expect("forks derived branch");
    let updated_json = append_component_json(
        &forked_json,
        "branch_alpha_01",
        "source_msg",
        json!({
            "id": "view_msg",
            "componentType": "text_content",
            "type": "text_content",
            "metadata": {
                "source": "review",
                "kind": "branch_note",
                "aux": {
                    "view_actor_kind": "overlay",
                    "view_kind": "review",
                    "source_branch_id": "branch_alpha_01",
                    "target_branch_id": "main"
                }
            },
            "content": "<introspective-thoughts>check build</introspective-thoughts>"
        }),
    );
    let updated = decode_context_tree_ledger(&updated_json).expect("decodes updated root");

    assert_eq!(updated.branches["branch_alpha_01"].head_revision, 1);
    assert_eq!(updated.branches["main"].head_revision, 0);
    assert!(
        updated.branches["branch_alpha_01"]
            .current
            .component_records()
            .contains_key("view_msg")
    );
    assert!(
        !updated.branches["main"]
            .current
            .component_records()
            .contains_key("view_msg")
    );
    let aux = &updated.branches["branch_alpha_01"]
        .current
        .component_records()["view_msg"]
        .aux;
    assert_eq!(aux["metadata"]["aux"]["view_kind"], "review");
}

#[test]
fn pact_ref_uses_transaction_identity_not_cycle_identity() {
    let pact_ref = PactRef {
        root_id: "root_1".to_string(),
        branch_id: "main".to_string(),
        revision: Some(42),
        transaction_id: Some("txn_21".to_string()),
        component_id: Some("node_1".to_string()),
        content_ref: None,
    };

    let json = serde_json::to_string(&pact_ref).expect("ref json");
    assert!(json.contains("\"transaction_id\":\"txn_21\""));
    assert!(!json.contains("cycle"));
}

#[test]
fn transaction_trace_round_trips_without_tool_dispatch_boundary() {
    let base_ref = PactRef {
        root_id: "root_1".to_string(),
        branch_id: "main".to_string(),
        revision: Some(7),
        transaction_id: Some("txn_7".to_string()),
        component_id: None,
        content_ref: None,
    };
    let request_ref = PactRef {
        content_ref: Some("cnt_request".to_string()),
        ..base_ref.clone()
    };
    let response_ref = PactRef {
        content_ref: Some("cnt_response".to_string()),
        ..base_ref.clone()
    };
    let trace = PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: "root_1".to_string(),
        branch_id: "main".to_string(),
        transaction_id: "txn_7".to_string(),
        authored_ref: base_ref.clone(),
        pre_request_ref: base_ref.clone(),
        post_request_alteration_ref: base_ref.clone(),
        request_ref: Some(request_ref),
        response_ref: Some(response_ref),
        post_response_alteration_ref: Some(base_ref.clone()),
        post_response_ref: Some(PactRef {
            revision: Some(8),
            ..base_ref
        }),
        alteration_ids: Vec::new(),
        commit: None,
        replay_trace: None,
        aux: BTreeMap::new(),
    };

    trace.validate().expect("valid trace");
    let json = serde_json::to_string(&trace).expect("trace json");
    assert!(json.contains("post_request_alteration_ref"));
    assert!(json.contains("request_ref"));
    assert!(!json.contains("tool_dispatch"));
    assert!(!json.contains("ToolDispatch"));
}

#[test]
fn transaction_trace_builder_creates_valid_native_refs() {
    let request = r#"{
          "schema": "pact.transaction_trace_build_request.v1",
          "root_id": "root_1",
          "branch_id": "main",
          "transaction_id": "txn_7",
          "authored_ref": {"revision": 1},
          "pre_request_ref": {"revision": 2},
          "post_request_alteration_ref": {"revision": 3},
          "request_ref": {"revision": 3, "content_ref": "cnt_request"},
          "response_ref": {"revision": 3, "content_ref": "cnt_response"},
          "post_response_ref": {"revision": 4},
          "aux": {"request_id": "req_1", "response_id": "res_1"}
        }"#;

    let trace_json = transaction_trace_build_json(request).expect("build trace");
    let trace: PactTransactionTrace = serde_json::from_str(&trace_json).expect("trace json");

    assert_eq!(trace.schema, TRANSACTION_TRACE_SCHEMA);
    assert_eq!(trace.authored_ref.root_id, "root_1");
    assert_eq!(trace.authored_ref.revision, Some(1));
    assert_eq!(
        trace
            .request_ref
            .as_ref()
            .and_then(|pact_ref| pact_ref.content_ref.as_deref()),
        Some("cnt_request")
    );
    assert_eq!(
        trace
            .response_ref
            .as_ref()
            .and_then(|pact_ref| pact_ref.content_ref.as_deref()),
        Some("cnt_response")
    );
    assert_eq!(
        trace
            .post_response_ref
            .as_ref()
            .and_then(|pact_ref| pact_ref.revision),
        Some(4)
    );
    assert_eq!(
        trace.aux.get("request_id").and_then(|v| v.as_str()),
        Some("req_1")
    );
    trace.validate().expect("valid trace");
}

#[test]
fn transaction_trace_builder_rejects_empty_boundary_refs() {
    let request = r#"{
          "schema": "pact.transaction_trace_build_request.v1",
          "root_id": "root_1",
          "branch_id": "main",
          "transaction_id": "txn_7",
          "authored_ref": {},
          "pre_request_ref": {"revision": 2},
          "post_request_alteration_ref": {"revision": 3}
        }"#;

    let error = transaction_trace_build_json(request).expect_err("rejects empty ref");
    assert!(
        error
            .to_string()
            .contains("authored_ref requires at least one")
    );
}

#[test]
fn transaction_trace_commit_accepts_structured_content_ref() {
    let base_ref = PactRef {
        root_id: "root_1".to_string(),
        branch_id: "main".to_string(),
        revision: Some(7),
        transaction_id: Some("txn_7".to_string()),
        component_id: None,
        content_ref: None,
    };
    let trace = PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: "root_1".to_string(),
        branch_id: "main".to_string(),
        transaction_id: "txn_7".to_string(),
        authored_ref: base_ref.clone(),
        pre_request_ref: base_ref.clone(),
        post_request_alteration_ref: base_ref.clone(),
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: Some(base_ref.clone()),
        alteration_ids: Vec::new(),
        replay_trace: None,
        aux: BTreeMap::new(),
        commit: Some(PactCommit {
            commit_id: "commit_12345678".to_string(),
            phase: "pre_request".to_string(),
            kind: "surface_commit".to_string(),
            pact_ref: None,
            content_ref: Some(serde_json::json!({
                "kind": "surface_commit",
                "path": "content/surface_commit.json"
            })),
            created_at: Some("2026-05-18T00:00:00Z".to_string()),
        }),
    };

    trace.validate().expect("valid trace");
    let json = serde_json::to_string(&trace).expect("trace json");
    assert!(json.contains("\"commit\""));
    assert!(json.contains("\"kind\":\"surface_commit\""));
    assert!(json.contains("\"path\":\"content/surface_commit.json\""));
}

#[test]
fn transaction_trace_rejects_refs_from_another_branch() {
    let trace = PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: "root_1".to_string(),
        branch_id: "main".to_string(),
        transaction_id: "txn_7".to_string(),
        authored_ref: PactRef {
            root_id: "root_1".to_string(),
            branch_id: "branch_b".to_string(),
            revision: Some(1),
            transaction_id: Some("txn_7".to_string()),
            component_id: None,
            content_ref: None,
        },
        pre_request_ref: PactRef {
            root_id: "root_1".to_string(),
            branch_id: "main".to_string(),
            revision: Some(1),
            transaction_id: Some("txn_7".to_string()),
            component_id: None,
            content_ref: None,
        },
        post_request_alteration_ref: PactRef {
            root_id: "root_1".to_string(),
            branch_id: "main".to_string(),
            revision: Some(1),
            transaction_id: Some("txn_7".to_string()),
            component_id: None,
            content_ref: None,
        },
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: None,
        alteration_ids: Vec::new(),
        commit: None,
        replay_trace: None,
        aux: BTreeMap::new(),
    };

    assert!(trace.validate().is_err());
}

#[test]
fn owned_projection_builder_matches_the_borrowed_builder_exactly() {
    let root = crate::Component::from_component_value(&serde_json::json!({
        "id": "root", "componentType": "root", "children": [
            {"id": "m1", "componentType": "message_turn", "role": "user", "children": [
                {"id": "t1", "componentType": "text_content", "content": "x".repeat(2048)}
            ]},
            {"id": "m2", "componentType": "message_turn", "role": "assistant", "children": [
                {"id": "t2", "componentType": "text_content", "content": "short"}
            ]}
        ]
    }))
    .expect("component tree");
    let mut first_seen = std::collections::BTreeMap::new();
    first_seen.insert("m1".to_string(), 3_u64);
    let borrowed = crate::store::build_projection_from_component(&root, &first_seen, 1024)
        .expect("borrowed projection");
    let owned = crate::store::build_projection_from_owned_component(root, first_seen, 1024)
        .expect("owned projection");
    assert_eq!(
        serde_json::to_value(&borrowed).expect("borrowed json"),
        serde_json::to_value(&owned).expect("owned json")
    );
}
