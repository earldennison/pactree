use super::*;
use crate::store::ContentStore;
use crate::tree::ComponentTree;

fn tree_with_one_user_message() -> Value {
    json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "user",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "container",
                        "componentType": "message_container",
                        "type": "message_container",
                        "pact_depth": 0,
                        "children": [
                            {
                                "id": "text_a",
                                "componentType": "text_content",
                                "content": "hello world"
                            }
                        ]
                    }
                ]
            }
        ]
    })
}

#[test]
fn projects_single_user_message() {
    let thread = build_provider_thread(
        &tree_with_one_user_message(),
        &ProviderThreadOptions::default(),
    )
    .expect("project");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].get("message_type").and_then(Value::as_str),
        Some("client_request")
    );
    let content = messages[0]
        .get("content")
        .and_then(Value::as_array)
        .expect("content");
    assert_eq!(content.len(), 1);
    assert_eq!(
        content[0].get("content").and_then(Value::as_str),
        Some("hello world")
    );
}

#[test]
fn tree_renderer_matches_json_adapter_for_canonical_user_text() {
    let value = tree_with_one_user_message();
    let tree = ComponentTree::from_value(&value).expect("component tree");

    let from_value =
        build_provider_thread(&value, &ProviderThreadOptions::default()).expect("value");
    let from_tree =
        build_provider_thread_tree(&tree, &ProviderThreadOptions::default()).expect("tree");

    assert_eq!(from_tree, from_value);
}

#[test]
fn projects_reasoning_content_as_reasoning_block_not_text() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "assistant",
            "componentType": "message_turn",
            "role": "assistant",
            "pact_depth": 0,
            "children": [{
                "id": "reasoning",
                "componentType": "reasoning_content",
                "content": "private but displayable",
                "aux": {"visibility": "displayable", "provider": "deepseek"}
            }]
        }]
    });
    let opts = ProviderThreadOptions {
        include_partial_tool_calls: true,
        fail_on_orphan_tool_calls: false,
    };
    let thread = build_provider_thread(&tree, &opts).expect("project");
    let content = thread["messages"][0]["content"]
        .as_array()
        .expect("content");

    assert_eq!(content[0]["type"].as_str(), Some("reasoning_content"));
    assert_eq!(content[0]["visibility"].as_str(), Some("displayable"));
    assert_eq!(
        content[0]["content"].as_str(),
        Some("private but displayable")
    );
}

#[test]
fn projects_opaque_reasoning_without_visible_text() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "assistant",
            "componentType": "message_turn",
            "role": "assistant",
            "pact_depth": 0,
            "children": [{
                "id": "reasoning",
                "componentType": "reasoning_content",
                "content": "must not leak",
                "data": "encrypted-payload",
                "aux": {
                    "visibility": "opaque",
                    "provider": "openai",
                    "encoding": "provider_encrypted"
                }
            }]
        }]
    });
    let opts = ProviderThreadOptions {
        include_partial_tool_calls: true,
        fail_on_orphan_tool_calls: false,
    };
    let thread = build_provider_thread(&tree, &opts).expect("project");
    let content = thread["messages"][0]["content"]
        .as_array()
        .expect("content");

    assert_eq!(content[0]["type"].as_str(), Some("reasoning_content"));
    assert_eq!(content[0]["visibility"].as_str(), Some("opaque"));
    assert_eq!(content[0]["content"].as_str(), Some(""));
    assert_eq!(content[0]["data"].as_str(), Some("encrypted-payload"));
}

#[test]
fn provider_projection_hydrates_visible_content_refs() {
    let mut content_store = ContentStore::default();
    let body = "visible bytes ".repeat(200);
    let record = content_store.put_inline("text", &body, 80);
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "active_message",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "children": [{
                "id": "txt_ref",
                "componentType": "text_content",
                "content_ref": record.id,
            }]
        }]
    });

    let thread = build_provider_thread_with_content_store(
        &tree,
        &content_store,
        &ProviderThreadOptions::default(),
    )
    .expect("hydrate provider-visible text ref");

    assert_eq!(thread["messages"][0]["content"][0]["content"], body);
    assert!(
        tree["children"][0]["children"][0].get("content").is_none(),
        "hydration must not mutate the hot canonical tree"
    );
}

#[test]
fn provider_projection_rejects_unresolved_visible_content_refs() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "active_message",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "children": [{
                "id": "txt_ref",
                "componentType": "text_content",
                "content_ref": "cnt_missing",
            }]
        }]
    });

    let err = build_provider_thread_with_content_store(
        &tree,
        &ContentStore::default(),
        &ProviderThreadOptions::default(),
    )
    .expect_err("missing visible ref is not silently omitted");

    assert!(err.reason.contains("cnt_missing"));
}

#[test]
fn refuses_to_emit_with_orphan_tool_calls() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "active_message",
            "type": "message_turn",
            "role": "assistant",
            "pact_depth": 0,
            "children": [{
                "id": "tc_orphan_node",
                "componentType": "tool_call",
                "tool_call_id": "tc_orphan",
                "tool_name": "shell",
                "parameters": {"cmd": "x"}
            }]
        }]
    });
    let err = build_provider_thread(&tree, &ProviderThreadOptions::default()).unwrap_err();
    assert!(err.reason.contains("unpaired"));
    assert_eq!(err.offending_tool_call_ids, vec!["tc_orphan".to_string()]);
}

#[test]
fn allows_partial_when_opt_in() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "active_message",
            "type": "message_turn",
            "role": "assistant",
            "pact_depth": 0,
            "children": [{
                "id": "tc_part_node",
                "componentType": "tool_call",
                "tool_call_id": "tc_part",
                "tool_name": "shell",
                "parameters": {"cmd": "x"}
            }]
        }]
    });
    let opts = ProviderThreadOptions {
        include_partial_tool_calls: true,
        fail_on_orphan_tool_calls: false,
    };
    let _thread = build_provider_thread(&tree, &opts).expect("partial-allowed");
}

#[test]
fn skips_partial_tool_call_block_by_default() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "assistant",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "tc_skip_node",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "tc_skip",
                        "aux": {
                            "tool_call_id": "tc_skip",
                            "tool_name": "shell",
                            "parameters": {"cmd": "x"}
                        }
                    }
                ]
            }
        ]
    });
    let opts = ProviderThreadOptions {
        include_partial_tool_calls: false,
        fail_on_orphan_tool_calls: false,
    };
    let thread = build_provider_thread(&tree, &opts).expect("project");

    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    assert!(messages.is_empty());
}

#[test]
fn emits_result_for_tree_paired_tool_call() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "history",
                "componentType": "conversation_history",
                "children": [
                    {
                        "id": "assistant_turn",
                        "componentType": "message_turn",
                        "role": "assistant",
                        "pact_depth": 1,
                        "children": [
                            {
                                "id": "call",
                                "componentType": "tool_call",
                                "tool_call_id": "tc_suppressed",
                                "aux": {
                                    "tool_call_id": "tc_suppressed",
                                    "tool_name": "shell",
                                    "parameters": {"command": "pwd"}
                                }
                            }
                        ]
                    },
                    {
                        "id": "tool_turn",
                        "componentType": "message_turn",
                        "role": "user",
                        "pact_depth": 0,
                        "children": [
                            {
                                "id": "result",
                                "componentType": "tool_result",
                                "tool_call_id": "tc_suppressed",
                                "content": "/tmp/project",
                                "aux": {
                                    "tool_call_id": "tc_suppressed",
                                    "tool_name": "shell",
                                    "success": true
                                }
                            }
                        ]
                    }
                ]
            }
        ]
    });
    let opts = ProviderThreadOptions {
        include_partial_tool_calls: false,
        fail_on_orphan_tool_calls: false,
    };
    let thread = build_provider_thread(&tree, &opts).expect("project");
    let messages = thread["messages"].as_array().expect("messages");

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["message_type"], "provider_response");
    assert_eq!(messages[1]["message_type"], "client_request");
    assert_eq!(
        messages[1]["content"][0]["tool_call_id"].as_str(),
        Some("tc_suppressed")
    );
}

#[test]
fn projects_assistant_message_with_paired_tool_call() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "assistant",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "say",
                        "componentType": "text_content",
                        "content": "running it"
                    },
                    {
                        "id": "tc_p_node",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "tc_p",
                        "aux": {
                            "tool_call_id": "tc_p",
                            "tool_name": "shell",
                            "parameters": {"cmd": "ls"}
                        }
                    }
                ]
            }
        ]
    });
    let opts = ProviderThreadOptions {
        include_partial_tool_calls: true,
        fail_on_orphan_tool_calls: false,
    };
    let thread = build_provider_thread(&tree, &opts).expect("project");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].get("message_type").and_then(Value::as_str),
        Some("provider_response")
    );
    let content = messages[0]
        .get("content")
        .and_then(Value::as_array)
        .expect("content");
    assert_eq!(content.len(), 2);
    assert!(content[0].get("content").is_some());
    assert_eq!(
        content[1].get("tool_call_id").and_then(Value::as_str),
        Some("tc_p")
    );
}

#[test]
fn splits_assistant_turn_tool_result_into_following_client_request() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "assistant",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "tr_node",
                        "componentType": "tool_result",
                        "type": "tool_result",
                        "tool_call_id": "tc_split",
                        "content": "ok",
                        "aux": {
                            "tool_call_id": "tc_split",
                            "success": true
                        }
                    },
                    {
                        "id": "tc_node",
                        "componentType": "tool_call",
                        "type": "tool_call",
                        "tool_call_id": "tc_split",
                        "aux": {
                            "tool_call_id": "tc_split",
                            "tool_name": "shell",
                            "parameters": {"cmd": "pwd"}
                        }
                    }
                ]
            }
        ]
    });

    let thread = build_provider_thread(&tree, &ProviderThreadOptions::default()).expect("project");
    let messages = thread["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["message_type"], "provider_response");
    assert_eq!(messages[1]["message_type"], "client_request");
    assert_eq!(
        messages[0]["content"][0]["tool_call_id"].as_str(),
        Some("tc_split")
    );
    assert_eq!(
        messages[1]["content"][0]["tool_call_id"].as_str(),
        Some("tc_split")
    );
    assert_eq!(messages[1]["content"][0]["message"].as_str(), Some("ok"));
}

#[test]
fn ignores_removed_surface_escape_hatch_from_node() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "assistant",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "say",
                        "componentType": "text_content",
                        "content": "thinking out loud",
                        "aux": {
                            "vendor_private_text": "private chain"
                        }
                    }
                ]
            }
        ]
    });

    let thread = build_provider_thread(&tree, &ProviderThreadOptions::default()).expect("project");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    assert!(messages[0].get("metadata").is_none());
}

#[test]
fn projects_tool_shaped_components_with_aux_payloads() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "history",
                "componentType": "conversation_history",
                "children": [
                    {
                        "id": "assistant_turn",
                        "componentType": "message_turn",
                        "role": "assistant",
                        "pact_depth": 1,
                        "children": [
                            {
                                "id": "assistant_text",
                                "componentType": "text_content",
                                "content": "checking"
                            },
                            {
                                "id": "call",
                                "componentType": "tool_call",
                                "tool_call_id": "tc_shape",
                                "tool_name": "shell_execute",
                                "parameters": {"command": "pwd"},
                                "aux": {
                                    "tool_call_id": "tc_shape"
                                }
                            }
                        ]
                    },
                    {
                        "id": "tool_turn",
                        "componentType": "message_turn",
                        "role": "user",
                        "pact_depth": 0,
                        "children": [
                            {
                                "id": "result",
                                "componentType": "tool_result",
                                "tool_call_id": "tc_shape",
                                "tool_name": "shell_execute",
                                "content": "/tmp/project",
                                "aux": {
                                    "tool_call_id": "tc_shape",
                                    "success": true
                                }
                            }
                        ]
                    }
                ]
            }
        ]
    });

    let thread = build_provider_thread(&tree, &ProviderThreadOptions::default()).expect("project");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    assert_eq!(messages.len(), 2);
    let assistant_content = messages[0]
        .get("content")
        .and_then(Value::as_array)
        .expect("assistant content");
    assert_eq!(
        assistant_content[1]
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("tc_shape")
    );
    assert_eq!(
        assistant_content[1]
            .get("tool_name")
            .and_then(Value::as_str),
        Some("shell_execute")
    );
    assert_eq!(
        assistant_content[1]
            .get("parameters")
            .and_then(|v| v.get("command"))
            .and_then(Value::as_str),
        Some("pwd")
    );
    let tool_content = messages[1]
        .get("content")
        .and_then(Value::as_array)
        .expect("tool content");
    assert_eq!(
        tool_content[0].get("tool_call_id").and_then(Value::as_str),
        Some("tc_shape")
    );
    assert_eq!(
        tool_content[0].get("message").and_then(Value::as_str),
        Some("/tmp/project")
    );
    let keys = tool_content[0]
        .as_object()
        .expect("tool response object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "message".to_string(),
            "success".to_string(),
            "tool_call_id".to_string(),
            "tool_name".to_string(),
        ])
    );
}

#[test]
fn renders_structured_tool_result_when_content_is_derived() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "history",
                "componentType": "conversation_history",
                "children": [{
                    "id": "assistant_turn",
                    "componentType": "message_turn",
                    "role": "assistant",
                    "pact_depth": 1,
                    "children": [{
                        "id": "call",
                        "componentType": "tool_call",
                        "tool_call_id": "tc_structured",
                        "aux": {
                            "tool_call_id": "tc_structured",
                            "tool_name": "generic",
                            "parameters": {}
                        }
                    }]
                }]
            },
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "user",
                "pact_depth": 0,
                "children": [{
                    "id": "result",
                    "componentType": "tool_result",
                    "tool_call_id": "tc_structured",
                    "aux": {
                        "tool_call_id": "tc_structured",
                        "tool_name": "generic",
                        "success": true,
                        "result": {
                            "stdout": "/tmp",
                            "exit_code": 0
                        }
                    }
                }]
            }
        ]
    });

    let thread = build_provider_thread(&tree, &ProviderThreadOptions::default()).expect("project");
    let message = thread["messages"][1]["content"][0]["message"]
        .as_str()
        .expect("message");
    assert!(message.contains("\"stdout\":\"/tmp\""));
    assert!(message.contains("\"exit_code\":0"));
}

#[test]
fn projects_real_media_refs_as_multimedia_blocks() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "user",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "img",
                        "componentType": "media_content",
                        "media_kind": "image",
                        "label": "reference image",
                        "url": "https://example.com/image.png",
                        "mime_type": "image/png",
                        "width": 640,
                        "height": 480
                    },
                    {
                        "id": "doc",
                        "componentType": "media_content",
                        "media_kind": "document",
                        "content": "spec",
                        "mime_type": "application/pdf",
                        "filename": "spec.pdf",
                        "page_count": 3,
                        "aux": {
                            "media_content": {"data": "cGRm", "encoding": "base64"}
                        }
                    }
                ]
            }
        ]
    });

    let thread = build_provider_thread(&tree, &ProviderThreadOptions::default()).expect("project");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    let content = messages[0]
        .get("content")
        .and_then(Value::as_array)
        .expect("content");
    let image = content
        .iter()
        .find(|block| {
            block
                .get("media_content")
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
                == Some("https://example.com/image.png")
        })
        .expect("image media block");
    let document = content
        .iter()
        .find(|block| {
            block
                .get("media_content")
                .and_then(|value| value.get("data"))
                .and_then(Value::as_str)
                == Some("cGRm")
        })
        .expect("document media block");
    assert_eq!(
        image
            .get("media_content")
            .and_then(|v| v.get("url"))
            .and_then(Value::as_str),
        Some("https://example.com/image.png")
    );
    assert_eq!(
        image.get("mime_type").and_then(Value::as_str),
        Some("image/png")
    );
    assert_eq!(image.get("width").and_then(Value::as_i64), Some(640));
    assert_eq!(
        document
            .get("media_content")
            .and_then(|v| v.get("data"))
            .and_then(Value::as_str),
        Some("cGRm")
    );
    assert_eq!(
        document.get("filename").and_then(Value::as_str),
        Some("spec.pdf")
    );
}

#[test]
fn media_nodes_without_real_refs_fall_back_to_text() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [
            {
                "id": "active",
                "componentType": "active_message",
                "type": "message_turn",
                "role": "user",
                "pact_depth": 0,
                "children": [
                    {
                        "id": "img_desc",
                        "componentType": "media_content",
                        "media_kind": "image",
                        "content": "a sketch of the interface"
                    }
                ]
            }
        ]
    });

    let thread = build_provider_thread(&tree, &ProviderThreadOptions::default()).expect("project");
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");
    let content = messages[0]
        .get("content")
        .and_then(Value::as_array)
        .expect("content");
    assert_eq!(
        content[0].get("content").and_then(Value::as_str),
        Some("a sketch of the interface")
    );
    assert!(content[0].get("media_content").is_none());
}
