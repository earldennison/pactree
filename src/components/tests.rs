use super::*;

#[test]
fn parses_component_json_into_native_node_and_orders_children() {
    let value = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {"id": "b", "componentType": "text_content", "offset": 1, "content": "later"},
            {"id": "a", "componentType": "text_content", "offset": 0, "content": "first"}
        ]
    });

    let node = Component::from_component_value(&value).expect("native node");
    let ids: Vec<&str> = node
        .children
        .iter()
        .map(|child| child.id.as_str())
        .collect();
    assert_eq!(ids, vec!["a", "b"]);
}

#[test]
fn projects_native_node_back_to_component_json() {
    let json = r#"{
          "id": "text",
          "componentType": "text_content",
          "type": "text_content",
          "parent_id": "container",
          "offset": 2,
          "content": "hello",
          "attrs": {"source": "test"},
          "aux": {"text": "hello"},
          "key": "stable"
        }"#;

    let node_json = component_from_json(json).expect("native node json");
    let projected_json = component_project_json(&node_json).expect("projection");
    let projected: Value = serde_json::from_str(&projected_json).expect("json");
    assert_eq!(projected["id"], "text");
    assert_eq!(projected["componentType"], "text_content");
    assert_eq!(projected["content"], "hello");
    assert_eq!(projected["aux"]["text"], "hello");
    assert_eq!(projected["aux"]["source"], "test");
    assert_eq!(projected["key"], "stable");
    assert!(projected["aux"].get("key").is_none());
    assert_eq!(projected["child_ids"].as_array().unwrap().len(), 0);
}

#[test]
fn component_json_adapters_use_component_vocabulary() {
    let json = r#"{
          "id": "text",
          "componentType": "text_content",
          "type": "text_content",
          "content": "hello"
        }"#;

    let component_json = component_from_json(json).expect("component json");
    let projected_json = component_project_json(&component_json).expect("projection");
    let projected: Value = serde_json::from_str(&projected_json).expect("json");

    assert_eq!(projected["id"], "text");
    assert_eq!(projected["componentType"], "text_content");
    assert_eq!(projected["content"], "hello");
}

#[test]
fn component_prepare_for_context_json_preserves_wire_shape() {
    let json = r#"{
          "id": "text",
          "componentType": "text_content",
          "type": "text_content",
          "content": "hello"
        }"#;
    let context = r#"{"next_creation_index": 7}"#;

    let prepared_json =
        component_prepare_for_context_json(json, context).expect("prepared component");
    let prepared: Value = serde_json::from_str(&prepared_json).expect("json");

    assert_eq!(prepared["node"]["id"], "text");
    assert_eq!(prepared["assigned_ids"][0], "text");
    assert_eq!(prepared["next_creation_index"], 8);
}

#[test]
fn external_refs_round_trip_and_project_publicly() {
    let value = serde_json::json!({
        "id": "text",
        "componentType": "text_content",
        "content": "hi",
        "external_refs": [
            {"kind": "citation", "uri": "https://example.com", "role": "source"}
        ]
    });
    let node = Component::from_component_value(&value).expect("node");
    assert_eq!(node.external_refs.len(), 1);
    assert_eq!(node.external_refs[0].kind, "citation");
    let projected = node.project_component_value();
    assert_eq!(projected["external_refs"][0]["kind"], "citation");
    assert_eq!(projected["external_refs"][0]["uri"], "https://example.com");

    assert!(projected["aux"].get("external_refs").is_none());
}

#[test]
fn normal_projection_hides_internal_refs_but_inspect_shows_them() {
    let mut node = Component::from_component_value(&serde_json::json!({
        "id": "text", "componentType": "text_content", "content": "hi"
    }))
    .expect("node");
    node.lifecycle.internal_refs.state = Some(InternalRef {
        id: "state_1".to_string(),
        kind: "component_state".to_string(),
        field: None,
        aux: Default::default(),
    });
    node.lifecycle.revision = 7;

    let projected = node.project_component_value();
    assert_eq!(projected["lifecycle"]["revision"], 7);
    assert!(projected["lifecycle"].get("internal_refs").is_none());
    let projected_str = serde_json::to_string(&projected).unwrap();
    assert!(
        !projected_str.contains("state_1"),
        "internal ref leaked into normal projection"
    );

    let inspected = node.inspect_component_value();
    assert_eq!(
        inspected["lifecycle"]["internal_refs"]["state"]["id"],
        "state_1"
    );
    assert_eq!(inspected["lifecycle"]["revision"], 7);
}

#[test]
fn lifecycle_state_parse_rejects_noncanonical() {
    assert_eq!(
        LifecycleState::parse("active").unwrap(),
        LifecycleState::Active
    );
    assert_eq!(
        LifecycleState::parse("render_requested").unwrap(),
        LifecycleState::RenderRequested
    );
    let err = LifecycleState::parse("registered").unwrap_err();
    assert!(format!("{err}").contains("non-canonical lifecycle state"));
}

#[test]
fn guard_projection_never_emits_owner_or_context_id() {
    let node = Component::from_component_value(&serde_json::json!({
        "id": "n", "componentType": "text_content", "content": "x",
        "owner": "main", "context_id": "ctx"
    }))
    .expect("node");
    let projected = node.project_component_value();
    assert!(
        projected.get("owner").is_none(),
        "owner leaked into projection"
    );
    assert!(
        projected.get("context_id").is_none(),
        "context_id leaked into projection"
    );

    assert!(projected["aux"].get("owner").is_none());
    assert!(projected["aux"].get("context_id").is_none());
}

#[test]
fn guard_tool_call_and_result_are_distinct_kinds_not_text() {
    let call = Component::from_component_value(&serde_json::json!({
        "id": "tc", "componentType": "tool_call",
        "tool_call_id": "call_1", "tool_name": "search",
        "parameters": {"q": "x"}
    }))
    .expect("tool_call");
    assert!(matches!(call.kind(), ComponentKind::ToolCall(_)));
    assert_eq!(call.component_type(), "tool_call");

    let result = Component::from_component_value(&serde_json::json!({
        "id": "tr", "componentType": "tool_result",
        "tool_call_id": "call_1", "success": true, "content": "ok"
    }))
    .expect("tool_result");
    assert!(matches!(result.kind(), ComponentKind::ToolResult(_)));
    assert_eq!(result.component_type(), "tool_result");
}

#[test]
fn guard_single_discriminant_one_kind_tag_round_trips() {
    let node = Component::from_component_value(&serde_json::json!({
        "id": "t", "componentType": "text_content", "content": "hello"
    }))
    .expect("node");
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "text_content");
    let reparsed = Component::from_component_value(&projected).expect("reparse");
    assert_eq!(reparsed.component_type(), "text_content");
    assert_eq!(reparsed.content(), Some("hello"));
}

#[test]
fn guard_unknown_kind_round_trips_payload_losslessly() {
    let value = serde_json::json!({
        "id": "vc", "componentType": "vendor_custom_kind",
        "content": "[No working directory]",
        "custom_field": {"nested": 1}
    });
    let node = Component::from_component_value(&value).expect("node");
    assert!(matches!(node.kind(), ComponentKind::Unknown(_)));
    assert_eq!(node.content(), Some("[No working directory]"));
    let projected = node.project_component_value();

    assert_eq!(projected["content"], "[No working directory]");
    assert_eq!(projected["aux"]["custom_field"]["nested"], 1);
}

#[test]
fn guard_lifecycle_state_and_revision_round_trip_on_component() {
    let mut node = Component::from_component_value(&serde_json::json!({
        "id": "n", "componentType": "text_content", "content": "x"
    }))
    .expect("node");
    node.lifecycle.state = LifecycleState::Rendered;
    node.lifecycle.revision = 4;

    let projected = node.project_component_value();
    assert_eq!(projected["lifecycle"]["state"], "rendered");
    assert_eq!(projected["lifecycle"]["revision"], 4);

    assert!(projected["lifecycle"].get("internal_refs").is_none());

    let reparsed = Component::from_component_value(&projected).expect("reparse");
    assert_eq!(reparsed.lifecycle.state, LifecycleState::Rendered);
    assert_eq!(reparsed.lifecycle.revision, 4);

    let plain = Component::from_component_value(&serde_json::json!({
        "id": "m", "componentType": "text_content", "content": "y"
    }))
    .expect("node");
    assert!(plain.project_component_value().get("lifecycle").is_none());
}

#[test]
fn canonical_lifecycle_key_and_tags_do_not_store_in_attrs() {
    let json = r#"{
          "id": "note",
          "componentType": "text_content",
          "ttl": 2,
          "cadence": 3,
          "key": "family",
          "tags": ["sticky", "important"],
          "attrs": {
            "ttl": 2,
            "cad": 3,
            "key": "family",
            "tags": ["sticky", "important"],
            "host_note": "kept"
          }
        }"#;

    let node_json = component_from_json(json).expect("native node json");
    let node: Component = serde_json::from_str(&node_json).expect("native node");
    assert_eq!(node.lifecycle.policy.ttl, Some(2));
    assert_eq!(node.lifecycle.policy.cad, Some(3));
    assert_eq!(node.key.as_deref(), Some("family"));
    assert_eq!(node.tags, vec!["sticky", "important"]);
    assert!(!node.aux.contains_key("ttl"));
    assert!(!node.aux.contains_key("cad"));
    assert!(!node.aux.contains_key("key"));
    assert!(!node.aux.contains_key("tags"));
    assert_eq!(node.aux["host_note"], "kept");

    let projected_json = component_project_json(&node_json).expect("projection");
    let projected: Value = serde_json::from_str(&projected_json).expect("json");
    assert_eq!(projected["ttl"], 2);
    assert_eq!(projected["cad"], 3);
    assert_eq!(projected["key"], "family");
    assert_eq!(projected["tags"][1], "important");
    assert!(projected["aux"].get("ttl").is_none());
    assert!(projected["aux"].get("cad").is_none());
    assert!(projected["aux"].get("key").is_none());
    assert!(projected["aux"].get("tags").is_none());
}

#[test]
fn cadence_without_ttl_is_rejected() {
    let err = component_from_json(
        r#"{
              "id": "bad_lifecycle",
              "componentType": "text_content",
              "cad": 2,
              "content": "invalid"
            }"#,
    )
    .expect_err("cad without ttl must be invalid");

    assert!(err.to_string().contains("cad without ttl"), "{err}");
}

#[test]
fn projects_hot_pact_attrs_and_orders_conversation_history_by_depth() {
    let json = r#"{
          "id": "history",
          "componentType": "conversation_history",
          "children": [
            {"id": "d1", "componentType": "message_turn", "pact_depth": 1, "attrs": {"ttl": null}},
            {"id": "d2", "componentType": "message_turn", "pact_depth": 2, "attrs": {"ttl": 3}}
          ]
        }"#;

    let node_json = component_from_json(json).expect("native node json");
    let projected_json = component_project_json(&node_json).expect("projection");
    let projected: Value = serde_json::from_str(&projected_json).expect("json");

    assert_eq!(projected["children"][0]["id"], "d2");
    assert_eq!(projected["children"][0]["ttl"], 3);
    assert!(projected["children"][1].get("ttl").is_some());
}

#[test]
fn rejects_non_string_content_at_component_boundary() {
    let json = r#"{"id":"bad","componentType":"text_content","content":{"host":"object"}}"#;
    let err = component_from_json(json).expect_err("rejects host-shaped content");
    assert!(err.to_string().contains("$.content"));
    assert!(err.to_string().contains("string"));
}

#[test]
fn rejects_removed_surface_escape_hatch_at_component_boundary() {
    let json =
        r#"{"id":"bad","componentType":"text_content","surface":{"reasoning_content":"old"}}"#;
    let err = component_from_json(json).expect_err("rejects surface");
    assert!(err.to_string().contains("$.surface"));
    assert!(err.to_string().contains("not a PACT component field"));
}

#[test]
fn canonical_text_content_is_built_by_rust() {
    let node_json =
        pact_canonical_component_json("TextContent", r#"{"id":"txt","content":"hello"}"#)
            .expect("canonical text component");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["componentType"], "text_content");
    assert_eq!(node["type"], "text_content");
    assert_eq!(node["content"], "hello");
}

#[test]
fn canonical_component_escape_hatch_accepts_extension_type() {
    let node_json = pact_canonical_component_json(
        "PACTComponent",
        r#"{"id":"custom","componentType":"org:panel","attrs":{"mode":"compact"}}"#,
    )
    .expect("canonical extension component");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["componentType"], "org:panel");

    assert_eq!(node["attrs"]["mode"], "compact");
}

#[test]
fn canonical_component_escape_hatch_accepts_declared_inline_extension_payload() {
    let large = "private metadata ".repeat(128);
    let payload = serde_json::json!({
        "id": "custom_inline",
        "componentType": "org:panel",
        "aux": {
            "private_reason": large,
            "payload_slot_policies": {
                "private_reason": {"strategy": "inline", "provider_visible": true}
            }
        }
    });
    let node_json = pact_canonical_component_json("PACTComponent", &payload.to_string())
        .expect("inline policy");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["aux"]["private_reason"], large);
    assert_eq!(
        node["aux"]["payload_slot_policies"]["private_reason"]["strategy"],
        "inline"
    );
}

#[test]
fn canonical_component_escape_hatch_rejects_undeclared_large_extension_payload() {
    let large = "private metadata ".repeat(128);
    let payload = serde_json::json!({
        "id": "custom_inline",
        "componentType": "org:panel",
        "aux": {"private_reason": large}
    });
    let err = pact_canonical_component_json("PACTComponent", &payload.to_string())
        .expect_err("large extension payload needs policy or ref");

    assert!(
        err.to_string().contains("private_reason"),
        "expected field name in error, got {err}"
    );
    assert!(err.to_string().contains("content_ref"));
}

#[test]
fn canonical_component_constructor_rejects_unknown_kind_without_escape_hatch() {
    let err = pact_canonical_component_json("org:panel", r#"{"id":"custom"}"#)
        .expect_err("unknown kind rejected");

    assert!(err.to_string().contains("use PACTComponent"));
}

#[test]
fn canonical_component_constructor_rejects_node_escape_hatch() {
    let err = pact_canonical_component_json(
        "PACTNode",
        r#"{"id":"custom","componentType":"org:panel","attrs":{"mode":"compact"}}"#,
    )
    .expect_err("node escape hatch is not a canonical kind");

    assert!(err.to_string().contains("use PACTComponent"));
}

#[test]
fn canonical_nodes_reject_full_content_next_to_content_ref() {
    let err = pact_canonical_component_json(
        "TextContent",
        r#"{
              "id":"txt",
              "content":"full duplicated body",
              "content_ref":{"id":"cnt_ab12","preview":"full"}
            }"#,
    )
    .expect_err("content plus ref rejected");

    assert!(err.to_string().contains("$.content"));
    assert!(err.to_string().contains("content_ref"));
}

#[test]
fn canonical_nodes_reject_inline_text_in_content_ref() {
    let err = pact_canonical_component_json(
            "ToolResult",
            r#"{
              "id":"tool_result_bad_ref",
              "tool_call_id":"call_1",
              "tool_name":"shell_execute",
              "success":false,
              "content_ref":"Parameter validation failed for execute: got an unexpected keyword argument 'max_chars'"
            }"#,
        )
        .expect_err("content_ref is a storage ref id, not inline content");

    assert!(err.to_string().contains("$.content_ref.id"));
    assert!(err.to_string().contains("not inline content"));
}

#[test]
fn canonical_nodes_ignore_null_content_ref_from_host_models() {
    let node_json = pact_canonical_component_json(
        "ToolResult",
        r#"{
              "id":"tool_result_null_ref",
              "tool_call_id":"call_1",
              "tool_name":"shell_execute",
              "success":false,
              "content":"ordinary failed tool output",
              "content_ref":null
            }"#,
    )
    .expect("null content_ref is omitted by the canonical boundary");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["content"], "ordinary failed tool output");
    assert!(node.get("content_ref").is_none());
}

#[test]
fn canonical_content_ref_preserves_semantic_source() {
    let node_json = pact_canonical_component_json(
        "TextContent",
        r#"{
              "id":"txt",
              "content_ref":{
                "id":"cnt_ab12",
                "field":"content",
                "preview":"first chars",
                "source":{"kind":"file_snapshot","path":"/x/y.rs","hash":"blake3:abc"}
              }
            }"#,
    )
    .expect("content ref source preserved");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["content_ref"]["source"]["kind"], "file_snapshot");
    assert_eq!(node["content_ref"]["source"]["path"], "/x/y.rs");
    assert_eq!(node["content_ref"]["preview"], "first chars");
}

#[test]
fn large_visible_body_is_ref_not_inline() {
    let node_json = pact_canonical_component_json(
        "TextContent",
        r#"{
              "id":"txt",
              "content_ref":{
                "id":"cnt_large",
                "field":"content",
                "chars":4096,
                "preview":"bounded preview",
                "source":{"kind":"content_addressed","hash":"blake3:abc"}
              }
            }"#,
    )
    .expect("large visible body is represented as a ref");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert!(node.get("content").is_none());
    assert_eq!(node["content_ref"]["id"], "cnt_large");
    assert_eq!(node["content_ref"]["preview"], "bounded preview");
}

#[test]
fn file_snapshot_ref_carries_generic_source_metadata() {
    let node_json = pact_canonical_component_json(
        "TextContent",
        r#"{
              "id":"txt_file",
              "content_ref":{
                "id":"cnt_file",
                "field":"content",
                "preview":"fn main()",
                "source":{
                  "kind":"file_snapshot",
                  "path":"/workspace/src/main.rs",
                  "hash":"blake3:def",
                  "range":[0,4096],
                  "vcs":{"kind":"git","commit":"abc123"}
                }
              }
            }"#,
    )
    .expect("file snapshot source accepted");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["content_ref"]["source"]["kind"], "file_snapshot");
    assert_eq!(node["content_ref"]["source"]["hash"], "blake3:def");
    assert_eq!(node["content_ref"]["source"]["range"][1], 4096);
    assert_eq!(node["content_ref"]["source"]["vcs"]["kind"], "git");
}

#[test]
fn generated_artifact_ref_carries_generic_source_metadata() {
    let payload = serde_json::json!({
        "id": "result",
        "tool_call_id": "call_1",
        "tool_name": "generic",
        "success": true,
        "result_ref": {
            "id": "cnt_result",
            "source": {
                "kind": "generated_artifact",
                "hash": "blake3:abc",
                "bytes": 4096,
                "path": "/tmp/artifact.txt"
            }
        }
    });

    let node_json = pact_canonical_component_json("ToolResult", &payload.to_string())
        .expect("generated ref source accepted");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(node["result_ref"]["id"], "cnt_result");
    assert_eq!(node["result_ref"]["source"]["kind"], "generated_artifact");
}

#[test]
fn payload_slot_policy_rejects_undeclared_large_field() {
    let huge = "x".repeat(DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES + 1);
    let payload = serde_json::json!({
        "id": "custom",
        "componentType": "org:custom",
        "attrs": {
            "debug_blob": huge
        }
    });

    let err = pact_canonical_component_json("PACTComponent", &payload.to_string())
        .expect_err("large undeclared payload rejected");

    assert!(err.to_string().contains("$.debug_blob"));
    assert!(err.to_string().contains("content_ref"));
}

#[test]
fn extension_object_id_field_is_not_implicitly_a_content_ref() {
    let payload = serde_json::json!({
        "id": "custom",
        "componentType": "org:custom",
        "attrs": {
            "task_payload": {
                "id": "t1",
                "body": "x".repeat(DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES + 1)
            }
        }
    });

    let err = pact_canonical_component_json("PACTComponent", &payload.to_string())
        .expect_err("ordinary id-bearing extension objects are not refs");

    assert!(err.to_string().contains("$.task_payload"));
    assert!(err.to_string().contains("content_ref"));
}

#[test]
fn payload_slot_policy_spills_declared_large_field() {
    let huge_preview = "x".repeat(DEFAULT_PREVIEW_INLINE_LIMIT_BYTES);
    let payload = serde_json::json!({
        "id": "custom",
        "componentType": "org:custom",
        "attrs": {
            "debug_blob": {
                "content_ref": {
                    "id": "cnt_debug",
                    "preview_inline": huge_preview,
                    "source": {"kind": "generated_artifact", "hash": "blake3:abc"}
                }
            }
        }
    });

    let node_json = pact_canonical_component_json("PACTComponent", &payload.to_string())
        .expect("large declared ref accepted");
    let node: Value = serde_json::from_str(&node_json).expect("node json");

    assert_eq!(
        node["attrs"]["debug_blob"]["content_ref"]["id"],
        "cnt_debug"
    );
}

#[test]
fn payload_slot_policy_requires_source_for_large_tool_result_refs() {
    let payload = serde_json::json!({
        "id": "result",
        "tool_call_id": "call_1",
        "tool_name": "shell",
        "success": true,
        "result": "x".repeat(DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES + 1),
        "result_ref": {"id": "cnt_result"}
    });

    let err = pact_canonical_component_json("ToolResult", &payload.to_string())
        .expect_err("large generated result ref needs source");

    assert!(err.to_string().contains("$.result"));
    assert!(err.to_string().contains("source"));
}

#[test]
fn canonical_tool_call_requires_structured_parameters() {
    let err = pact_canonical_component_json(
        "ToolCall",
        r#"{"id":"call","tool_call_id":"tc_1","tool_name":"read"}"#,
    )
    .expect_err("parameters required");

    assert!(err.to_string().contains("$.parameters"));
}

#[test]
fn prepares_component_node_with_context_provenance() {
    let component = r#"{
          "id": "turn",
          "componentType": "message_turn",
          "children": [
            {"id": "text", "componentType": "text_content", "content": "hello"}
          ]
        }"#;
    let context = r#"{
          "next_creation_index": 7,
          "transaction_index": 3,
          "created_at_ns": 123456,
          "created_at_iso": "2026-05-19T00:00:00",
          "born_turn_index": 3,
          "transaction_id": "txn_1",
          "branch_id": "main",
          "source": "pact_insert"
        }"#;

    let prepared_json =
        component_prepare_for_context_json(component, context).expect("prepared node");
    let prepared: Value = serde_json::from_str(&prepared_json).expect("prepared json");

    assert_eq!(prepared["next_creation_index"], 9);
    assert_eq!(
        prepared["assigned_ids"],
        serde_json::json!(["turn", "text"])
    );
    assert_eq!(prepared["node"]["creation_index"], 7);
    assert_eq!(prepared["node"]["created_at_ns"], 123456);
    assert_eq!(prepared["node"]["transaction_index"], 3);
    assert_eq!(prepared["node"]["aux"]["born_turn_index"], 3);
    assert_eq!(prepared["node"]["provenance"]["transaction_id"], "txn_1");
    assert_eq!(prepared["node"]["provenance"]["branch_id"], "main");
    assert_eq!(prepared["node"]["children"][0]["creation_index"], 8);
}

#[test]
fn node_parses_reference_only_capture_and_file_source_ref() {
    let value = serde_json::json!({
        "id": "file_read_abc",
        "componentType": "text_content",
        "type": "text_content",
        "content": "the visible file text",
        "content_capture": "reference_only",
        "source_ref": {
            "kind": "file",
            "path": "/project/app.py",
            "byte_range": [0, 4096],
            "sha256": "abc123",
            "mtime_ns": 123,
            "size_bytes": 9821,
        }
    });
    let node = Component::from_component_value(&value).expect("parse");
    assert_eq!(
        node.content_capture(),
        Some(ContentCapturePolicy::ReferenceOnly)
    );
    let source_ref = node.source_ref().expect("source_ref");
    assert_eq!(source_ref.kind, "file");
    assert_eq!(source_ref.path.as_deref(), Some("/project/app.py"));
    assert_eq!(source_ref.byte_range, Some([0, 4096]));
    assert!(!node.content_capture().unwrap().persists_content());

    assert!(!node.aux.contains_key("content_capture"));
    assert!(!node.aux.contains_key("source_ref"));
}

#[test]
fn node_parses_capture_policy_promoted_from_attrs() {
    let value = serde_json::json!({
        "id": "n1",
        "componentType": "text_content",
        "attrs": {
            "content_capture": "redacted",
            "source_ref": {"kind": "file", "path": "/tmp/x"}
        }
    });
    let node = Component::from_component_value(&value).expect("parse");
    assert_eq!(node.content_capture(), Some(ContentCapturePolicy::Redacted));
    assert_eq!(node.source_ref().map(|s| s.kind.as_str()), Some("file"));
}

#[test]
fn invalid_content_capture_policy_is_rejected() {
    let value = serde_json::json!({
        "id": "bad",
        "componentType": "text_content",
        "content_capture": "purged",
    });
    let err = Component::from_component_value(&value).expect_err("must reject");
    assert!(
        format!("{err:?}").contains("content_capture"),
        "expected content_capture error, got {err:?}"
    );
}

#[test]
fn file_source_ref_requires_path() {
    let value = serde_json::json!({
        "id": "bad",
        "componentType": "text_content",
        "source_ref": {"kind": "file"},
    });
    let err = Component::from_component_value(&value).expect_err("must reject");
    assert!(
        format!("{err:?}").contains("source_ref"),
        "expected source_ref error, got {err:?}"
    );
}

#[test]
fn source_ref_byte_range_must_be_ascending() {
    let value = serde_json::json!({
        "id": "bad",
        "componentType": "text_content",
        "source_ref": {"kind": "file", "path": "/tmp/x", "byte_range": [10, 0]},
    });
    let err = Component::from_component_value(&value).expect_err("must reject");
    assert!(
        format!("{err:?}").contains("byte_range"),
        "expected byte_range error, got {err:?}"
    );
}

#[test]
fn runtime_pact_node_keeps_content_under_reference_only_capture() {
    let value = serde_json::json!({
        "id": "file_read",
        "componentType": "text_content",
        "content": "visible runtime text",
        "content_capture": "reference_only",
        "source_ref": {"kind": "file", "path": "/x"},
    });
    let node = Component::from_component_value(&value).expect("parse");
    assert_eq!(node.content(), Some("visible runtime text"));
    assert_eq!(
        node.content_capture(),
        Some(ContentCapturePolicy::ReferenceOnly)
    );

    let projected = node.project_component_value();
    assert_eq!(projected["content"], "visible runtime text");
}

#[test]
fn projection_round_trips_capture_policy_and_source_ref() {
    let value = serde_json::json!({
        "id": "n1",
        "componentType": "text_content",
        "content_capture": "reference_only",
        "source_ref": {"kind": "file", "path": "/x"},
    });
    let node = Component::from_component_value(&value).expect("parse");
    let projected = node.project_component_value();
    assert_eq!(projected["content_capture"], "reference_only");
    assert_eq!(projected["source_ref"]["kind"], "file");
    assert_eq!(projected["source_ref"]["path"], "/x");
}

#[test]
fn media_kind_classifies_canonical_and_custom() {
    let img = MediaPayload {
        media_kind: MediaKind::Image,
        ..Default::default()
    };
    assert_eq!(img.media_kind(), MediaKind::Image);

    let custom = MediaPayload {
        media_kind: MediaKind::Custom("hologram".to_string()),
        ..Default::default()
    };
    assert_eq!(
        custom.media_kind(),
        MediaKind::Custom("hologram".to_string())
    );
}

#[test]
fn typed_media_component_types_normalize_to_generic_media() {
    for (media_type, expected_kind, expected_kind_str) in [
        ("image_content", MediaKind::Image, "image"),
        ("audio_content", MediaKind::Audio, "audio"),
        ("video_content", MediaKind::Video, "video"),
        ("document_content", MediaKind::Document, "document"),
    ] {
        let value = serde_json::json!({
            "id": "m",
            "componentType": media_type,
            "content": "a caption",
        });
        let node = Component::from_component_value(&value).expect("parse typed media");
        match node.kind() {
            ComponentKind::Media(payload) => {
                assert_eq!(payload.media_kind, expected_kind);
            }
            other => panic!("expected Media kind, got {other:?}"),
        }
        let projected = node.project_component_value();
        assert_eq!(projected["componentType"], "media_content");
        assert_eq!(projected["media_kind"], expected_kind_str);
        assert_eq!(projected["content"], "a caption");
    }
}

#[test]
fn media_content_canonical_shape_round_trips() {
    let value = serde_json::json!({
        "id": "m",
        "componentType": "media_content",
        "media_kind": "image",
        "content": "base64-or-caption",
    });
    let node = Component::from_component_value(&value).expect("parse media_content");
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "media_content");
    assert_eq!(projected["media_kind"], "image");
    assert_eq!(projected["content"], "base64-or-caption");
}

#[test]
fn media_image_projects_generic_kind_with_inline_text_body() {
    let payload = MediaPayload {
        media_kind: MediaKind::Image,
        mime_type: Some("image/png".to_string()),
        label: Some("hero".to_string()),
        body: MediaBody::InlineText {
            data: "iVBORw0KGgo".to_string(),
            encoding: Some("base64".to_string()),
        },
        width: Some(640),
        height: Some(480),
        ..Default::default()
    };
    let node = component_with_kind("img", ComponentKind::Media(payload));
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "media_content");
    assert_eq!(projected["media_kind"], "image");
    assert_eq!(projected["content"], "iVBORw0KGgo");
    assert_eq!(projected["encoding"], "base64");
    assert_eq!(projected["mime_type"], "image/png");
    assert_eq!(projected["label"], "hero");
    assert_eq!(projected["width"], 640);
    assert_eq!(projected["height"], 480);

    let reparsed = Component::from_component_value(&projected).expect("reparse");
    assert_eq!(reparsed.kind(), node.kind());
}

#[test]
fn media_audio_projects_generic_kind_with_url_body() {
    let payload = MediaPayload {
        media_kind: MediaKind::Audio,
        mime_type: Some("audio/mpeg".to_string()),
        body: MediaBody::Url {
            url: "https://example.com/clip.mp3".to_string(),
        },
        duration_ms: Some(42_000),
        ..Default::default()
    };
    let node = component_with_kind("aud", ComponentKind::Media(payload));
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "media_content");
    assert_eq!(projected["media_kind"], "audio");
    assert_eq!(projected["url"], "https://example.com/clip.mp3");
    assert_eq!(projected["duration_ms"], 42_000);
    let reparsed = Component::from_component_value(&projected).expect("reparse");
    assert_eq!(reparsed.kind(), node.kind());
}

#[test]
fn media_video_projects_generic_kind_with_inline_bytes_body() {
    let payload = MediaPayload {
        media_kind: MediaKind::Video,
        mime_type: Some("video/mp4".to_string()),
        body: MediaBody::InlineBytes {
            bytes: vec![0, 1, 2, 3, 255],
            encoding: None,
        },
        width: Some(1920),
        height: Some(1080),
        ..Default::default()
    };
    let node = component_with_kind("vid", ComponentKind::Media(payload));
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "media_content");
    assert_eq!(projected["media_kind"], "video");
    assert_eq!(projected["bytes"], serde_json::json!([0, 1, 2, 3, 255]));
    let reparsed = Component::from_component_value(&projected).expect("reparse");
    assert_eq!(reparsed.kind(), node.kind());
}

#[test]
fn media_document_projects_generic_kind_with_content_ref_body() {
    let payload = MediaPayload {
        media_kind: MediaKind::Document,
        mime_type: Some("application/pdf".to_string()),
        filename: Some("spec.pdf".to_string()),
        body: MediaBody::ContentRef(PactContentRef {
            id: "store-123".to_string(),
            field: None,
            chars: None,
            preview: None,
            source: None,
        }),
        page_count: Some(12),
        ..Default::default()
    };
    let node = component_with_kind("doc", ComponentKind::Media(payload));
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "media_content");
    assert_eq!(projected["media_kind"], "document");
    assert_eq!(projected["filename"], "spec.pdf");
    assert_eq!(projected["page_count"], 12);
    assert_eq!(projected["content_ref"]["id"], "store-123");
    let reparsed = Component::from_component_value(&projected).expect("reparse");
    assert_eq!(reparsed.kind(), node.kind());
}

#[test]
fn media_custom_family_projects_generic_kind_without_new_variant() {
    let payload = MediaPayload {
        media_kind: MediaKind::Custom("cad_mesh".to_string()),
        mime_type: Some("model/stl".to_string()),
        label: Some("bracket".to_string()),
        body: MediaBody::ExternalRef(ExternalRef {
            kind: "s3".to_string(),
            uri: Some("s3://bucket/bracket.stl".to_string()),
            role: None,
            data: None,
        }),
        ..Default::default()
    };
    let node = component_with_kind("cad", ComponentKind::Media(payload));
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "media_content");
    assert_eq!(projected["media_kind"], "cad_mesh");
    assert_eq!(projected["label"], "bracket");
    assert_eq!(projected["external_ref"]["kind"], "s3");
    assert_eq!(projected["external_ref"]["uri"], "s3://bucket/bracket.stl");
    let reparsed = Component::from_component_value(&projected).expect("reparse");
    match reparsed.kind() {
        ComponentKind::Media(p) => {
            assert_eq!(p.media_kind, MediaKind::Custom("cad_mesh".to_string()));
        }
        other => panic!("expected Media kind, got {other:?}"),
    }
    assert_eq!(reparsed.kind(), node.kind());
}

#[test]
fn media_unknown_extension_fields_fold_into_media_aux() {
    let value = serde_json::json!({
        "id": "m",
        "componentType": "media_content",
        "media_kind": "image",
        "url": "https://example.com/x.png",
        "vendor_tag": "acme",
    });
    let node = Component::from_component_value(&value).expect("parse");
    match node.kind() {
        ComponentKind::Media(payload) => {
            assert_eq!(
                payload.aux.get("vendor_tag").and_then(|v| v.as_str()),
                Some("acme")
            );
        }
        other => panic!("expected Media kind, got {other:?}"),
    }
    let projected = node.project_component_value();
    assert_eq!(projected["vendor_tag"], "acme");
    assert_eq!(projected["url"], "https://example.com/x.png");
}

fn component_with_kind(id: &str, kind: ComponentKind) -> Component {
    Component {
        id: id.to_string(),
        subtype: None,
        parent_id: None,
        role: None,
        offset: 0,
        pact_depth: None,
        priority: 0,
        transaction_index: 0,
        created_at_ns: 0,
        created_at_iso: None,
        creation_index: 0,
        key: None,
        tags: Vec::new(),
        aux: PactAttrs::new(),
        kind,
        provenance: None,
        lifecycle: ComponentLifecycle::default(),
        external_refs: Vec::new(),
        children: Vec::new(),
    }
}

#[test]
fn guard_component_type_is_derived_only_from_kind() {
    let text = component_with_kind(
        "t",
        ComponentKind::Text(TextPayload {
            content: Some("hi".to_string()),
            ..Default::default()
        }),
    );
    assert_eq!(text.component_type(), "text_content");
    let projected = text.project_component_value();
    assert_eq!(projected["componentType"], "text_content");

    let call = component_with_kind(
        "tc",
        ComponentKind::ToolCall(ToolCallPayload {
            tool_call_id: "call_1".to_string(),
            tool_name: "search".to_string(),
            ..Default::default()
        }),
    );
    assert_eq!(call.component_type(), "tool_call");
    assert_eq!(call.project_component_value()["componentType"], "tool_call");
}

#[test]
fn guard_ingested_tool_call_projects_tool_call_and_matches_kind() {
    let node = Component::from_component_value(&serde_json::json!({
        "id": "tc", "componentType": "tool_call",
        "tool_call_id": "call_1", "tool_name": "search",
        "parameters": {"q": "x"}
    }))
    .expect("tool_call");
    assert!(matches!(node.kind(), ComponentKind::ToolCall(_)));
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "tool_call");
}

#[test]
fn guard_custom_component_type_round_trips_through_unknown_kind() {
    let node = Component::from_component_value(&serde_json::json!({
        "id": "sh", "componentType": "shell",
        "content": "echo hi"
    }))
    .expect("shell component");
    assert!(matches!(node.kind(), ComponentKind::Unknown(_)));
    assert_eq!(node.component_type(), "shell");
    let projected = node.project_component_value();
    assert_eq!(projected["componentType"], "shell");

    let reparsed = Component::from_component_value(&projected).expect("reparse shell");
    assert!(matches!(reparsed.kind(), ComponentKind::Unknown(_)));
    assert_eq!(reparsed.component_type(), "shell");
}

#[test]
fn guard_normal_projection_emits_componenttype_not_nodetype_or_attrs() {
    let node = Component::from_component_value(&serde_json::json!({
        "id": "g", "componentType": "text_content", "content": "hi",
        "custom_ext": {"k": 1}
    }))
    .expect("node");
    let projected = node.project_component_value();
    let obj = projected.as_object().expect("object");
    assert_eq!(
        obj.get("componentType").and_then(Value::as_str),
        Some("text_content")
    );
    assert!(
        !obj.contains_key("nodeType"),
        "projection must not emit nodeType"
    );
    assert!(!obj.contains_key("attrs"), "projection must not emit attrs");
    assert_eq!(projected["aux"]["custom_ext"]["k"], 1);
}

#[test]
fn top_level_org_folds_into_aux() {
    let node = Component::from_component_value(&serde_json::json!({
        "id": "org_fold",
        "componentType": "text_content",
        "content": "hi",
        "org": {
            "acme": {"view_kind": "review"}
        }
    }))
    .expect("component");

    let projected = node.project_component_value();
    let obj = projected.as_object().expect("object");
    assert!(
        !obj.contains_key("org"),
        "org must not survive as a top-level component lane"
    );
    assert_eq!(projected["aux"]["org"]["acme"]["view_kind"], "review");
}

#[test]
fn guard_no_node_vocabulary_in_production_sources() {
    fn scan(dir: &std::path::Path, forbidden: &[&str], hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                scan(&path, forbidden, hits);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            for tok in forbidden {
                if text.contains(tok) {
                    hits.push(format!("{}: {tok}", path.display()));
                }
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        "NodeRecord",
        "PactComponentRecord",
        "PactTreeNode",
        "SelectedNode",
        "target_node_id",
        "changed_node_ids",
        "root-fork-node",
        "fork-node",
        "load nodes",
        "produced_node_ids",
        "replaced_node_ids",
        "deleted_node_ids",
        "affected_node_ids",
        "ComponentRegistry",
        concat!("file_", "material", "ize_nodes_json"),
    ];
    let mut hits = Vec::new();
    scan(&src, &forbidden, &mut hits);
    assert!(
        hits.is_empty(),
        "forbidden node vocabulary reintroduced in production sources: {hits:?}"
    );
}

#[test]
fn authored_node_is_stamped_with_creation_time_at_instantiation() {
    let before = now_created_at_ns();
    let node = Component::text_authoring("hello".to_string(), AuthoringComponentOptions::default());
    let after = now_created_at_ns();

    assert!(
        node.created_at_ns > 0,
        "an instantiated node must carry a non-zero creation time"
    );
    assert!(
        node.created_at_ns >= before && node.created_at_ns <= after,
        "creation time {} must fall within the construction window [{before}, {after}]",
        node.created_at_ns,
    );

    let later =
        Component::text_authoring("world".to_string(), AuthoringComponentOptions::default());
    assert!(
        later.created_at_ns >= node.created_at_ns,
        "later-minted node {} must not predate earlier one {}",
        later.created_at_ns,
        node.created_at_ns,
    );

    let reasoning =
        Component::reasoning_authoring("why".to_string(), AuthoringComponentOptions::default());
    assert!(reasoning.created_at_ns > 0);
}
