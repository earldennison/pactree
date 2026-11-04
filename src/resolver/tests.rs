use super::PactResolver;
use super::commit::{commit_record_from_mutations, find_node_value};
use crate::components::{
    Component, ComponentInternalRefs, ComponentLifecycle, ComponentLifecyclePolicy,
    DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES, InternalRef, LifecycleState,
};
use crate::file_format::CommitRecordOp;
use crate::mutation::MutationKind;
use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
use crate::selector::select_json;
use crate::store::project_branch_component_tree;
use crate::trace::{BoundaryKind, StagedTraceTarget};
use crate::tree::ComponentTree;
use serde_json::{Value, json};

fn collect_hollow_message_turn_ids(tree: &ComponentTree, out: &mut Vec<String>) {
    super::tools::collect_hollow_context_container_ids(tree.root(), out);
}
use std::collections::BTreeMap as Map;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn fresh_resolver() -> PactResolver {
    let root = r#"{
            "id": "root",
            "componentType": "root",
            "children": []
        }"#;
    PactResolver::from_initial_tree(root, Some("main")).expect("resolver")
}

fn component(mut value: Value) -> Component {
    if let Some(object) = value.as_object_mut()
        && !object.contains_key("id")
    {
        let id = TEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        object.insert(
            "id".to_string(),
            Value::String(format!("test_component_{id}")),
        );
    }
    Component::from_component_value(&value).expect("component")
}

fn component_without_id(mut value: Value) -> Component {
    if let Some(object) = value.as_object_mut() {
        object
            .entry("id".to_string())
            .or_insert_with(|| Value::String("__test_blank_id__".to_string()));
    }
    let mut component = component(value);
    component.id.clear();
    component
}

fn lifecycle_marker(label: &str, revision: u64) -> ComponentLifecycle {
    ComponentLifecycle {
        state: LifecycleState::RenderRequested,
        revision,
        policy: ComponentLifecyclePolicy {
            ttl: Some(3),
            cad: Some(2),
        },
        internal_refs: ComponentInternalRefs {
            state: Some(InternalRef {
                id: format!("{label}_state_ref"),
                kind: "state".to_string(),
                field: Some("lifecycle".to_string()),
                aux: Map::new(),
            }),
            projection: Some(InternalRef {
                id: format!("{label}_projection_ref"),
                kind: "projection".to_string(),
                field: None,
                aux: Map::new(),
            }),
        },
    }
}

fn lifecycle_component(id: &str, label: &str, revision: u64) -> Component {
    let mut component = component(json!({
        "id": id,
        "componentType": "text_content",
        "content": label
    }));
    component.lifecycle = lifecycle_marker(label, revision);
    component
}

fn fresh_provider_resolver() -> PactResolver {
    let root = r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {
                    "id": "active",
                    "componentType": "active_message",
                    "type": "message_turn",
                    "role": "user",
                    "children": [
                        {
                            "id": "active_container",
                            "componentType": "message_container",
                            "children": []
                        }
                    ]
                }
            ]
        }"#;
    PactResolver::from_initial_tree(root, Some("main")).expect("resolver")
}

fn resolver_with_tree(root: &str) -> PactResolver {
    PactResolver::from_initial_tree(root, Some("main")).expect("resolver")
}

fn env(branch: &str) -> PactOperationEnvelope {
    PactOperationEnvelope {
        branch_id: branch.to_string(),
        source: SourceActor::User,
        source_id: None,
        host_timestamp_ns: None,
        idempotency_key: None,
        aux: Map::new(),
    }
}

fn next_test_id(prefix: &str) -> String {
    let id = TEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{id:08x}")
}

fn count_node_id(value: &Value, target_id: &str) -> usize {
    let here = usize::from(value.get("id").and_then(Value::as_str) == Some(target_id));
    let children = value
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|child| count_node_id(child, target_id))
        .sum::<usize>();
    here + children
}

fn find_node_by_content<'a>(value: &'a Value, content: &str) -> Option<&'a Value> {
    if value.get("content").and_then(Value::as_str) == Some(content) {
        return Some(value);
    }
    value
        .get("children")
        .and_then(Value::as_array)
        .and_then(|children| {
            children
                .iter()
                .find_map(|child| find_node_by_content(child, content))
        })
}

fn message_op(
    envelope: PactOperationEnvelope,
    role: String,
    text: Option<String>,
    content_ref: Option<String>,
) -> PactOperation {
    if role == "system" {
        let mut node = json!({
            "id": next_test_id("sys"),
            "componentType": "text_content",
            "type": "text_content",
            "parent_id": "system",
            "offset": 0,
            "pact_depth": -1,
        });
        if let Some(text) = text {
            node["content"] = Value::String(text);
        } else if let Some(content_ref) = content_ref {
            node["content_ref"] = json!({"id": content_ref});
        }
        return PactOperation::Insert {
            envelope,
            target_selector: None,
            parent_id: Some("system".to_string()),
            depth: None,
            offset: None,
            component: component(node),
        };
    }

    let message_id = next_test_id("msg");
    let mut children = Vec::new();
    if let Some(text) = text {
        children.push(json!({
            "id": format!("{message_id}_text"),
            "componentType": "text_content",
            "type": "text_content",
            "parent_id": message_id,
            "offset": 0,
            "pact_depth": 1,
            "content": text,
        }));
    } else if let Some(content_ref) = content_ref {
        children.push(json!({
            "id": format!("{message_id}_text"),
            "componentType": "text_content",
            "type": "text_content",
            "parent_id": message_id,
            "offset": 0,
            "pact_depth": 1,
            "content_ref": {"id": content_ref},
        }));
    }
    PactOperation::Insert {
        envelope,
        target_selector: None,
        parent_id: Some("root".to_string()),
        depth: None,
        offset: None,
        component: component(json!({
            "id": message_id,
            "componentType": "message_turn",
            "type": "message_turn",
            "role": role,
            "parent_id": "root",
            "offset": 0,
            "pact_depth": 1,
            "children": children,
        })),
    }
}

#[test]
fn ingest_validates_and_dedups() {
    let mut resolver = fresh_resolver();
    let ops = vec![
        message_op(
            PactOperationEnvelope {
                idempotency_key: Some("k_1".to_string()),
                ..env("main")
            },
            "user".to_string(),
            Some("hello".to_string()),
            None,
        ),
        message_op(
            PactOperationEnvelope {
                idempotency_key: Some("k_1".to_string()),
                ..env("main")
            },
            "user".to_string(),
            Some("hello again".to_string()),
            None,
        ),
        PactOperation::Update {
            envelope: env("main"),
            target_component_id: Some("n_1".to_string()),
            target_selector: None,
            content: None,
            content_ref: None,
            fields: Map::new(),
            aux: Map::new(),
        },
    ];
    let outcome = resolver.ingest_operations(ops).expect("ingest");
    assert_eq!(outcome.accepted, 1);
    assert_eq!(outcome.deduped, 1);
    assert_eq!(outcome.rejected.len(), 1);
    assert_eq!(resolver.pending("main").len(), 1);
}

fn produced_component_ids(outcome: &crate::trace::ResolveOutcome) -> Vec<String> {
    outcome
        .traces
        .iter()
        .flat_map(|trace| trace.produced_refs.iter())
        .filter_map(|pact_ref| pact_ref.component_id.clone())
        .collect()
}

#[test]
fn insert_without_node_id_assigns_native_id() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component_without_id(
                json!({"componentType": "text_content", "type": "text_content", "content": "x"}),
            ),
        }])
        .expect("ingest");
    let resolved = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let produced = produced_component_ids(&resolved);
    assert_eq!(
        produced.len(),
        1,
        "expected one produced ref, got {produced:?}"
    );
    assert!(
        produced[0].starts_with("node_"),
        "expected native node id, got {produced:?}"
    );
}

#[test]
fn append_without_node_id_assigns_native_id() {
    let mut resolver = resolver_with_tree(
        r#"{"id":"root","componentType":"root","children":[{"id":"parent","componentType":"message_container","offset":0,"children":[]}]}"#,
    );
    resolver
        .ingest_operations(vec![PactOperation::Append {
            envelope: env("main"),
            target_component_id: Some("parent".to_string()),
            target_selector: None,
            offset: None,
            component: component_without_id(
                json!({"componentType": "text_content", "type": "text_content", "content": "y"}),
            ),
        }])
        .expect("ingest");
    let resolved = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let produced = produced_component_ids(&resolved);
    assert_eq!(
        produced.len(),
        1,
        "expected one produced ref, got {produced:?}"
    );
    assert!(
        produced[0].starts_with("node_"),
        "expected native node id, got {produced:?}"
    );
}

#[test]
fn replace_without_node_id_preserves_target_id() {
    let mut resolver = resolver_with_tree(
        r#"{"id":"root","componentType":"root","children":[{"id":"keep","componentType":"text_content","type":"text_content","offset":0,"content":"a"}]}"#,
    );
    resolver
        .ingest_operations(vec![PactOperation::Replace {
            envelope: env("main"),
            target_component_id: Some("keep".to_string()),
            target_selector: None,
            component: component_without_id(
                json!({"componentType": "text_content", "type": "text_content", "content": "b"}),
            ),
        }])
        .expect("ingest");
    let resolved = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let produced = produced_component_ids(&resolved);
    assert!(
        produced.contains(&"keep".to_string()),
        "replace should preserve the target id, got {produced:?}"
    );
}

#[test]
fn resolve_inserted_message_advances_head_after_commit() {
    let mut resolver = fresh_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("first message".to_string()),
            None,
        )])
        .expect("ingest");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    assert_eq!(outcome.applied, vec!["insert".to_string()]);
    assert!(outcome.commit_candidate.is_some());

    let candidate = outcome.commit_candidate.unwrap();
    let commit = resolver.seal(&candidate).expect("seal");
    assert_eq!(commit.revision, initial_rev + 1);

    let post_head = resolver.ledger.branches.get("main").unwrap().head_revision;
    assert_eq!(post_head, initial_rev + 1);
}

#[test]
fn inserted_system_text_mutates_system_header_not_user_turn() {
    let root = r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {"id": "system", "componentType": "system_header", "children": []}
            ]
        }"#;
    let mut resolver = PactResolver::from_initial_tree(root, Some("main")).expect("resolver");
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "system".to_string(),
            Some("runtime protocol".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let thread = resolver
        .build_provider_thread(
            "main",
            None,
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("provider thread");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["message_type"], "system_header");
    assert_eq!(messages[0]["content"][0]["content"], "runtime protocol");
}

#[test]
fn native_add_message_lowers_to_primitive_message_insert() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "user",
            vec![component(
                json!({"componentType": "text_content", "content": "hello from native helper"}),
            )],
            Map::new(),
        )
        .expect("native add_message");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let preview = resolver.preview("main").expect("preview");
    assert_eq!(preview["children"][0]["role"], "user");
    assert_eq!(
        preview["children"][0]["children"][0]["content"],
        "hello from native helper"
    );
}

#[test]
fn serialized_empty_text_preserves_explicit_id_without_status_marker() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message_json(
            "main",
            "assistant",
            vec![
                json!({
                    "id": "reserved_empty_text",
                    "componentType": "text_content",
                    "content": ""
                }),
                json!({
                    "componentType": "text_content",
                    "content": ""
                }),
            ],
            Map::new(),
        )
        .expect("serialized add_message");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let preview = resolver.preview("main").expect("preview");
    let reserved = find_node_value(&preview, "reserved_empty_text")
        .expect("explicitly identified empty text should be preserved");
    assert_eq!(reserved["content"], "");
    assert!(reserved.get("content_status").is_none());

    let assistant = preview["children"]
        .as_array()
        .expect("root children")
        .iter()
        .find(|node| node["role"] == "assistant")
        .expect("assistant message");
    assert_eq!(
        assistant["children"]
            .as_array()
            .expect("assistant children")
            .len(),
        1,
        "anonymous empty text should remain a no-op"
    );
}

#[test]
fn native_add_message_assigns_role_depths_and_preserves_text_order() {
    let root = r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {"id": "system", "componentType": "system_header", "children": []}
            ]
        }"#;
    let mut resolver = PactResolver::from_initial_tree(root, Some("main")).expect("resolver");
    resolver
        .add_message(
            "main",
            "system",
            vec![component(
                json!({"componentType": "text_content", "content": "system first"}),
            )],
            Map::new(),
        )
        .expect("system add");
    resolver
        .add_message(
            "main",
            "assistant",
            vec![
                component(json!({"componentType": "text_content", "content": "before"})),
                component(json!({
                    "componentType": "tool_call",
                    "tool_call_id": "call_order_1",
                    "tool_name": "shell_execute",
                    "parameters": {"cmd": "pwd"}
                })),
            ],
            Map::new(),
        )
        .expect("assistant add");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let preview = resolver.preview("main").expect("preview");
    let system = preview["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "system")
        .unwrap();
    assert_eq!(system["children"][0]["pact_depth"], -1);
    let assistant = preview["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["role"] == "assistant")
        .unwrap();
    assert_eq!(assistant["children"][0]["componentType"], "text_content");
    assert_eq!(assistant["children"][1]["componentType"], "tool_call");
}

#[test]
fn native_add_message_preserves_canonical_tool_nodes() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "assistant",
            vec![
                component(json!({"componentType": "text_content", "content": "I will run a tool"})),
                component(json!({
                    "componentType": "tool_call",
                    "tool_call_id": "call_native_1",
                    "tool_name": "shell_execute",
                    "parameters": {"cmd": "pwd"},
                    "key": "shell"
                })),
            ],
            Map::new(),
        )
        .expect("native assistant add_message");
    resolver
        .add_message(
            "main",
            "user",
            vec![component(json!({
                "componentType": "tool_result",
                "tool_call_id": "call_native_1",
                "success": true,
                "result": {"stdout": "/tmp"}
            }))],
            Map::new(),
        )
        .expect("native result add_message");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    assert!(
        resolver.unpaired_tool_call_ids().is_empty(),
        "native tool pair should be complete"
    );
    let preview = resolver.preview("main").expect("preview");
    let preview = ComponentTree::from_value(&preview).expect("preview component tree");
    let mut hollow = Vec::new();
    collect_hollow_message_turn_ids(&preview, &mut hollow);
    assert!(
        hollow.is_empty(),
        "native tool result should not leave hollow turns"
    );
    let calls = resolver.list_tool_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_call_id, "call_native_1");
    assert_eq!(calls[0].status, crate::tools::ToolCallStatus::Completed);
}

#[test]
fn native_add_message_preserves_reasoning_and_text_order() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "assistant",
            vec![
                component(
                    json!({"componentType": "reasoning_content", "content": "thinking it through"}),
                ),
                component(
                    json!({"componentType": "text_content", "content": "here is the answer"}),
                ),
            ],
            Map::new(),
        )
        .expect("native reasoning add_message");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let preview = resolver.preview("main").expect("preview");
    let assistant = preview["children"]
        .as_array()
        .and_then(|children| {
            children
                .iter()
                .find(|child| child["role"] == "assistant")
                .cloned()
        })
        .expect("assistant turn");
    let kids = assistant["children"].as_array().expect("children");
    assert_eq!(kids.len(), 2, "{kids:?}");
    assert_eq!(kids[0]["componentType"], "reasoning_content");
    assert_eq!(kids[0]["content"], "thinking it through");
    assert_eq!(kids[1]["componentType"], "text_content");
    assert_eq!(kids[1]["content"], "here is the answer");
    assert_eq!(kids[0]["offset"].as_i64(), Some(0));
    assert_eq!(kids[1]["offset"].as_i64(), Some(1));
}

#[test]
fn tool_result_batch_does_not_leave_empty_user_turn() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                None,
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_tc_surface",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_surface",
                    "tool_name": "shell",
                    "parameters": {"cmd": "pwd"}
                })),
            },
        ])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve");

    resolver
        .ingest_operations(vec![
            message_op(env("main"), "user".to_string(), None, None),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Host,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='user'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "result_tc_surface",
                    "componentType": "tool_result",
                    "tool_call_id": "tc_surface",
                    "tool_name": "shell",
                    "success": true,
                    "content": "/workspace"
                })),
            },
        ])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve");

    let staged = resolver
        .staged_state
        .get("main")
        .map(|(tree, _)| tree)
        .expect("staged tree");
    let mut hollow = Vec::new();
    collect_hollow_message_turn_ids(staged, &mut hollow);
    assert_eq!(hollow, Vec::<String>::new());
}

#[test]
fn keyed_tool_call_pairs_are_protocol_nodes_not_managed_surfaces() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                Some("first".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_old",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_old",
                    "tool_name": "shell",
                    "parameters": {"cmd": "pwd"},
                    "key": "surface:shell"
                })),
            },
        ])
        .expect("ingest old call");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve old call");
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: PactOperationEnvelope {
                source: SourceActor::Host,
                ..env("main")
            },
            target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
            parent_id: None,
            depth: None,
            offset: None,
            component: component(json!({
                "id": "result_old",
                "componentType": "tool_result",
                "tool_call_id": "tc_old",
                "tool_name": "shell",
                "success": true,
                "content": "old"
            })),
        }])
        .expect("ingest old result");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve old result");

    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                Some("second".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_new",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_new",
                    "tool_name": "shell",
                    "parameters": {"cmd": "ls"},
                    "key": "surface:shell"
                })),
            },
        ])
        .expect("ingest new call");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve new call");

    let staged = resolver.preview("main").expect("preview");
    assert!(
        find_node_value(&staged, "call_old").is_some(),
        "tool calls are provider protocol obligations and must not be key-collapsed"
    );
    assert!(
        find_node_value(&staged, "result_old").is_some(),
        "paired tool results must survive with their call"
    );
    assert!(find_node_value(&staged, "call_new").is_some());
}

#[test]
fn same_response_keyed_tool_calls_all_survive_until_commit() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                Some("run both".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_00",
                    "componentType": "tool_call",
                    "type": "tool_call",
                    "tool_call_id": "tc_00",
                    "tool_name": "shell",
                    "parameters": {"cmd": "pwd"},
                    "key": "surface:shell"
                })),
            },
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_01",
                    "componentType": "tool_call",
                    "type": "tool_call",
                    "tool_call_id": "tc_01",
                    "tool_name": "shell",
                    "parameters": {"cmd": "ls"},
                    "key": "surface:shell"
                })),
            },
        ])
        .expect("ingest response calls");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Adhoc)
        .expect("resolve response calls");

    let staged = resolver.preview("main").expect("preview");
    assert!(find_node_value(&staged, "call_00").is_some());
    assert!(find_node_value(&staged, "call_01").is_some());
}

#[test]
fn keyed_tool_pairs_without_retention_do_not_collapse() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {
                    "id": "turn_0",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "assistant",
                    "children": [{
                        "id": "container_0",
                        "componentType": "message_container",
                        "type": "message_container",
                        "children": [
                            {
                                "id": "call_0",
                                "componentType": "tool_call",
                                "type": "tool_call",
                                "tool_call_id": "pair_0",
                                "tool_name": "file_reader",
                                "key": "tool:file_reader"
                            },
                            {
                                "id": "result_0",
                                "componentType": "tool_result",
                                "type": "tool_result",
                                "tool_call_id": "pair_0",
                                "tool_name": "file_reader",
                                "success": true,
                                "key": "tool:file_reader",
                                "content": "first"
                            }
                        ]
                    }]
                },
                {
                    "id": "turn_1",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "assistant",
                    "children": [{
                        "id": "container_1",
                        "componentType": "message_container",
                        "type": "message_container",
                        "children": [
                            {
                                "id": "call_1",
                                "componentType": "tool_call",
                                "type": "tool_call",
                                "tool_call_id": "pair_1",
                                "tool_name": "file_reader",
                                "key": "tool:file_reader"
                            },
                            {
                                "id": "result_1",
                                "componentType": "tool_result",
                                "type": "tool_result",
                                "tool_call_id": "pair_1",
                                "tool_name": "file_reader",
                                "success": true,
                                "key": "tool:file_reader",
                                "content": "second"
                            }
                        ]
                    }]
                }
            ]
        }"#,
    );

    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve keyed pairs without retention");

    let head = resolver
        .preview("main")
        .expect("preview after no-op retention");
    assert!(find_node_value(&head, "call_0").is_some());
    assert!(find_node_value(&head, "result_0").is_some());
    assert!(find_node_value(&head, "call_1").is_some());
    assert!(find_node_value(&head, "result_1").is_some());
    assert!(
        resolver
            .staged_state
            .get("main")
            .is_none_or(|(_, mutations)| {
                !mutations
                    .iter()
                    .any(|mutation| mutation.kind == MutationKind::Delete)
            }),
        "Pact key metadata alone must not imply retention=1"
    );
}

#[test]
fn tool_pair_lifecycle_syncs_from_call_to_result() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                Some("checking".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_sync",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_sync",
                    "tool_name": "shell",
                    "parameters": {},
                    "ttl": 3,
                    "cad": 1
                })),
            },
        ])
        .expect("ingest call");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve call");

    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: PactOperationEnvelope {
                source: SourceActor::Host,
                ..env("main")
            },
            target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
            parent_id: None,
            depth: None,
            offset: None,
            component: component(json!({
                "id": "result_sync",
                "componentType": "tool_result",
                "tool_call_id": "tc_sync",
                "tool_name": "shell",
                "success": true,
                "content": "ok"
            })),
        }])
        .expect("ingest result");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve result");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    let call = find_node_value(&staged, "call_sync").expect("call");
    let result = find_node_value(&staged, "result_sync").expect("result");
    assert_eq!(call.get("ttl").and_then(Value::as_i64), Some(3));
    assert_eq!(call.get("cad").and_then(Value::as_i64), Some(1));
    assert_eq!(result.get("ttl").and_then(Value::as_i64), Some(3));
    assert_eq!(result.get("cad").and_then(Value::as_i64), Some(1));
    assert!(
        mutations.iter().any(|mutation| {
            mutation.kind == MutationKind::Update
                && mutation.target.component_id.as_deref() == Some("result_sync")
        }),
        "result lifecycle sync must be persisted as a Pact mutation"
    );
}

#[test]
fn tool_pair_lifecycle_syncs_from_result_to_call() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                Some("checking".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_from_result",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_result_sync",
                    "tool_name": "notes",
                    "parameters": {}
                })),
            },
        ])
        .expect("ingest call");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve call");

    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: PactOperationEnvelope {
                source: SourceActor::Host,
                ..env("main")
            },
            target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
            parent_id: None,
            depth: None,
            offset: None,
            component: component(json!({
                "id": "result_with_lifecycle",
                "componentType": "tool_result",
                "tool_call_id": "tc_result_sync",
                "tool_name": "notes",
                "success": true,
                "content": "ok",
                "ttl": 2
            })),
        }])
        .expect("ingest result");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve result");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    let call = find_node_value(&staged, "call_from_result").expect("call");
    let result = find_node_value(&staged, "result_with_lifecycle").expect("result");
    assert_eq!(call.get("ttl").and_then(Value::as_i64), Some(2));
    assert_eq!(result.get("ttl").and_then(Value::as_i64), Some(2));
    assert!(
        mutations.iter().any(|mutation| {
            mutation.kind == MutationKind::Update
                && mutation.target.component_id.as_deref() == Some("call_from_result")
        }),
        "call lifecycle sync must be persisted as a Pact mutation"
    );
}

#[test]
fn tool_pair_lifecycle_conflicts_are_rejected() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                "assistant".to_string(),
                Some("checking".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_conflict",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_conflict",
                    "tool_name": "shell",
                    "parameters": {},
                    "ttl": 2
                })),
            },
        ])
        .expect("ingest call");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve call");

    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: PactOperationEnvelope {
                source: SourceActor::Host,
                ..env("main")
            },
            target_selector: Some(".message_turn{role='assistant'}:last".to_string()),
            parent_id: None,
            depth: None,
            offset: None,
            component: component(json!({
                "id": "result_conflict",
                "componentType": "tool_result",
                "tool_call_id": "tc_conflict",
                "tool_name": "shell",
                "success": true,
                "content": "ok",
                "ttl": 5
            })),
        }])
        .expect("ingest conflicting result");
    let err = resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect_err("conflicting pair ttl must reject the boundary");
    assert!(err.to_string().contains("conflicting ttl values"), "{err}");
}

#[test]
fn mutation_contract_insert_append_replace_and_delete_are_pact_operations() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "container",
                    "componentType": "message_turn",
                    "role": "assistant",
                    "children": []
                })),
            },
            PactOperation::Append {
                envelope: env("main"),
                target_component_id: Some("container".to_string()),
                target_selector: None,
                offset: None,
                component: component(json!({
                    "id": "child",
                    "componentType": "text_content",
                    "content": "draft"
                })),
            },
            PactOperation::Replace {
                envelope: env("main"),
                target_component_id: Some("child".to_string()),
                target_selector: None,
                component: component(json!({
                    "id": "child",
                    "componentType": "text_content",
                    "content": "final"
                })),
            },
        ])
        .expect("ingest insert append replace");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve insert append replace");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    assert_eq!(
        find_node_value(&staged, "child")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("final")
    );
    assert_eq!(
        mutations
            .iter()
            .filter(|mutation| matches!(mutation.kind, MutationKind::Insert | MutationKind::Update))
            .count(),
        3
    );

    resolver
        .ingest_operations(vec![PactOperation::Delete {
            envelope: env("main"),
            target_component_id: Some("container".to_string()),
            target_selector: None,
        }])
        .expect("ingest delete");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve delete");

    let (staged, mutations) = resolver
        .staged_state
        .get("main")
        .expect("staged state after delete");
    let staged = staged.project_value();
    assert!(find_node_value(&staged, "container").is_none());
    assert!(find_node_value(&staged, "child").is_none());
    assert!(
        mutations
            .iter()
            .any(|mutation| mutation.kind == MutationKind::Delete
                && mutation.target.component_id.as_deref() == Some("container"))
    );
}

#[test]
fn delete_component_replays_from_durable_transaction_trace() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "delete_parent",
                        "componentType": "message_turn",
                        "role": "assistant",
                        "children": [
                            {
                                "id": "delete_child",
                                "componentType": "text_content",
                                "content": "deleted child"
                            }
                        ]
                    },
                    {
                        "id": "keep_after_delete",
                        "componentType": "text_content",
                        "content": "still here"
                    }
                ]
            }"#,
    );
    let base_current = resolver.ledger.branches["main"].current.clone();

    resolver
        .delete(
            "main",
            Some("delete_parent".to_string()),
            None,
            BoundaryKind::Authored,
        )
        .expect("delete subtree");

    let mut replay_only_ledger = resolver.ledger().clone();
    let branch = replay_only_ledger
        .branches
        .get_mut("main")
        .expect("main branch");
    assert!(
        branch.transaction_traces.iter().any(|trace| {
            trace.ops.iter().any(|op| {
                op.component_id == "delete_parent"
                    && matches!(
                        op.kind,
                        crate::transaction_trace::TraceOpKind::DeleteComponent
                    )
            })
        }),
        "delete must be represented as durable trace replay data"
    );
    branch.current = base_current;

    let branch = replay_only_ledger
        .branches
        .get("main")
        .expect("main branch");
    let replayed =
        project_branch_component_tree(&replay_only_ledger, branch, Some(branch.head_revision))
            .expect("project delete from durable trace")
            .project_value();
    assert!(find_node_value(&replayed, "delete_parent").is_none());
    assert!(find_node_value(&replayed, "delete_child").is_none());
    assert!(find_node_value(&replayed, "keep_after_delete").is_some());
}

#[test]
fn upsert_inserts_missing_node_then_replaces_existing_node() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "system",
                "children": []
            }]
        }"#,
    );

    resolver
        .ingest_operations(vec![PactOperation::Upsert {
            envelope: env("main"),
            target_component_id: Some("file_manager".to_string()),
            target_selector: None,
            parent_id: Some("system".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "file_manager",
                "componentType": "file_manager",
                "type": "file_manager",
                "content": "[No working directory]"
            })),
        }])
        .expect("ingest initial upsert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve initial upsert");

    let inserted = resolver.preview("main").expect("preview inserted");
    assert_eq!(
        find_node_value(&inserted, "file_manager")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("[No working directory]")
    );

    resolver
        .ingest_operations(vec![PactOperation::Upsert {
            envelope: env("main"),
            target_component_id: Some("file_manager".to_string()),
            target_selector: None,
            parent_id: Some("system".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "file_manager",
                "componentType": "file_manager",
                "type": "file_manager",
                "content": "cwd=/workspace/pactree"
            })),
        }])
        .expect("ingest replacement upsert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve replacement upsert");

    let replaced = resolver.preview("main").expect("preview replaced");
    assert_eq!(count_node_id(&replaced, "file_manager"), 1);
    let file_manager = find_node_value(&replaced, "file_manager").expect("file manager node");
    assert_eq!(
        file_manager.get("parent_id").and_then(Value::as_str),
        Some("system")
    );
    assert_eq!(
        file_manager.get("content").and_then(Value::as_str),
        Some("cwd=/workspace/pactree")
    );
}

#[test]
fn serialized_operation_adapters_cover_canonical_ops() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "system",
                "children": []
            }]
        }"#,
    );

    let insert = resolver
        .insert_json(
            "main",
            None,
            Some("system".to_string()),
            None,
            None,
            json!({
                "id": "native_panel",
                "componentType": "file_manager",
                "type": "file_manager",
                "content": "panel"
            }),
            BoundaryKind::Authored,
        )
        .expect("insert");
    assert_eq!(insert["schema"], "pact.insert.v1");
    assert_eq!(
        find_node_value(&resolver.preview("main").expect("preview"), "native_panel")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("panel")
    );

    let append = resolver
        .append_json(
            "main",
            Some("native_panel".to_string()),
            None,
            None,
            json!({
                "id": "native_text",
                "componentType": "text_content",
                "type": "text_content",
                "content": "draft"
            }),
            BoundaryKind::Authored,
        )
        .expect("append");
    assert_eq!(append["schema"], "pact.append.v1");
    assert!(find_node_value(&resolver.preview("main").expect("preview"), "native_text").is_some());

    let update = resolver
        .update(
            "main",
            Some("native_text".to_string()),
            None,
            Some("updated".to_string()),
            None,
            Map::new(),
            Map::new(),
            BoundaryKind::Authored,
        )
        .expect("update");
    assert_eq!(update["schema"], "pact.update.v1");
    assert_eq!(
        find_node_value(&resolver.preview("main").expect("preview"), "native_text")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("updated")
    );

    let replace = resolver
        .replace_json(
            "main",
            Some("native_text".to_string()),
            None,
            json!({
                "id": "native_text",
                "componentType": "text_content",
                "type": "text_content",
                "content": "final"
            }),
            BoundaryKind::Authored,
        )
        .expect("replace");
    assert_eq!(replace["schema"], "pact.replace.v1");
    assert_eq!(replace["replaced_component_ids"][0], "native_text");

    let upsert_insert = resolver
        .upsert_json(
            "main",
            Some("native_status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            json!({
                "id": "native_status",
                "componentType": "text_content",
                "type": "text_content",
                "content": "first"
            }),
            BoundaryKind::Authored,
        )
        .expect("upsert insert");
    assert_eq!(upsert_insert["schema"], "pact.upsert.v1");
    assert_eq!(
        find_node_value(&resolver.preview("main").expect("preview"), "native_status")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("first")
    );

    let upsert_replace = resolver
        .upsert_json(
            "main",
            Some("native_status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            json!({
                "id": "native_status",
                "componentType": "text_content",
                "type": "text_content",
                "content": "second"
            }),
            BoundaryKind::Authored,
        )
        .expect("upsert replace");
    assert_eq!(upsert_replace["schema"], "pact.upsert.v1");
    assert_eq!(upsert_replace["replaced_component_ids"][0], "native_status");

    let delete = resolver
        .delete(
            "main",
            Some("native_text".to_string()),
            None,
            BoundaryKind::Authored,
        )
        .expect("delete");
    assert_eq!(delete["schema"], "pact.delete.v1");
    assert!(
        delete["deleted_component_ids"]
            .as_array()
            .expect("deleted ids")
            .iter()
            .any(|id| id == "native_text")
    );

    let preview = resolver.preview("main").expect("preview");
    assert!(find_node_value(&preview, "native_text").is_none());
    assert_eq!(
        find_node_value(&preview, "native_status")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("second")
    );
}

#[test]
fn typed_component_operation_methods_cover_authoring_ops() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "system",
                "children": []
            }]
        }"#,
    );

    let insert = resolver
        .insert_component(
            "main",
            None,
            Some("system".to_string()),
            None,
            None,
            component(json!({
                "id": "typed_panel",
                "componentType": "file_manager",
                "type": "file_manager",
                "content": "panel"
            })),
            BoundaryKind::Authored,
        )
        .expect("insert component");
    assert_eq!(insert["schema"], "pact.insert.v1");

    resolver
        .append_component(
            "main",
            Some("typed_panel".to_string()),
            None,
            None,
            component(json!({
                "id": "typed_text",
                "componentType": "text_content",
                "type": "text_content",
                "content": "draft"
            })),
            BoundaryKind::Authored,
        )
        .expect("append component");

    let replace = resolver
        .replace_component(
            "main",
            Some("typed_text".to_string()),
            None,
            component(json!({
                "id": "typed_text",
                "componentType": "text_content",
                "type": "text_content",
                "content": "final"
            })),
            BoundaryKind::Authored,
        )
        .expect("replace component");
    assert_eq!(replace["replaced_component_ids"][0], "typed_text");

    let upsert = resolver
        .upsert_component(
            "main",
            Some("typed_status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            component(json!({
                "id": "typed_status",
                "componentType": "text_content",
                "type": "text_content",
                "content": "status"
            })),
            BoundaryKind::Authored,
        )
        .expect("upsert component");
    assert_eq!(upsert["schema"], "pact.upsert.v1");

    let preview = resolver.preview("main").expect("preview");
    assert_eq!(
        find_node_value(&preview, "typed_text")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("final")
    );
    assert_eq!(
        find_node_value(&preview, "typed_status")
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("status")
    );
}

#[test]
fn typed_authoring_preserves_inspect_only_lifecycle_in_view_components() {
    let root = r#"{
        "id": "root",
        "componentType": "root",
        "children": []
    }"#;
    let mut resolver = resolver_with_tree(root);

    let inserted = lifecycle_component("typed_inserted", "typed_inserted", 11);
    let inserted_lifecycle = inserted.lifecycle.clone();
    resolver
        .insert_component(
            "main",
            None,
            Some("root".to_string()),
            None,
            None,
            inserted,
            BoundaryKind::Authored,
        )
        .expect("typed insert");
    assert_eq!(
        resolver
            .head()
            .expect("head")
            .component("typed_inserted")
            .expect("inserted")
            .lifecycle,
        inserted_lifecycle
    );

    let appended = lifecycle_component("typed_appended", "typed_appended", 12);
    let appended_lifecycle = appended.lifecycle.clone();
    resolver
        .append_component(
            "main",
            Some("root".to_string()),
            None,
            None,
            appended,
            BoundaryKind::Authored,
        )
        .expect("typed append");
    assert_eq!(
        resolver
            .head()
            .expect("head")
            .component("typed_appended")
            .expect("appended")
            .lifecycle,
        appended_lifecycle
    );

    let replacement = lifecycle_component("typed_inserted", "typed_replaced", 13);
    let replacement_lifecycle = replacement.lifecycle.clone();
    resolver
        .replace_component(
            "main",
            Some("typed_inserted".to_string()),
            None,
            replacement,
            BoundaryKind::Authored,
        )
        .expect("typed replace");
    let replaced = resolver
        .head()
        .expect("head")
        .component("typed_inserted")
        .expect("replaced");
    assert_eq!(replaced.lifecycle, replacement_lifecycle);
    assert!(
        replaced
            .project_component_value()
            .get("lifecycle")
            .and_then(Value::as_object)
            .and_then(|lifecycle| lifecycle.get("internal_refs"))
            .is_none(),
        "normal projection keeps internal refs inspect-only"
    );
    assert!(
        replaced
            .inspect_component_value()
            .get("lifecycle")
            .and_then(|lifecycle| lifecycle.get("internal_refs"))
            .is_some(),
        "inspect projection exposes internal refs"
    );

    let upsert_replacement = lifecycle_component("typed_appended", "typed_upsert_replace", 14);
    let upsert_replace_lifecycle = upsert_replacement.lifecycle.clone();
    resolver
        .upsert_component(
            "main",
            Some("typed_appended".to_string()),
            None,
            Some("root".to_string()),
            None,
            None,
            upsert_replacement,
            BoundaryKind::Authored,
        )
        .expect("typed upsert replace");
    assert_eq!(
        resolver
            .head()
            .expect("head")
            .component("typed_appended")
            .expect("upsert replaced")
            .lifecycle,
        upsert_replace_lifecycle
    );

    let upsert_insert = lifecycle_component("typed_upserted", "typed_upsert_insert", 15);
    let upsert_insert_lifecycle = upsert_insert.lifecycle.clone();
    resolver
        .upsert_component(
            "main",
            None,
            None,
            Some("root".to_string()),
            None,
            None,
            upsert_insert,
            BoundaryKind::Authored,
        )
        .expect("typed upsert insert");
    assert_eq!(
        resolver
            .head()
            .expect("head")
            .component("typed_upserted")
            .expect("upsert inserted")
            .lifecycle,
        upsert_insert_lifecycle
    );
}

#[test]
fn typed_staged_trace_replay_preserves_component_lifecycle() {
    let root = r#"{
        "id": "root",
        "componentType": "root",
        "children": []
    }"#;
    let mut resolver = resolver_with_tree(root);
    let first = lifecycle_component("first_typed", "first_typed", 21);
    let second = lifecycle_component("second_typed", "second_typed", 22);
    let second_lifecycle = second.lifecycle.clone();
    let first_op = PactOperation::Insert {
        envelope: PactOperationEnvelope {
            host_timestamp_ns: Some(101),
            ..env("main")
        },
        target_selector: None,
        parent_id: Some("root".to_string()),
        depth: None,
        offset: None,
        component: first,
    };
    let second_op = PactOperation::Insert {
        envelope: PactOperationEnvelope {
            host_timestamp_ns: Some(102),
            ..env("main")
        },
        target_selector: None,
        parent_id: Some("root".to_string()),
        depth: None,
        offset: None,
        component: second,
    };

    resolver
        .resolve(vec![first_op, second_op], "main", BoundaryKind::Authored)
        .expect("resolve");
    let first_trace_id = resolver
        .staged_traces("main")
        .first()
        .expect("first trace")
        .trace_id
        .clone();

    assert!(
        resolver
            .discard_staged_trace("main", &first_trace_id)
            .expect("discard first staged trace")
    );
    let (staged, _) = resolver.staged_state.get("main").expect("staged state");
    assert!(staged.component("first_typed").is_none());
    assert_eq!(
        staged
            .component("second_typed")
            .expect("replayed second component")
            .lifecycle,
        second_lifecycle
    );
}

#[test]
fn serialized_component_authoring_adapters_mint_missing_ids_before_parse() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "system",
                "children": []
            }]
        }"#,
    );

    let insert = resolver
        .insert_json(
            "main",
            None,
            Some("system".to_string()),
            None,
            None,
            json!({
                "componentType": "text_content",
                "type": "text_content",
                "content": "inserted without id"
            }),
            BoundaryKind::Authored,
        )
        .expect("insert without id");
    let inserted_id = insert["produced_component_ids"][0]
        .as_str()
        .expect("insert produced id")
        .to_string();
    assert!(inserted_id.starts_with("node_"));
    assert!(find_node_value(&resolver.preview("main").expect("preview"), &inserted_id).is_some());

    let append = resolver
        .append_json(
            "main",
            Some("system".to_string()),
            None,
            None,
            json!({
                "componentType": "text_content",
                "type": "text_content",
                "content": "appended without id"
            }),
            BoundaryKind::Authored,
        )
        .expect("append without id");
    let appended_id = append["produced_component_ids"][0]
        .as_str()
        .expect("append produced id")
        .to_string();
    assert!(appended_id.starts_with("node_"));

    let replace = resolver
        .replace_json(
            "main",
            Some(appended_id.clone()),
            None,
            json!({
                "componentType": "text_content",
                "type": "text_content",
                "content": "replacement without id"
            }),
            BoundaryKind::Authored,
        )
        .expect("replace without id");
    assert_eq!(replace["replaced_component_ids"][0], appended_id);
    assert_eq!(
        find_node_value(&resolver.preview("main").expect("preview"), &appended_id)
            .and_then(|node| node.get("content"))
            .and_then(Value::as_str),
        Some("replacement without id")
    );

    let upsert = resolver
        .upsert_json(
            "main",
            Some("status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            json!({
                "componentType": "text_content",
                "type": "text_content",
                "content": "upserted without id"
            }),
            BoundaryKind::Authored,
        )
        .expect("upsert without id");
    assert_eq!(upsert["produced_component_ids"][0], "status");
    assert!(find_node_value(&resolver.preview("main").expect("preview"), "status").is_some());
}

#[test]
fn typed_and_serialized_authoring_paths_have_behavior_parity() {
    let root = r#"{
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "system",
            "componentType": "system_header",
            "type": "system_header",
            "role": "system",
            "children": []
        }]
    }"#;
    let mut json_resolver = resolver_with_tree(root);
    let mut typed_resolver = resolver_with_tree(root);

    let panel = json!({
        "id": "panel",
        "componentType": "file_manager",
        "type": "file_manager",
        "content": "panel"
    });
    json_resolver
        .insert_json(
            "main",
            None,
            Some("system".to_string()),
            None,
            None,
            panel.clone(),
            BoundaryKind::Authored,
        )
        .expect("json insert");
    typed_resolver
        .insert_component(
            "main",
            None,
            Some("system".to_string()),
            None,
            None,
            component(panel),
            BoundaryKind::Authored,
        )
        .expect("typed insert");

    let child = json!({
        "id": "text",
        "componentType": "text_content",
        "type": "text_content",
        "content": "draft"
    });
    json_resolver
        .append_json(
            "main",
            Some("panel".to_string()),
            None,
            None,
            child.clone(),
            BoundaryKind::Authored,
        )
        .expect("json append");
    typed_resolver
        .append_component(
            "main",
            Some("panel".to_string()),
            None,
            None,
            component(child),
            BoundaryKind::Authored,
        )
        .expect("typed append");

    for resolver in [&mut json_resolver, &mut typed_resolver] {
        resolver
            .update(
                "main",
                Some("text".to_string()),
                None,
                Some("updated".to_string()),
                None,
                Map::new(),
                Map::new(),
                BoundaryKind::Authored,
            )
            .expect("update");
    }

    let replacement = json!({
        "id": "text",
        "componentType": "text_content",
        "type": "text_content",
        "content": "final"
    });
    json_resolver
        .replace_json(
            "main",
            Some("text".to_string()),
            None,
            replacement.clone(),
            BoundaryKind::Authored,
        )
        .expect("json replace");
    typed_resolver
        .replace_component(
            "main",
            Some("text".to_string()),
            None,
            component(replacement),
            BoundaryKind::Authored,
        )
        .expect("typed replace");

    let status_first = json!({
        "id": "status",
        "componentType": "text_content",
        "type": "text_content",
        "content": "first"
    });
    json_resolver
        .upsert_json(
            "main",
            Some("status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            status_first.clone(),
            BoundaryKind::Authored,
        )
        .expect("json upsert insert");
    typed_resolver
        .upsert_component(
            "main",
            Some("status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            component(status_first),
            BoundaryKind::Authored,
        )
        .expect("typed upsert insert");

    let status_second = json!({
        "id": "status",
        "componentType": "text_content",
        "type": "text_content",
        "content": "second"
    });
    json_resolver
        .upsert_json(
            "main",
            Some("status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            status_second.clone(),
            BoundaryKind::Authored,
        )
        .expect("json upsert replace");
    typed_resolver
        .upsert_component(
            "main",
            Some("status".to_string()),
            None,
            Some("system".to_string()),
            None,
            None,
            component(status_second),
            BoundaryKind::Authored,
        )
        .expect("typed upsert replace");

    for resolver in [&mut json_resolver, &mut typed_resolver] {
        resolver
            .delete(
                "main",
                Some("text".to_string()),
                None,
                BoundaryKind::Authored,
            )
            .expect("prune");
    }

    let mut json_preview = json_resolver.preview("main").expect("json preview");
    let mut typed_preview = typed_resolver.preview("main").expect("typed preview");
    normalize_created_at_ns(&mut json_preview);
    normalize_created_at_ns(&mut typed_preview);
    assert_eq!(json_preview, typed_preview);
}

fn normalize_created_at_ns(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("created_at_ns") {
                map.insert("created_at_ns".to_string(), serde_json::Value::from(0u64));
            }
            for child in map.values_mut() {
                normalize_created_at_ns(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                normalize_created_at_ns(child);
            }
        }
        _ => {}
    }
}

#[test]
fn update_operation_sets_canonical_fields_after_insert() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "memory_text",
                "componentType": "text_content",
                "content": "literal context",
                "attrs": {"tags": ["stale-attr-shadow"], "host_note": "kept"}
            })),
        }])
        .expect("ingest insert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve insert");

    let mut fields = Map::new();
    fields.insert("tags".to_string(), json!(["archived", "pager"]));
    fields.insert("ttl".to_string(), json!(7));
    fields.insert("key".to_string(), json!("memory-slot"));
    resolver
        .ingest_operations(vec![PactOperation::Update {
            envelope: env("main"),
            target_component_id: Some("memory_text".to_string()),
            target_selector: None,
            content: None,
            content_ref: None,
            fields,
            aux: Map::new(),
        }])
        .expect("ingest canonical field update");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve canonical field update");

    let (staged, _) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    let node = find_node_value(&staged, "memory_text").expect("memory text node");
    assert_eq!(node["tags"], json!(["archived", "pager"]));
    assert_eq!(node["ttl"], json!(7));
    assert_eq!(node["key"], json!("memory-slot"));
    assert!(node["aux"].get("tags").is_none());
    assert_eq!(node["aux"]["host_note"], json!("kept"));

    let selected = select_json(
        &serde_json::to_string(&staged).expect("staged json"),
        "+archived",
    )
    .expect("tag selector");
    let matches: Vec<Value> = serde_json::from_str(&selected).expect("selector json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["id"], "memory_text");
}

#[test]
fn deleting_only_tool_pair_prunes_empty_container_and_turn() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "turn_shell",
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "assistant",
                "children": [{
                    "id": "container_shell",
                    "componentType": "message_container",
                    "type": "message_container",
                    "children": [
                        {
                            "id": "call_shell",
                            "componentType": "tool_call",
                            "type": "tool_call",
                            "tool_call_id": "call_shell",
                            "tool_name": "shell_execute"
                        },
                        {
                            "id": "result_shell",
                            "componentType": "tool_result",
                            "type": "tool_result",
                            "tool_call_id": "call_shell",
                            "tool_name": "shell_execute",
                            "success": true,
                            "content": "ok"
                        }
                    ]
                }]
            }]
        }"#,
    );

    resolver
        .ingest_operations(vec![PactOperation::Delete {
            envelope: env("main"),
            target_component_id: Some("call_shell".to_string()),
            target_selector: None,
        }])
        .expect("ingest delete");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve delete");

    let (staged, mutations) = resolver
        .staged_state
        .get("main")
        .expect("staged state after delete");
    let staged = staged.project_value();
    assert!(
        find_node_value(&staged, "call_shell").is_none(),
        "deleting a tool call removes the call"
    );
    assert!(
        find_node_value(&staged, "result_shell").is_none(),
        "deleting a tool call removes its paired result"
    );
    assert!(
        find_node_value(&staged, "container_shell").is_none(),
        "the empty message container is removed after the tool pair is gone"
    );
    assert!(
        find_node_value(&staged, "turn_shell").is_none(),
        "the whole message turn is removed once no containers remain"
    );
    assert!(
        mutations
            .iter()
            .any(|mutation| mutation.kind == MutationKind::Delete
                && mutation.target.component_id.as_deref() == Some("call_shell"))
    );
    assert!(
        mutations
            .iter()
            .any(|mutation| mutation.kind == MutationKind::Delete
                && mutation.target.component_id.as_deref() == Some("container_shell"))
    );
    assert!(
        mutations
            .iter()
            .any(|mutation| mutation.kind == MutationKind::Delete
                && mutation.target.component_id.as_deref() == Some("turn_shell"))
    );
}

#[test]
fn preview_shows_resolved_state_without_advancing_head() {
    let mut resolver = fresh_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("preview only".to_string()),
            None,
        )])
        .expect("ingest");

    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    assert!(outcome.commit_candidate.is_some());

    let preview = resolver.preview("main").expect("preview");
    assert!(
        preview.to_string().contains("preview only"),
        "preview should include resolved uncommitted content: {preview}"
    );
    let post_preview_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    assert_eq!(
        post_preview_rev, initial_rev,
        "preview must not advance committed branch head"
    );
}

#[test]
fn preview_view_projects_staged_state_without_advancing_head() {
    let mut resolver = fresh_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("committed before staged view".to_string()),
            None,
        )])
        .expect("ingest committed");
    let committed = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve committed")
        .commit_candidate
        .expect("committed candidate");
    resolver.seal(&committed).expect("seal committed");
    let committed_rev = resolver.ledger.branches["main"].head_revision;
    assert_eq!(committed_rev, initial_rev + 1);
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("staged view".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let view = resolver.preview_view("main").expect("preview view");
    let post_preview_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    assert_eq!(
        post_preview_rev, committed_rev,
        "preview_view must not advance committed branch head"
    );
    assert_eq!(view.head_revision, committed_rev + 1);
    assert!(
        !view.message_order.is_empty(),
        "preview_view must assemble message_order for staged messages"
    );
    let committed_message_id = view
        .message_order
        .first()
        .expect("committed message id")
        .clone();
    let message_id = view.message_order.last().expect("message id");
    assert_eq!(
        view.tree.first_seen.get(&committed_message_id),
        Some(&committed_rev),
        "preview_view must preserve committed first_seen revisions"
    );
    assert_eq!(
        view.tree.first_seen.get(message_id),
        Some(&(committed_rev + 1)),
        "preview_view must mark staged components as first seen at the next revision"
    );
    assert_eq!(
        view.lifecycle_seen.get(message_id),
        Some(&(resolver.ledger.branches["main"].lifecycle_tick + 1))
    );
    assert!(
        view.component_records().values().any(|node| {
            node.node_type == "text_content" && node.content.as_deref() == Some("staged view")
        }),
        "preview_view must include staged content nodes"
    );
}

#[test]
fn preview_view_builds_staged_projection_once() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("single projection preview".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    crate::tests::instrumentation::reset_projection_build_count();
    let view = resolver.preview_view("main").expect("preview view");
    assert!(
        view.component_records().values().any(|node| {
            node.node_type == "text_content"
                && node.content.as_deref() == Some("single projection preview")
        }),
        "preview_view must include staged content"
    );
    assert_eq!(
        crate::tests::instrumentation::projection_build_count(),
        1,
        "preview_view should build the staged projection once, not probe and rebuild"
    );
}

#[test]
fn resolve_with_no_pending_returns_empty_outcome() {
    let mut resolver = fresh_resolver();
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Adhoc)
        .expect("resolve");
    assert!(outcome.applied.is_empty());
    assert!(outcome.commit_candidate.is_none());
}

#[test]
fn resolve_records_rejection_for_unknown_branch_op() {
    let mut resolver = fresh_resolver();

    resolver
        .ingest_operations(vec![message_op(
            env("ghost_branch"),
            "user".to_string(),
            Some("nowhere".to_string()),
            None,
        )])
        .expect("ingest");
    let err = resolver
        .resolve(Vec::new(), "ghost_branch", BoundaryKind::Authored)
        .unwrap_err();
    assert!(err.to_string().contains("branch not found"), "{err}");
}

#[test]
fn open_file_round_trips_committed_state() {
    use std::path::PathBuf;
    let tmp = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path: PathBuf = tmp.join(format!("pact_resolver_open_test_{nanos}.pact"));

    {
        let mut r = fresh_resolver();
        r.bind_file(path.clone());
        r.ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("persisted hello".to_string()),
            None,
        )])
        .expect("ingest");
        let outcome = r
            .resolve(Vec::new(), "main", BoundaryKind::Authored)
            .expect("resolve");
        r.seal(&outcome.commit_candidate.unwrap()).expect("seal");
    }

    let reopened = PactResolver::open_file(path.clone()).expect("open_file");
    let branch = reopened.ledger.branches.get("main").expect("main branch");
    assert!(branch.head_revision >= 1);
    assert!(
        !branch.transaction_traces.is_empty(),
        "file reopen must hydrate committed replay trace history"
    );
    let preview = reopened.preview("main").expect("reopened preview");
    assert!(
        preview.to_string().contains("persisted hello"),
        "reopened file must project committed content: {preview}"
    );

    std::fs::remove_file(path).ok();
}

#[test]
fn staged_state_accumulates_across_resolves() {
    let mut resolver = fresh_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("first staged".to_string()),
            None,
        )])
        .expect("ingest A");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve A");
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "assistant".to_string(),
            Some("second staged".to_string()),
            None,
        )])
        .expect("ingest B");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve B");

    let preview = resolver.preview("main").expect("preview");
    let preview_str = preview.to_string();
    assert!(
        preview_str.contains("first staged"),
        "preview must retain content from first staged resolve: {preview_str}"
    );
    assert!(
        preview_str.contains("second staged"),
        "preview must include content from second staged resolve: {preview_str}"
    );
    let head_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    assert_eq!(
        head_rev, initial_rev,
        "staged resolves must not advance branch head"
    );
}

#[test]
fn discard_staged_drops_uncommitted_work() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("to discard".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    assert!(resolver.has_staged("main"));

    let discarded = resolver.discard_staged("main");
    assert!(discarded, "discard should report removal");
    assert!(!resolver.has_staged("main"));

    let preview_after = resolver.preview("main").expect("preview");
    assert!(
        !preview_after.to_string().contains("to discard"),
        "preview after discard should fall back to committed head: {preview_after}"
    );

    assert!(!resolver.discard_staged("main"));
}

#[test]
fn discard_staged_trace_replays_remaining_staged_operations() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("first staged".to_string()),
            None,
        )])
        .expect("ingest first");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve first");
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("second staged".to_string()),
            None,
        )])
        .expect("ingest second");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve second");

    let traces = resolver.staged_traces("main");
    assert_eq!(traces.len(), 2);
    assert!(
        traces.iter().all(|trace| !trace.ops.is_empty()),
        "staged trace handles must carry trace-op metadata for inspection/seal"
    );
    let first_trace_id = traces[0].trace_id.clone();
    let second_trace_id = traces[1].trace_id.clone();

    assert!(
        resolver
            .discard_staged_trace("main", &first_trace_id)
            .expect("discard first trace")
    );

    let preview = resolver.preview("main").expect("preview");
    let preview_text = preview.to_string();
    assert!(
        !preview_text.contains("first staged"),
        "discarded trace must be removed from preview: {preview_text}"
    );
    assert!(
        preview_text.contains("second staged"),
        "remaining staged trace must be replayed into preview: {preview_text}"
    );

    let remaining = resolver.staged_traces("main");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].trace_id, second_trace_id);
    assert!(resolver.has_staged("main"));
}

#[test]
fn discard_staged_traces_by_idempotency_key() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            PactOperationEnvelope {
                idempotency_key: Some("drop_key".to_string()),
                ..env("main")
            },
            "user".to_string(),
            Some("drop keyed staged".to_string()),
            None,
        )])
        .expect("ingest drop");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve drop");
    resolver
        .ingest_operations(vec![message_op(
            PactOperationEnvelope {
                idempotency_key: Some("keep_key".to_string()),
                ..env("main")
            },
            "user".to_string(),
            Some("keep keyed staged".to_string()),
            None,
        )])
        .expect("ingest keep");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve keep");

    let removed = resolver
        .discard_staged_traces(
            "main",
            StagedTraceTarget::IdempotencyKey("drop_key".to_string()),
        )
        .expect("discard by key");
    assert_eq!(removed, 1);

    let preview = resolver.preview("main").expect("preview").to_string();
    assert!(!preview.contains("drop keyed staged"));
    assert!(preview.contains("keep keyed staged"));
    let remaining = resolver.staged_traces("main");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].idempotency_key.as_deref(), Some("keep_key"));
}

#[test]
fn discard_staged_traces_by_operation_kind() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "kind_target",
                "componentType": "text_content",
                "content": "before update"
            })),
        }])
        .expect("ingest insert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve insert");
    resolver
        .ingest_operations(vec![PactOperation::Update {
            envelope: env("main"),
            target_component_id: Some("kind_target".to_string()),
            target_selector: None,
            content: Some("after update".to_string()),
            content_ref: None,
            fields: Map::new(),
            aux: Map::new(),
        }])
        .expect("ingest update");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve update");

    let removed = resolver
        .discard_staged_traces(
            "main",
            StagedTraceTarget::OperationKind("update".to_string()),
        )
        .expect("discard update");
    assert_eq!(removed, 1);

    let preview = resolver.preview("main").expect("preview").to_string();
    assert!(preview.contains("before update"));
    assert!(!preview.contains("after update"));
    let remaining = resolver.staged_traces("main");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].op_kind, "insert");
}

#[test]
fn discard_staged_traces_by_component_id() {
    let mut resolver = fresh_resolver();
    for (id, content) in [
        ("drop_component", "drop component staged"),
        ("keep_component", "keep component staged"),
    ] {
        resolver
            .ingest_operations(vec![PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": id,
                    "componentType": "text_content",
                    "content": content
                })),
            }])
            .expect("ingest component");
        resolver
            .resolve(Vec::new(), "main", BoundaryKind::Authored)
            .expect("resolve component");
    }

    let removed = resolver
        .discard_staged_traces(
            "main",
            StagedTraceTarget::ComponentId("drop_component".to_string()),
        )
        .expect("discard component");
    assert_eq!(removed, 1);

    let preview = resolver.preview("main").expect("preview").to_string();
    assert!(!preview.contains("drop component staged"));
    assert!(preview.contains("keep component staged"));
    assert_eq!(resolver.staged_traces("main").len(), 1);
}

#[test]
fn discard_staged_traces_rejects_selector_target_until_index_backed() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("selector discard stays staged".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let err = resolver
        .discard_staged_traces("main", StagedTraceTarget::Selector("#whatever".to_string()))
        .expect_err("selector discard must be explicit unsupported");
    assert!(err.to_string().contains("not supported yet"));
    assert!(
        resolver
            .preview("main")
            .expect("preview")
            .to_string()
            .contains("selector discard stays staged")
    );
    assert_eq!(resolver.staged_traces("main").len(), 1);
}

#[test]
fn discard_unknown_staged_trace_is_noop() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("keep me".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    assert!(
        !resolver
            .discard_staged_trace("main", "trace_missing")
            .expect("unknown discard")
    );
    assert!(
        resolver
            .preview("main")
            .unwrap()
            .to_string()
            .contains("keep me")
    );
    assert_eq!(resolver.staged_traces("main").len(), 1);
}

#[test]
fn discard_staged_trace_rebuilds_pending_external_requirements() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: PactOperationEnvelope {
                source: SourceActor::Assistant,
                ..env("main")
            },
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "call_to_discard",
                "componentType": "tool_call",
                "tool_call_id": "tc_discard",
                "tool_name": "shell",
                "parameters": {"cmd": "pwd"}
            })),
        }])
        .expect("ingest tool call");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve tool call");
    assert_eq!(resolver.pending_external("main").len(), 1);
    let trace_id = resolver.staged_traces("main")[0].trace_id.clone();

    assert!(
        resolver
            .discard_staged_trace("main", &trace_id)
            .expect("discard tool call")
    );

    assert!(resolver.pending_external("main").is_empty());
    assert!(resolver.staged_traces("main").is_empty());
    assert!(
        !resolver
            .preview("main")
            .expect("preview")
            .to_string()
            .contains("tc_discard")
    );
}

#[test]
fn resolver_externalizes_large_node_payloads_before_apply() {
    let mut resolver = fresh_resolver();
    let body = "x".repeat(DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES + 1);
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "large_result",
                "componentType": "tool_result",
                "type": "tool_result",
                "tool_call_id": "tc_large",
                "content": body,
                "attrs": {"success": true},
                "org": {"text": body}
            })),
        }])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let hot_tree = resolver
        .staged_state
        .get("main")
        .map(|(tree, _)| tree)
        .expect("resolved staged state");
    let hot_tree = hot_tree.project_value();
    let hot_node = find_node_value(&hot_tree, "large_result").expect("hot large node");
    assert!(hot_node.get("content").is_none());
    let content_ref = hot_node
        .get("content_ref")
        .and_then(Value::as_object)
        .expect("content ref");
    let ref_id = content_ref.get("id").and_then(Value::as_str).unwrap();
    assert!(
        hot_node
            .get("org")
            .and_then(|org| org.get("text"))
            .is_none()
    );
    assert_eq!(
        resolver
            .ledger
            .content
            .records
            .get(ref_id)
            .and_then(|record| record.storage.inline.as_deref()),
        Some(body.as_str())
    );

    let preview = resolver.preview("main").expect("preview");
    let node = find_node_value(&preview, "large_result").expect("large node");
    assert_eq!(
        node.get("content").and_then(Value::as_str),
        Some(body.as_str())
    );
    assert!(node.get("content_ref").is_none());
}

#[test]
fn resolver_commit_reexternalizes_projected_current_head_content_refs() {
    let mut resolver = fresh_resolver();
    let body = "x".repeat(DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES + 32);
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some(body.clone()),
            None,
        )])
        .expect("ingest");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let boundary = outcome.commit_candidate.expect("commit candidate");
    resolver.seal(&boundary).expect("seal");

    let branch = resolver.ledger.branches.get("main").expect("main branch");
    let records = branch.current.component_records();
    let large_node = records
        .values()
        .find(|node| node.node_type == "text_content")
        .expect("large text node");
    let content_ref = large_node.content_ref.as_deref().expect("content ref");

    assert!(
        content_ref.starts_with("cnt_"),
        "committed head must use durable content refs, got {content_ref}"
    );
    assert!(!content_ref.starts_with("blob_"));
    assert!(resolver.ledger.content.records.contains_key(content_ref));

    let thread = resolver
        .build_provider_thread(
            "main",
            None,
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("provider thread");
    let rendered = serde_json::to_string(&thread).expect("thread json");
    assert!(rendered.contains(&body));
    assert!(!rendered.contains("blob_"));
}

#[test]
fn keyed_insert_collapses_prior_sibling_by_generic_pact_key() {
    let root = r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {
                    "id": "old_surface",
                    "componentType": "text_content",
                    "type": "text_content",
                    "key": "managed.surface",
                    "tags": ["pact:managed"],
                    "content": "old managed"
                },
                {
                    "id": "same_key_unmanaged",
                    "componentType": "text_content",
                    "type": "text_content",
                    "key": "managed.surface",
                    "content": "same key slot"
                },
                {
                    "id": "other_surface",
                    "componentType": "text_content",
                    "type": "text_content",
                    "key": "other.surface",
                    "tags": ["pact:managed"],
                    "content": "other managed slot"
                }
            ]
        }"#;
    let mut resolver = resolver_with_tree(root);
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "new_surface",
                "componentType": "text_content",
                "type": "text_content",
                "key": "managed.surface",
                "tags": ["pact:managed"],
                "content": "new managed"
            })),
        }])
        .expect("ingest");

    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    assert!(
        find_node_value(&staged, "old_surface").is_none(),
        "prior managed sibling in the same key slot should be removed from the current head"
    );
    assert!(find_node_value(&staged, "new_surface").is_some());
    assert!(find_node_value(&staged, "same_key_unmanaged").is_none());
    assert!(find_node_value(&staged, "other_surface").is_some());

    assert_eq!(mutations.len(), 3);
    assert_eq!(mutations[0].kind, MutationKind::Delete);
    assert_eq!(
        mutations[0].target.component_id.as_deref(),
        Some("old_surface")
    );
    assert_eq!(mutations[1].kind, MutationKind::Delete);
    assert_eq!(
        mutations[1].target.component_id.as_deref(),
        Some("same_key_unmanaged")
    );
    assert_eq!(mutations[2].kind, MutationKind::Insert);

    let branch = resolver.ledger.branches.get("main").unwrap();
    let (commit, _) = commit_record_from_mutations(
        "main",
        branch.head_revision + 1,
        mutations,
        &staged,
        &mut crate::store::ContentStore::default(),
    )
    .expect("commit record");
    assert!(matches!(
        commit.ops.first(),
        Some(CommitRecordOp::DeleteComponent { component_id }) if component_id == "old_surface"
    ));
    assert!(matches!(
        commit.ops.get(2),
        Some(CommitRecordOp::InsertComponent { node_ref, .. }) if node_ref == "new_surface"
    ));
}

#[test]
fn commit_insert_with_selector_uses_resolved_parent_id() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "msg_tool_results",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "user"
                })),
            },
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: Some("#msg_tool_results".to_string()),
                parent_id: None,
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "result_by_selector",
                    "componentType": "tool_result",
                    "type": "tool_result",
                    "tool_call_id": "call_selector_parent",
                    "tool_name": "shell_execute",
                    "success": true,
                    "content": "ok"
                })),
            },
        ])
        .expect("ingest");

    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let commit_candidate = outcome.commit_candidate.expect("commit candidate");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    assert!(find_node_value(&staged, "result_by_selector").is_some());
    let branch = resolver.ledger.branches.get("main").unwrap();
    let (commit, _) = commit_record_from_mutations(
        "main",
        branch.head_revision + 1,
        mutations,
        &staged,
        &mut crate::store::ContentStore::default(),
    )
    .expect("commit record");
    assert!(matches!(
        commit.ops.get(1),
        Some(CommitRecordOp::InsertComponent { parent_id, node_ref, .. })
            if parent_id == "msg_tool_results" && node_ref == "result_by_selector"
    ));

    let commit_ref = resolver.seal(&commit_candidate).expect("seal");
    assert_eq!(commit_ref.revision, 1);
}

#[test]
fn commit_delete_with_selector_uses_resolved_node_id() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "stale_tool_row",
                    "componentType": "tool_result",
                    "type": "tool_result",
                    "tool_call_id": "call_selector_delete",
                    "tool_name": "shell_execute",
                    "success": true,
                    "content": "old"
                })),
            },
            PactOperation::Delete {
                envelope: env("main"),
                target_component_id: None,
                target_selector: Some("#stale_tool_row".to_string()),
            },
        ])
        .expect("ingest");

    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let commit_candidate = outcome.commit_candidate.expect("commit candidate");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    assert!(find_node_value(&staged, "stale_tool_row").is_none());
    assert_eq!(mutations.len(), 2);
    assert_eq!(mutations[1].kind, MutationKind::Delete);
    assert_eq!(
        mutations[1].target.component_id.as_deref(),
        Some("stale_tool_row")
    );

    let branch = resolver.ledger.branches.get("main").unwrap();
    let (commit, _) = commit_record_from_mutations(
        "main",
        branch.head_revision + 1,
        mutations,
        &staged,
        &mut crate::store::ContentStore::default(),
    )
    .expect("commit record");
    assert!(matches!(
        commit.ops.get(1),
        Some(CommitRecordOp::DeleteComponent { component_id }) if component_id == "stale_tool_row"
    ));

    let commit_ref = resolver.seal(&commit_candidate).expect("seal");
    assert_eq!(commit_ref.revision, 1);
}

#[test]
fn commit_delete_with_tool_pair_selector_records_single_concrete_delete() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_by_selector",
                    "componentType": "tool_call",
                    "type": "tool_call",
                    "tool_call_id": "call_selector_pair",
                    "tool_name": "shell_execute",
                    "parameters": {"command": "pwd"}
                })),
            },
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "result_by_selector",
                    "componentType": "tool_result",
                    "type": "tool_result",
                    "tool_call_id": "call_selector_pair",
                    "tool_name": "shell_execute",
                    "success": true,
                    "content": "ok"
                })),
            },
            PactOperation::Delete {
                envelope: env("main"),
                target_component_id: None,
                target_selector: Some("{tool_call_id=call_selector_pair}".to_string()),
            },
        ])
        .expect("ingest");

    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");
    let commit_candidate = outcome.commit_candidate.expect("commit candidate");

    let (staged, mutations) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    assert!(find_node_value(&staged, "call_by_selector").is_none());
    assert!(find_node_value(&staged, "result_by_selector").is_none());
    assert_eq!(mutations.len(), 3);
    assert_eq!(mutations[2].kind, MutationKind::Delete);
    assert_eq!(
        mutations[2].target.component_id.as_deref(),
        Some("call_by_selector")
    );

    let branch = resolver.ledger.branches.get("main").unwrap();
    let (commit, _) = commit_record_from_mutations(
        "main",
        branch.head_revision + 1,
        mutations,
        &staged,
        &mut crate::store::ContentStore::default(),
    )
    .expect("commit record");
    assert_eq!(commit.ops.len(), 3);
    assert!(matches!(
        commit.ops.get(2),
        Some(CommitRecordOp::DeleteComponent { component_id }) if component_id == "call_by_selector"
    ));

    let commit_ref = resolver.seal(&commit_candidate).expect("seal");
    assert_eq!(commit_ref.revision, 1);
}

#[test]
fn inserting_top_level_turn_shifts_existing_turn_depths_and_sets_new_d0() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![
            message_op(
                env("main"),
                "assistant".to_string(),
                Some("I will inspect it.".to_string()),
                None,
            ),
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "result_like_turn",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "user",
                    "children": [{
                        "id": "result_child",
                        "componentType": "tool_result",
                        "type": "tool_result",
                        "tool_call_id": "call_depth",
                        "tool_name": "shell_execute",
                        "success": true,
                        "content": "ok"
                    }]
                })),
            },
        ])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let (staged, _) = resolver.staged_state.get("main").expect("staged state");
    let staged = staged.project_value();
    let children = staged
        .get("children")
        .and_then(Value::as_array)
        .expect("root children");
    let mut depths: Vec<_> = children
        .iter()
        .filter(|child| child.get("componentType").and_then(Value::as_str) == Some("message_turn"))
        .map(|child| {
            (
                child
                    .get("children")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|item| {
                        item.get("content")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("componentType").and_then(Value::as_str))
                    })
                    .unwrap_or(""),
                child
                    .get("pact_depth")
                    .and_then(Value::as_i64)
                    .unwrap_or(-99),
            )
        })
        .collect();
    depths.sort_by_key(|(_label, depth)| *depth);
    assert_eq!(depths, vec![("ok", 0), ("I will inspect it.", 1)]);
}

#[test]
fn preview_hydrates_large_tool_parameters_from_pact_content_store() {
    let mut resolver = fresh_resolver();
    let body = "tool parameter ".repeat(120);
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "large_call",
                "componentType": "tool_call",
                "type": "tool_call",
                "tool_call_id": "tc_large",
                "tool_name": "tool_update",
                "parameters": {"content": body}
            })),
        }])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let hot_tree = resolver
        .staged_state
        .get("main")
        .map(|(tree, _)| tree)
        .expect("resolved staged state");
    let hot_tree = hot_tree.project_value();
    let hot_node = find_node_value(&hot_tree, "large_call").expect("hot tool call");
    assert!(hot_node.get("parameters").is_none());
    assert!(hot_node.get("parameters_ref").is_some());

    let preview = resolver.preview("main").expect("preview");
    let node = find_node_value(&preview, "large_call").expect("preview tool call");
    assert_eq!(
        node.get("parameters")
            .and_then(|parameters| parameters.get("content"))
            .and_then(Value::as_str),
        Some(body.as_str())
    );
    assert!(node.get("parameters_ref").is_none());
}

#[test]
fn seal_staged_advances_head_after_multiple_staged_resolves() {
    let mut resolver = fresh_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("staged A".to_string()),
            None,
        )])
        .expect("ingest A");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve A");
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "assistant".to_string(),
            Some("staged B".to_string()),
            None,
        )])
        .expect("ingest B");

    let commit_ref = resolver
        .seal_staged("main", BoundaryKind::Request)
        .expect("seal_staged")
        .expect("commit_ref present");
    let post_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    assert!(
        post_rev > initial_rev,
        "seal_staged must advance branch head"
    );
    assert_eq!(commit_ref.after_ref.revision, Some(post_rev));
    assert!(
        !resolver.has_staged("main"),
        "seal_staged must drain the staged state"
    );
}

#[test]
fn seal_staged_with_no_work_is_noop() {
    let mut resolver = fresh_resolver();
    let result = resolver
        .seal_staged("main", BoundaryKind::Request)
        .expect("seal_staged no-op");
    assert!(result.is_none());
}

#[test]
fn pact_lifecycle_expiry_updates_head() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "notice",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "expires",
                        "ttl": 1
                    },
                    {
                        "id": "permanent",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "stays"
                    }
                ]
            }"#,
    );

    let no_tick = resolver
        .seal_staged("main", BoundaryKind::Request)
        .expect("ordinary commit");
    assert!(no_tick.is_none());
    assert!(
        find_node_value(&resolver.preview("main").expect("head"), "notice").is_some(),
        "ordinary commits must not age TTL"
    );

    let commit = resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("commit lifecycle")
        .expect("lifecycle commit");
    assert_eq!(commit.revision, 1);

    let head = resolver.preview("main").expect("head");
    assert!(find_node_value(&head, "notice").is_none());
    assert!(find_node_value(&head, "permanent").is_some());
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().lifecycle_tick,
        1
    );
}

#[test]
fn read_only_views_do_not_advance_lifecycle_tick() {
    let resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [{
                    "id": "turn_read",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "user",
                    "children": [{
                        "id": "container_read",
                        "componentType": "message_container",
                        "type": "message_container",
                        "children": [{
                            "id": "read_once",
                            "componentType": "text_content",
                            "type": "text_content",
                            "content": "read-only should not age me",
                            "ttl": 1
                        }]
                    }]
                }]
            }"#,
    );
    let branch = resolver.ledger.branches.get("main").unwrap();
    let initial_tick = branch.lifecycle_tick;
    let initial_revision = branch.head_revision;

    let preview = resolver.preview("main").expect("preview");
    assert!(find_node_value(&preview, "read_once").is_some());
    let view = resolver.preview_view("main").expect("preview view");
    assert!(view.tree.component_records().contains_key("read_once"));
    let selected = resolver
        .select("main", ".text_content", false)
        .expect("select");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "read_once");
    let thread = resolver
        .build_provider_thread(
            "main",
            None,
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("provider thread");
    assert!(thread.to_string().contains("read-only should not age me"));

    let branch = resolver.ledger.branches.get("main").unwrap();
    assert_eq!(branch.lifecycle_tick, initial_tick);
    assert_eq!(branch.head_revision, initial_revision);
}

#[test]
fn pact_lifecycle_ttl_two_survives_one_response_then_leaves_context() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "short_lived",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "visible for two response cycles",
                        "ttl": 2
                    },
                    {
                        "id": "stable",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "still here"
                    }
                ]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("first response")
        .expect("first lifecycle commit");
    let head = resolver.preview("main").expect("head after one response");
    assert!(
        find_node_value(&head, "short_lived").is_some(),
        "ttl=2 node must remain provider-visible after one full response cycle"
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("second response")
        .expect("second lifecycle commit");
    let head = resolver.preview("main").expect("head after two responses");
    assert!(
        find_node_value(&head, "short_lived").is_none(),
        "expired ttl=2 node must be absent from the actual Pact head"
    );
    assert!(find_node_value(&head, "stable").is_some());
}

#[test]
fn pact_lifecycle_multiple_ttl_windows_are_exact() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "ttl_one",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "one",
                        "ttl": 1
                    },
                    {
                        "id": "ttl_two",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "two",
                        "ttl": 2
                    },
                    {
                        "id": "ttl_three",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "three",
                        "ttl": 3
                    }
                ]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("tick one")
        .expect("tick one commit");
    let head = resolver.preview("main").expect("head at tick one");
    assert!(find_node_value(&head, "ttl_one").is_none());
    assert!(find_node_value(&head, "ttl_two").is_some());
    assert!(find_node_value(&head, "ttl_three").is_some());

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("tick two")
        .expect("tick two commit");
    let head = resolver.preview("main").expect("head at tick two");
    assert!(find_node_value(&head, "ttl_one").is_none());
    assert!(find_node_value(&head, "ttl_two").is_none());
    assert!(find_node_value(&head, "ttl_three").is_some());

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("tick three")
        .expect("tick three commit");
    let head = resolver.preview("main").expect("head at tick three");
    assert!(find_node_value(&head, "ttl_one").is_none());
    assert!(find_node_value(&head, "ttl_two").is_none());
    assert!(find_node_value(&head, "ttl_three").is_none());
}

#[test]
fn pact_lifecycle_expiry_prunes_empty_context_containers() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [{
                    "id": "turn_ephemeral",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "assistant",
                    "children": [{
                        "id": "container_ephemeral",
                        "componentType": "message_container",
                        "type": "message_container",
                        "children": [{
                            "id": "ephemeral_text",
                            "componentType": "text_content",
                            "type": "text_content",
                            "content": "goes away",
                            "ttl": 1
                        }]
                    }]
                }]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("response")
        .expect("lifecycle commit");
    let head = resolver.preview("main").expect("head");
    assert!(find_node_value(&head, "ephemeral_text").is_none());
    assert!(
        find_node_value(&head, "container_ephemeral").is_none(),
        "empty container left by ttl expiry must not stay in provider context"
    );
    assert!(
        find_node_value(&head, "turn_ephemeral").is_none(),
        "empty message turn left by ttl expiry must not stay in provider context"
    );
}

#[test]
fn pact_lifecycle_new_nodes_get_a_full_ttl_window() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "born_this_response",
                "componentType": "text_content",
                "type": "text_content",
                "content": "born during the response boundary",
                "ttl": 1
            })),
        }])
        .expect("ingest ttl node");

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("birth response")
        .expect("birth commit");
    let head = resolver.preview("main").expect("head after birth");
    assert!(
        find_node_value(&head, "born_this_response").is_some(),
        "a node born at this response boundary must not expire before it has lived one full response cycle"
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("expiry response")
        .expect("expiry commit");
    let head = resolver.preview("main").expect("head after expiry");
    assert!(
        find_node_value(&head, "born_this_response").is_none(),
        "ttl=1 node must leave the actual Pact head on the next response cycle"
    );
}

#[test]
fn pact_cadence_resurfaces_nodes_without_python_registry() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "sticky",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "remember",
                        "ttl": 1,
                        "cad": 1,
                        "key": "sticky.notice"
                    }
                ]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("commit lifecycle")
        .expect("lifecycle commit");

    let head = resolver.preview("main").expect("head");
    assert!(find_node_value(&head, "sticky").is_none());
    let resurfaced = find_node_value(&head, "sticky@r1").expect("resurfaced node");
    assert_eq!(resurfaced["content"], "remember");
    assert_eq!(resurfaced["ttl"], 1);
    assert_eq!(resurfaced["cad"], 1);
    assert_eq!(resurfaced["key"], "sticky.notice");
}

#[test]
fn lifecycle_ttl_and_cadence_replay_from_durable_transaction_trace() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "sticky_trace",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "remember through trace replay",
                        "ttl": 1,
                        "cad": 1,
                        "key": "sticky.trace"
                    },
                    {
                        "id": "stable_trace",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "still here"
                    }
                ]
            }"#,
    );
    let base_current = resolver.ledger.branches["main"].current.clone();

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("commit lifecycle")
        .expect("lifecycle commit");

    let mut replay_only_ledger = resolver.ledger().clone();
    let branch = replay_only_ledger
        .branches
        .get_mut("main")
        .expect("main branch");
    assert!(
        !branch.transaction_traces.is_empty(),
        "lifecycle commit must leave replayable trace history"
    );
    branch.current = base_current;

    let branch = replay_only_ledger
        .branches
        .get("main")
        .expect("main branch");
    let replayed =
        project_branch_component_tree(&replay_only_ledger, branch, Some(branch.head_revision))
            .expect("project lifecycle from durable trace")
            .project_value();
    assert!(
        find_node_value(&replayed, "sticky_trace").is_none(),
        "expired original component must be removed by trace replay: {replayed}"
    );
    let resurfaced =
        find_node_value(&replayed, "sticky_trace@r1").expect("resurfaced cadence component");
    assert_eq!(
        resurfaced.get("content").and_then(Value::as_str),
        Some("remember through trace replay")
    );
    assert!(find_node_value(&replayed, "stable_trace").is_some());
}

#[test]
fn pact_cadence_resurface_preserves_nested_placement() {
    let mut resolver = resolver_with_tree(
        r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "turn_1",
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "assistant",
                "pact_depth": 1,
                "children": [
                    {"id": "before", "componentType": "text_content", "type": "text_content", "offset": 0, "pact_depth": 1, "content": "before"},
                    {"id": "sticky", "componentType": "text_content", "type": "text_content", "offset": 1, "pact_depth": 1, "content": "remember", "ttl": 1, "cad": 1, "key": "sticky.notice"},
                    {"id": "after", "componentType": "text_content", "type": "text_content", "offset": 2, "pact_depth": 1, "content": "after"}
                ]
            }]
        }"#,
    );

    let head0 = resolver.preview("main").expect("head0");
    let sticky0 = find_node_value(&head0, "sticky").expect("sticky present pre-expiry");
    let orig_offset = sticky0["offset"].clone();
    let orig_depth = sticky0["pact_depth"].clone();

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("commit lifecycle")
        .expect("lifecycle commit");

    let head = resolver.preview("main").expect("head");
    assert!(
        find_node_value(&head, "sticky").is_none(),
        "expired sticky must be gone"
    );
    let resurfaced = find_node_value(&head, "sticky@r1").expect("resurfaced node");

    assert_eq!(resurfaced["parent_id"], "turn_1", "parent preserved");
    assert_eq!(resurfaced["offset"], orig_offset, "offset preserved");
    assert_eq!(resurfaced["pact_depth"], orig_depth, "pact_depth preserved");

    assert_eq!(resurfaced["content"], "remember");
    assert_eq!(resurfaced["ttl"], 1);
    assert_eq!(resurfaced["cad"], 1);
    assert_eq!(resurfaced["key"], "sticky.notice");

    let turn = find_node_value(&head, "turn_1").expect("turn_1");
    let order: Vec<&str> = turn["children"]
        .as_array()
        .expect("turn_1 children")
        .iter()
        .map(|child| child["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        order,
        vec!["before", "sticky@r1", "after"],
        "sibling order preserved"
    );
}

#[test]
fn pact_lifecycle_expires_tool_pair_with_one_delete_mutation() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [
                    {
                        "id": "call_ttl",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "call_pair_ttl",
                        "tool_name": "shell",
                        "parameters": {},
                        "ttl": 1
                    },
                    {
                        "id": "result_ttl",
                        "componentType": "tool_result",
                        "type": "tool_result",
                        "tool_call_id": "call_pair_ttl",
                        "tool_name": "shell",
                        "success": true,
                        "content": "ok",
                        "ttl": 1
                    }
                ]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("lifecycle commit must delete the pair once")
        .expect("commit ref");

    let head = resolver.preview("main").expect("head");
    assert!(find_node_value(&head, "call_ttl").is_none());
    assert!(find_node_value(&head, "result_ttl").is_none());
}

#[test]
fn pact_lifecycle_ttl_two_tool_pair_leaves_provider_context_after_two_responses() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [{
                    "id": "turn_pair_ttl",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "assistant",
                    "children": [{
                        "id": "container_pair_ttl",
                        "componentType": "message_container",
                        "type": "message_container",
                        "children": [
                            {
                                "id": "call_pair_ttl2",
                                "componentType": "tool_call",
                                "type": "tool_call",
                                "tool_call_id": "call_pair_ttl2",
                                "tool_name": "shell",
                                "parameters": {},
                                "ttl": 2
                            },
                            {
                                "id": "result_pair_ttl2",
                                "componentType": "tool_result",
                                "type": "tool_result",
                                "tool_call_id": "call_pair_ttl2",
                                "tool_name": "shell",
                                "success": true,
                                "content": "ttl-pair-output-visible",
                                "ttl": 2
                            }
                        ]
                    }]
                }]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("first response")
        .expect("first lifecycle commit");
    let head = resolver.preview("main").expect("head after one response");
    assert!(find_node_value(&head, "call_pair_ttl2").is_some());
    assert!(find_node_value(&head, "result_pair_ttl2").is_some());

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("second response")
        .expect("second lifecycle commit");
    let head = resolver.preview("main").expect("head after two responses");
    assert!(
        find_node_value(&head, "call_pair_ttl2").is_none(),
        "expired tool call half must be absent from the actual Pact head"
    );
    assert!(
        find_node_value(&head, "result_pair_ttl2").is_none(),
        "expired tool result half must be absent from the actual Pact head"
    );
    assert!(find_node_value(&head, "container_pair_ttl").is_none());
    assert!(find_node_value(&head, "turn_pair_ttl").is_none());

    let provider = resolver
        .build_provider_thread(
            "main",
            None,
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("provider thread");
    let provider_text = provider.to_string();
    assert!(!provider_text.contains("call_pair_ttl2"));
    assert!(!provider_text.contains("result_pair_ttl2"));
    assert!(!provider_text.contains("ttl-pair-output-visible"));
}

#[test]
fn pact_lifecycle_keyed_tool_pair_expires_from_context_without_retention() {
    let mut resolver = resolver_with_tree(
        r#"{
                "id": "root",
                "componentType": "root",
                "children": [{
                    "id": "managed_turn",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "assistant",
                    "children": [{
                        "id": "managed_container",
                        "componentType": "message_container",
                        "type": "message_container",
                        "children": [
                            {
                                "id": "managed_call",
                                "componentType": "tool_call",
                                "type": "tool_call",
                                "tool_call_id": "managed_pair",
                                "tool_name": "managed_tool",
                                "parameters": {"path": "x"},
                                "key": "surface:managed",
                                "ttl": 2
                            },
                            {
                                "id": "managed_result",
                                "componentType": "tool_result",
                                "type": "tool_result",
                                "tool_call_id": "managed_pair",
                                "tool_name": "managed_tool",
                                "success": true,
                                "content": "managed ttl payload",
                                "key": "surface:managed",
                                "ttl": 2
                            }
                        ]
                    }]
                }]
            }"#,
    );

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("first managed response")
        .expect("first managed lifecycle commit");
    let head = resolver.preview("main").expect("head after one response");
    assert!(find_node_value(&head, "managed_call").is_some());
    assert!(find_node_value(&head, "managed_result").is_some());

    resolver
        .seal_staged("main", BoundaryKind::Response)
        .expect("second managed response")
        .expect("second managed lifecycle commit");
    let head = resolver.preview("main").expect("head after expiry");
    assert!(find_node_value(&head, "managed_call").is_none());
    assert!(find_node_value(&head, "managed_result").is_none());
    assert!(find_node_value(&head, "managed_container").is_none());
    assert!(find_node_value(&head, "managed_turn").is_none());
}

#[test]
fn pact_lifecycle_tick_and_births_survive_file_reopen() {
    use std::path::PathBuf;

    let tmp = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path: PathBuf = tmp.join(format!("pact_lifecycle_reopen_{nanos}.pact"));

    {
        let mut resolver = fresh_resolver();
        resolver.bind_file(path.clone());
        resolver
            .ingest_operations(vec![
                PactOperation::Append {
                    envelope: env("main"),
                    target_component_id: Some("root".to_string()),
                    target_selector: None,
                    offset: None,
                    component: component(json!({
                        "id": "persisted_call_ttl",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "persisted_pair_ttl",
                        "tool_name": "shell",
                        "parameters": {},
                        "ttl": 2
                    })),
                },
                PactOperation::Append {
                    envelope: env("main"),
                    target_component_id: Some("root".to_string()),
                    target_selector: None,
                    offset: None,
                    component: component(json!({
                        "id": "persisted_result_ttl",
                        "componentType": "tool_result",
                        "type": "tool_result",
                        "tool_call_id": "persisted_pair_ttl",
                        "tool_name": "shell",
                        "success": true,
                        "content": "ok",
                        "ttl": 2
                    })),
                },
            ])
            .expect("ingest pair");
        resolver
            .seal_staged("main", BoundaryKind::Response)
            .expect("first lifecycle commit")
            .expect("commit ref");
        resolver
            .seal_staged("main", BoundaryKind::Response)
            .expect("second lifecycle commit")
            .expect("empty lifecycle commit ref");
        let branch = resolver.ledger.branches.get("main").unwrap();
        assert_eq!(branch.lifecycle_tick, 2);
        assert!(
            find_node_value(
                &resolver.preview("main").expect("head"),
                "persisted_call_ttl"
            )
            .is_some()
        );
    }

    let mut reopened = PactResolver::open_file(path.clone()).expect("open file");
    let branch = reopened.ledger.branches.get("main").unwrap();
    assert_eq!(branch.lifecycle_tick, 2);
    assert_eq!(
        branch
            .current
            .lifecycle_seen
            .get("persisted_call_ttl")
            .copied(),
        Some(1)
    );
    assert_eq!(
        branch
            .current
            .lifecycle_seen
            .get("persisted_result_ttl")
            .copied(),
        Some(1)
    );

    reopened
        .seal_staged("main", BoundaryKind::Response)
        .expect("third lifecycle commit")
        .expect("expiry commit ref");
    let head = reopened.preview("main").expect("head");
    assert!(find_node_value(&head, "persisted_call_ttl").is_none());
    assert!(find_node_value(&head, "persisted_result_ttl").is_none());

    let reopened_after_expiry = PactResolver::open_file(path.clone()).expect("reopen after expiry");
    let hydrated_head = reopened_after_expiry
        .preview("main")
        .expect("hydrated head after expiry");
    assert!(
        find_node_value(&hydrated_head, "persisted_call_ttl").is_none(),
        "expired tool call must stay absent after file hydration"
    );
    assert!(
        find_node_value(&hydrated_head, "persisted_result_ttl").is_none(),
        "expired tool result must stay absent after file hydration"
    );

    std::fs::remove_file(path).ok();
}

#[test]
fn seal_pre_flight_returns_thread_and_advances_head() {
    let mut resolver = fresh_provider_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![PactOperation::Append {
            envelope: env("main"),
            target_component_id: Some("active_container".to_string()),
            target_selector: None,
            offset: None,
            component: component(json!({
                "id": "text_1",
                "componentType": "text_content",
                "content": "hello provider",
                "children": [],
            })),
        }])
        .expect("ingest");

    let report = resolver
        .seal_pre_flight(
            "main",
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("seal preflight");
    assert_eq!(report["schema"], "pact.seal_pre_flight.v1");
    assert!(
        report["thread"].to_string().contains("hello provider"),
        "provider surface should include staged message: {report}"
    );
    assert!(
        !report["commit_ref"].is_null(),
        "staged provider surface commit should carry commit_ref"
    );
    let post_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    assert!(post_rev > initial_rev);
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().lifecycle_tick,
        1,
        "provider-send commits close one Pact lifecycle tick"
    );
    assert!(!resolver.has_staged("main"));
}

#[test]
fn seal_pre_flight_no_work_still_closes_provider_send_cycle() {
    let mut resolver = fresh_resolver();
    let report = resolver
        .seal_pre_flight(
            "main",
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("seal preflight");
    assert_eq!(report["schema"], "pact.seal_pre_flight.v1");
    assert!(!report["commit_ref"].is_null());
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().lifecycle_tick,
        1
    );
}

#[test]
fn seal_pre_flight_always_uses_preflight_boundary() {
    let mut resolver = fresh_resolver();
    let report = resolver
        .seal_pre_flight(
            "main",
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("lifecycle provider commit");
    assert_eq!(report["schema"], "pact.seal_pre_flight.v1");
    assert_eq!(report["commit_ref"]["boundary"], "Preflight");
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().lifecycle_tick,
        1
    );
    assert!(!report["commit_ref"].is_null());
}

#[test]
fn receive_stages_and_seals_post_response() {
    let mut resolver = fresh_resolver();
    let report = resolver
        .receive(
            "main",
            vec![
                component(
                    json!({"componentType": "text_content", "content": "I will call a tool"}),
                ),
                component(json!({
                    "componentType": "tool_call",
                    "tool_call_id": "call_provider_response",
                    "tool_name": "shell_execute",
                    "parameters": {"command": "pwd"}
                })),
            ],
            Map::new(),
        )
        .expect("provider response ingress");

    assert_eq!(report["schema"], "pact.receive.v1");
    assert!(!report["commit_ref"].is_null());
    assert!(!resolver.has_staged("main"));
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().lifecycle_tick,
        1
    );
    assert_eq!(
        resolver.unpaired_tool_call_ids(),
        vec!["call_provider_response".to_string()]
    );
    let preview = resolver.preview("main").expect("preview");
    assert!(
        preview.to_string().contains("I will call a tool"),
        "provider response should be committed into the Pact head: {preview}"
    );
    assert!(
        preview.to_string().contains("call_provider_response"),
        "provider tool call must survive committed PostResponse: {preview}"
    );
}

#[test]
fn receive_seals_already_staged_response_boundary() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "assistant",
            vec![component(
                json!({"componentType": "text_content", "content": "stream complete"}),
            )],
            Map::new(),
        )
        .expect("stage assistant response");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Adhoc)
        .expect("resolve staged response");
    assert!(resolver.has_staged("main"));

    let report = resolver
        .receive_staged("main", Map::new())
        .expect("receive staged response");

    assert_eq!(report["schema"], "pact.receive.v1");
    assert_eq!(report["commit_ref"]["boundary"], "PostResponse");
    assert!(!report["commit_ref"].is_null());
    assert!(!resolver.has_staged("main"));
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().lifecycle_tick,
        1
    );
    let preview = resolver.preview("main").expect("preview");
    assert!(
        preview.to_string().contains("stream complete"),
        "sealed response should be committed into Pact head: {preview}"
    );
}

#[test]
fn receive_streaming_boundary_mints_ids_for_staged_components() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component_without_id(json!({
                "componentType": "text_content",
                "type": "text_content",
                "content": "streamed without host id"
            })),
        }])
        .expect("ingest streamed provider response");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Adhoc)
        .expect("resolve staged provider response");
    assert!(resolver.has_staged("main"));

    let report = resolver
        .receive_staged("main", Map::new())
        .expect("receive staged provider response");
    assert_eq!(report["schema"], "pact.receive.v1");
    let preview = resolver.preview("main").expect("preview");
    let node = find_node_by_content(&preview, "streamed without host id")
        .expect("sealed response should include staged streamed content");
    let component_id = node.get("id").and_then(Value::as_str).expect("id");
    assert!(
        component_id.starts_with("node_"),
        "streaming close should commit a Pact-minted id, got {component_id:?}"
    );
}

#[test]
fn build_provider_thread_is_read_only_over_staged_trace() {
    let mut resolver = fresh_provider_resolver();
    let initial_rev = resolver.ledger.branches.get("main").unwrap().head_revision;
    resolver
        .ingest_operations(vec![PactOperation::Append {
            envelope: env("main"),
            target_component_id: Some("active_container".to_string()),
            target_selector: None,
            offset: None,
            component: component(json!({
                "id": "text_preview",
                "componentType": "text_content",
                "content": "preview provider",
                "children": [],
            })),
        }])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PreRequest)
        .expect("resolve");

    let thread = resolver
        .build_provider_thread(
            "main",
            None,
            &crate::provider_thread::ProviderThreadOptions::default(),
        )
        .expect("provider thread");

    assert!(thread.to_string().contains("preview provider"));
    assert_eq!(
        resolver.ledger.branches.get("main").unwrap().head_revision,
        initial_rev,
        "rendering provider thread must not commit/seal"
    );
    assert!(resolver.has_staged("main"));
}

#[test]
fn commit_seals_mutation_trace_into_history() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("trace me".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    assert_eq!(
        resolver
            .staged_traces
            .get("main")
            .map(|events| events.len()),
        Some(1),
        "mutation should automatically append to mutable staged traces"
    );
    let staged = resolver.staged_traces("main");
    assert_eq!(staged.len(), 1);
    assert!(
        !staged[0].ops.is_empty(),
        "staged trace handle must carry trace-op metadata before seal"
    );

    let commit_ref = resolver
        .seal_staged("main", BoundaryKind::Authored)
        .expect("seal")
        .expect("commit ref");
    assert!(!resolver.staged_traces.contains_key("main"));
    let trace = resolver
        .ledger
        .event_index
        .transaction_traces
        .get(&format!("tx_main_{}", commit_ref.revision))
        .expect("sealed trace");
    assert_eq!(trace.branch_id, "main");
    assert_eq!(
        trace.post_response_ref.as_ref().unwrap().revision,
        Some(commit_ref.revision)
    );
    assert_eq!(trace.aux["boundary"], "authored");
    assert_eq!(trace.aux["events"].as_array().unwrap().len(), 1);
    let branch = resolver.ledger.branches.get("main").expect("main branch");
    assert_eq!(branch.transaction_traces.len(), 1);
    assert!(
        !branch.transaction_traces[0].ops.is_empty(),
        "seal must convert staged work into durable transaction trace ops"
    );
}

#[test]
fn committed_tool_pair_replays_from_durable_transaction_trace() {
    let mut resolver = fresh_provider_resolver();
    let base_current = resolver.ledger.branches["main"].current.clone();
    resolver
        .ingest_operations(vec![
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Assistant,
                    ..env("main")
                },
                target_selector: None,
                parent_id: Some("active_container".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "call_trace_replay",
                    "componentType": "tool_call",
                    "tool_call_id": "tc_trace_replay",
                    "tool_name": "shell",
                    "parameters": {"cmd": "pwd"},
                    "children": []
                })),
            },
            PactOperation::Insert {
                envelope: PactOperationEnvelope {
                    source: SourceActor::Host,
                    ..env("main")
                },
                target_selector: None,
                parent_id: Some("active_container".to_string()),
                depth: None,
                offset: None,
                component: component(json!({
                    "id": "result_trace_replay",
                    "componentType": "tool_result",
                    "tool_call_id": "tc_trace_replay",
                    "tool_name": "shell",
                    "success": true,
                    "content": "ok",
                    "children": []
                })),
            },
        ])
        .expect("ingest tool pair");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::PostResponseAlteration)
        .expect("resolve tool pair");
    resolver
        .seal_staged("main", BoundaryKind::PostResponseAlteration)
        .expect("seal tool pair")
        .expect("commit ref");

    let branch = resolver.ledger.branches.get("main").expect("main branch");
    assert!(
        !branch.transaction_traces.is_empty(),
        "sealed tool pair must produce durable replay trace history"
    );

    let mut replay_only_ledger = resolver.ledger().clone();
    replay_only_ledger
        .branches
        .get_mut("main")
        .expect("main branch")
        .current = base_current;
    let replay_branch = replay_only_ledger
        .branches
        .get("main")
        .expect("main branch");
    let replayed = project_branch_component_tree(
        &replay_only_ledger,
        replay_branch,
        Some(replay_branch.head_revision),
    )
    .expect("project from durable trace")
    .project_value();
    let call = find_node_value(&replayed, "call_trace_replay").expect("replayed tool call");
    let result = find_node_value(&replayed, "result_trace_replay").expect("replayed tool result");
    assert_eq!(
        call.get("tool_call_id").and_then(Value::as_str),
        Some("tc_trace_replay")
    );
    assert_eq!(
        result.get("tool_call_id").and_then(Value::as_str),
        Some("tc_trace_replay")
    );
}

#[test]
fn fork_at_revision_replays_native_parent_trace_history() {
    let mut resolver = fresh_resolver();
    let base_current = resolver.ledger.branches["main"].current.clone();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("first traced revision".to_string()),
            None,
        )])
        .expect("ingest first");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve first");
    resolver
        .seal_staged("main", BoundaryKind::Authored)
        .expect("seal first")
        .expect("first commit ref");
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("second traced revision".to_string()),
            None,
        )])
        .expect("ingest second");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve second");
    resolver
        .seal_staged("main", BoundaryKind::Authored)
        .expect("seal second")
        .expect("second commit ref");

    let mut root_store = resolver.ledger().clone();
    let parent = root_store.branches.get_mut("main").expect("main branch");
    assert_eq!(parent.head_revision, 2);
    assert_eq!(
        parent.transaction_traces.len(),
        2,
        "native commits must leave replayable parent trace history"
    );
    parent.current = base_current;

    let (root_store, fork_id) = root_store
        .fork_at_revision(
            "main",
            Some(1),
            Some("trace_revision_fork".to_string()),
            Map::new(),
            DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES,
        )
        .expect("fork from parent trace history");
    let fork = root_store.branches.get(&fork_id).expect("fork branch");
    let forked_tree = fork.current.tree.root.project_component_value();
    assert!(
        forked_tree.to_string().contains("first traced revision"),
        "forked revision must include the selected parent trace: {forked_tree}"
    );
    assert!(
        !forked_tree.to_string().contains("second traced revision"),
        "forked revision must stop at the requested parent revision: {forked_tree}"
    );
}

#[test]
fn insert_tool_call_node_records_execute_requirement() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: PactOperationEnvelope {
                source: SourceActor::Assistant,
                ..env("main")
            },
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "call_tc_1",
                "componentType": "tool_call",
                "tool_call_id": "tc_1",
                "tool_name": "shell",
                "parameters": {"cmd": "ls"}
            })),
        }])
        .expect("ingest");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Response)
        .expect("resolve");
    assert_eq!(outcome.pending_external_mutations.len(), 1);
    assert_eq!(
        outcome.pending_external_mutations[0].idempotency_key,
        "tool_exec:tc_1"
    );
    assert_eq!(resolver.unpaired_tool_call_ids(), vec!["tc_1".to_string()]);
    assert_eq!(resolver.list_tool_calls().len(), 1);
}

#[test]
fn discard_staged_discards_mutable_staged_traces() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![message_op(
            env("main"),
            "user".to_string(),
            Some("discard me".to_string()),
            None,
        )])
        .expect("ingest");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    assert!(resolver.staged_traces.contains_key("main"));
    assert!(resolver.discard_staged("main"));
    assert!(!resolver.has_staged("main"));
    assert!(!resolver.staged_traces.contains_key("main"));
}

fn update_request(component_id: &str) -> crate::mutation::MutationRequest {
    crate::mutation::MutationRequest {
        schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
        request_id: None,
        transaction_id: None,
        kind: MutationKind::Update,
        target: crate::mutation::MutationTarget {
            component_id: Some(component_id.to_string()),
            ..Default::default()
        },
        node: None,
        mode: None,
        content: None,
        content_ref: None,
        fields: Map::new(),
        attrs: Map::new(),
    }
}

#[test]
fn commit_update_content_emits_attach_content_ref_not_replace_component() {
    let mut content = crate::store::ContentStore::default();
    let mut request = update_request("n1");
    request.content = Some("fresh body".to_string());
    let tree = json!({ "id": "root", "children": [{ "id": "n1", "content": "old body" }] });

    let (commit, nodes) = commit_record_from_mutations("main", 1, &[request], &tree, &mut content)
        .expect("commit record");

    assert!(
        nodes.is_empty(),
        "content update must not persist a whole node"
    );
    assert!(
        commit
            .ops
            .iter()
            .all(|op| !matches!(op, CommitRecordOp::ReplaceComponent { .. })),
        "content update must not emit ReplaceComponent: {:?}",
        commit.ops
    );
    let blob_id = match commit.ops.as_slice() {
        [
            CommitRecordOp::AttachContentRef {
                component_id,
                blob_id,
            },
        ] if component_id == "n1" => blob_id.clone(),
        other => panic!("expected a single AttachContentRef, got {other:?}"),
    };
    let record = content.records.get(&blob_id).expect("content blob present");
    assert_eq!(record.storage.inline.as_deref(), Some("fresh body"));
}

#[test]
fn commit_update_attrs_emits_set_component_aux() {
    let mut content = crate::store::ContentStore::default();
    let mut request = update_request("n1");
    request.attrs.insert("status".to_string(), json!("ok"));
    let tree = json!({ "id": "root", "children": [{ "id": "n1" }] });

    let (commit, nodes) = commit_record_from_mutations("main", 1, &[request], &tree, &mut content)
        .expect("commit record");

    assert!(nodes.is_empty());
    let value_ref = match commit.ops.as_slice() {
        [
            CommitRecordOp::SetComponentAux {
                component_id,
                key_symbol,
                value_ref,
            },
        ] if component_id == "n1" && key_symbol == "status" => value_ref.clone(),
        other => panic!("expected a single SetComponentAux, got {other:?}"),
    };
    let record = content
        .records
        .get(&value_ref)
        .expect("attr value blob present");
    assert_eq!(record.storage.inline.as_deref(), Some("\"ok\""));
}

#[test]
fn commit_update_promoted_field_falls_back_to_replace_component() {
    let mut content = crate::store::ContentStore::default();
    let mut request = update_request("n1");
    request
        .fields
        .insert("role".to_string(), json!("assistant"));
    let tree = json!({ "id": "root", "children": [{ "id": "n1", "role": "user" }] });

    let (commit, nodes) = commit_record_from_mutations("main", 1, &[request], &tree, &mut content)
        .expect("commit record");

    assert_eq!(
        nodes.len(),
        1,
        "promoted field update keeps a node snapshot"
    );
    assert!(matches!(
        commit.ops.as_slice(),
        [CommitRecordOp::ReplaceComponent { component_id, .. }] if component_id == "n1"
    ));
}

#[test]
fn commit_update_unresolvable_content_ref_falls_back_to_replace_component() {
    let mut content = crate::store::ContentStore::default();
    let mut request = update_request("n1");
    request.content_ref = Some(crate::components::PactContentRef {
        id: "cnt_external".to_string(),
        field: None,
        chars: None,
        preview: None,
        source: None,
    });
    let tree = json!({
        "id": "root", "children": [{ "id": "n1", "content_ref": { "id": "cnt_external" } }]
    });

    let (commit, nodes) = commit_record_from_mutations("main", 1, &[request], &tree, &mut content)
        .expect("commit record");

    assert_eq!(nodes.len(), 1, "unresolvable ref keeps a node snapshot");
    assert!(
        commit
            .ops
            .iter()
            .all(|op| !matches!(op, CommitRecordOp::AttachContentRef { .. })),
        "must not emit a dangling AttachContentRef: {:?}",
        commit.ops
    );
    assert!(matches!(
        commit.ops.as_slice(),
        [CommitRecordOp::ReplaceComponent { component_id, .. }] if component_id == "n1"
    ));
}

#[test]
fn outcome_metadata_after_message_construction() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "user",
            vec![component(
                json!({"componentType": "text_content", "content": "hi"}),
            )],
            Map::new(),
        )
        .expect("add_message");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    assert_eq!(
        outcome.produced_message_ids.len(),
        1,
        "one message turn produced: {:?}",
        outcome.produced_message_ids
    );
    assert_eq!(
        outcome.produced_message_child_ids.len(),
        1,
        "one message child produced: {:?}",
        outcome.produced_message_child_ids
    );

    assert!(
        outcome
            .produced_component_ids
            .iter()
            .any(|id| outcome.produced_message_ids.contains(id)),
        "produced_component_ids includes the message turn"
    );
    assert!(
        outcome
            .produced_message_child_ids
            .iter()
            .all(|id| outcome.produced_component_ids.contains(id)),
        "message children are a subset of produced_component_ids"
    );
    assert!(
        outcome
            .produced_component_ids
            .iter()
            .all(|id| outcome.affected_component_ids.contains(id)),
        "produced ids are a subset of affected ids"
    );
    assert!(outcome.replaced_component_ids.is_empty());
    assert!(outcome.deleted_component_ids.is_empty());
}

#[test]
fn outcome_metadata_after_append() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "turn_append",
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "assistant",
                "children": [
                    {
                        "id": "seed_block",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "seed"
                    }
                ]
            })),
        }])
        .expect("ingest insert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve insert");

    resolver
        .ingest_operations(vec![PactOperation::Append {
            envelope: env("main"),
            target_component_id: Some("turn_append".to_string()),
            target_selector: None,
            offset: None,
            component: component(json!({
                "id": "appended_block",
                "componentType": "text_content",
                "type": "text_content",
                "content": "appended"
            })),
        }])
        .expect("ingest append");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve append");

    assert!(
        outcome
            .produced_component_ids
            .contains(&"appended_block".to_string()),
        "append produces the new child id: {:?}",
        outcome.produced_component_ids
    );
    assert!(
        outcome
            .affected_component_ids
            .contains(&"appended_block".to_string()),
        "append marks the new child affected"
    );
}

#[test]
fn outcome_metadata_after_replace() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "block_orig",
                "componentType": "text_content",
                "type": "text_content",
                "content": "original"
            })),
        }])
        .expect("ingest insert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve insert");

    resolver
        .ingest_operations(vec![PactOperation::Replace {
            envelope: env("main"),
            target_component_id: Some("block_orig".to_string()),
            target_selector: None,
            component: component(json!({
                "id": "block_orig",
                "componentType": "text_content",
                "type": "text_content",
                "content": "replaced"
            })),
        }])
        .expect("ingest replace");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve replace");

    assert_eq!(
        outcome.replaced_component_ids,
        vec!["block_orig".to_string()],
        "replace records the replaced id"
    );
    assert!(
        outcome
            .affected_component_ids
            .contains(&"block_orig".to_string()),
        "replaced id is affected"
    );
    assert!(outcome.deleted_component_ids.is_empty());
}

#[test]
fn outcome_metadata_after_delete() {
    let mut resolver = fresh_resolver();
    resolver
        .ingest_operations(vec![PactOperation::Insert {
            envelope: env("main"),
            target_selector: None,
            parent_id: Some("root".to_string()),
            depth: None,
            offset: None,
            component: component(json!({
                "id": "turn_del",
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "user",
                "children": [
                    {
                        "id": "block_del",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "doomed"
                    }
                ]
            })),
        }])
        .expect("ingest insert");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve insert");

    resolver
        .ingest_operations(vec![PactOperation::Delete {
            envelope: env("main"),
            target_component_id: Some("turn_del".to_string()),
            target_selector: None,
        }])
        .expect("ingest delete");
    let outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve delete");

    assert!(
        outcome
            .deleted_component_ids
            .contains(&"turn_del".to_string()),
        "delete records the removed subtree root: {:?}",
        outcome.deleted_component_ids
    );
    assert!(
        outcome
            .affected_component_ids
            .contains(&"turn_del".to_string()),
        "deleted id is affected"
    );
    assert!(outcome.produced_component_ids.is_empty());
}

#[test]
fn outcome_metadata_after_tool_call_and_result_insertion() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "assistant",
            vec![component(json!({
                "componentType": "tool_call",
                "tool_call_id": "call_meta_1",
                "tool_name": "shell_execute",
                "parameters": {"cmd": "pwd"}
            }))],
            Map::new(),
        )
        .expect("assistant tool call");
    let call_outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve call");
    assert!(
        !call_outcome.produced_component_ids.is_empty(),
        "tool_call insertion produces node ids: {:?}",
        call_outcome.produced_component_ids
    );

    resolver
        .add_message(
            "main",
            "user",
            vec![component(json!({
                "componentType": "tool_result",
                "tool_call_id": "call_meta_1",
                "success": true,
                "result": {"stdout": "/tmp"}
            }))],
            Map::new(),
        )
        .expect("user tool result");
    let result_outcome = resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve result");
    assert!(
        !result_outcome.produced_component_ids.is_empty(),
        "tool_result insertion produces node ids: {:?}",
        result_outcome.produced_component_ids
    );
    assert!(
        result_outcome
            .produced_component_ids
            .iter()
            .all(|id| result_outcome.affected_component_ids.contains(id)),
        "produced tool-result ids are affected"
    );
}

#[test]
fn select_dto_populates_rich_fields_for_tool_nodes() {
    let mut resolver = fresh_resolver();
    resolver
        .add_message(
            "main",
            "assistant",
            vec![
                component(json!({"componentType": "text_content", "content": "calling tool"})),
                component(json!({
                    "componentType": "tool_call",
                    "tool_call_id": "call_select_1",
                    "tool_name": "shell_execute",
                    "parameters": {"cmd": "ls"}
                })),
            ],
            Map::new(),
        )
        .expect("assistant add");
    resolver
        .add_message(
            "main",
            "user",
            vec![component(json!({
                "componentType": "tool_result",
                "tool_call_id": "call_select_1",
                "success": true,
                "result": {"stdout": "ok"}
            }))],
            Map::new(),
        )
        .expect("tool result add");
    resolver
        .resolve(Vec::new(), "main", BoundaryKind::Authored)
        .expect("resolve");

    let tool_calls = resolver
        .select("main", ".tool_call", false)
        .expect("select tool_call");
    assert_eq!(tool_calls.len(), 1, "one tool_call: {tool_calls:?}");
    let call = &tool_calls[0];
    assert_eq!(call.component_type, "tool_call");
    assert_eq!(call.tool_call_id.as_deref(), Some("call_select_1"));
    assert_eq!(call.tool_name.as_deref(), Some("shell_execute"));
    assert_eq!(call.parameters, Some(json!({"cmd": "ls"})));
    assert!(call.parent_id.is_some(), "tool_call carries parent_id");
    assert!(!call.archived);
    assert!(!call.deleted);

    let tool_results = resolver
        .select("main", ".tool_result", false)
        .expect("select tool_result");
    assert_eq!(tool_results.len(), 1, "one tool_result: {tool_results:?}");
    let result = &tool_results[0];
    assert_eq!(result.component_type, "tool_result");
    assert_eq!(result.tool_call_id.as_deref(), Some("call_select_1"));
    assert_eq!(result.result, Some(json!({"stdout": "ok"})));

    let text = resolver
        .select("main", ".text_content", false)
        .expect("select text");
    assert!(
        text.iter()
            .any(|node| node.content.as_deref() == Some("calling tool")),
        "text content surfaced: {text:?}"
    );
}

#[test]
fn select_visibility_control_filters_archived_nodes() {
    let root = r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {
                    "id": "turn_vis",
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": "user",
                    "children": [
                        {
                            "id": "live_block",
                            "componentType": "text_content",
                            "type": "text_content",
                            "content": "live"
                        },
                        {
                            "id": "archived_block",
                            "componentType": "text_content",
                            "type": "text_content",
                            "content": "old",
                            "archived": true
                        }
                    ]
                }
            ]
        }"#;
    let resolver = resolver_with_tree(root);

    let visible = resolver
        .select("main", ".text_content", false)
        .expect("select default");
    assert_eq!(
        visible.len(),
        1,
        "archived node excluded by default: {visible:?}"
    );
    assert_eq!(visible[0].id, "live_block");

    let all = resolver
        .select("main", ".text_content", true)
        .expect("select include_archived");
    assert_eq!(all.len(), 2, "include_archived keeps archived: {all:?}");
    let archived = all.iter().find(|node| node.id == "archived_block").unwrap();
    assert!(archived.archived, "archived flag surfaced");
}

#[test]
fn boundary_alias_normalization_for_all_four_aliases() {
    let preflight_canonical: BoundaryKind =
        serde_json::from_str("\"Preflight\"").expect("Preflight");
    let pre_request: BoundaryKind = serde_json::from_str("\"pre_request\"").expect("pre_request");
    let before_provider_call: BoundaryKind =
        serde_json::from_str("\"before_provider_call\"").expect("before_provider_call");
    assert_eq!(pre_request, BoundaryKind::PreRequest);
    assert_eq!(before_provider_call, BoundaryKind::PreRequest);
    assert_eq!(preflight_canonical, BoundaryKind::PreRequest);

    let post_response_canonical: BoundaryKind =
        serde_json::from_str("\"PostResponse\"").expect("PostResponse");
    let post_response: BoundaryKind =
        serde_json::from_str("\"post_response\"").expect("post_response");
    let after_provider_response: BoundaryKind =
        serde_json::from_str("\"after_provider_response\"").expect("after_provider_response");
    assert_eq!(post_response, BoundaryKind::PostResponse);
    assert_eq!(after_provider_response, BoundaryKind::PostResponse);
    assert_eq!(post_response_canonical, BoundaryKind::PostResponse);

    assert_eq!(
        serde_json::to_string(&BoundaryKind::PreRequest).unwrap(),
        "\"Preflight\""
    );
    assert_eq!(
        serde_json::to_string(&BoundaryKind::PostResponse).unwrap(),
        "\"PostResponse\""
    );
}

#[test]
fn ledger_head_naming_contract() {
    let resolver = PactResolver::new("test_context", None).expect("resolver");
    let ledger: &crate::store::ContextTreeLedger = resolver.ledger();
    let head: &crate::store::PactView = resolver.head().expect("head view");
    assert!(!ledger.current_branch_id.is_empty());
    let _ = head;
}
