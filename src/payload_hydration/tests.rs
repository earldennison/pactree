use super::*;
use serde_json::json;

#[test]
fn hydrates_ref_backed_tool_parameters_as_json() {
    let mut content_store = ContentStore::default();
    let record = content_store.put_inline("node.extension", r#"{"content":"hello"}"#, 16);
    let mut tree = json!({
        "id": "call_1",
        "componentType": "tool_call",
        "parameters_ref": {"id": record.id, "field": "parameters"}
    });

    hydrate_payload_refs(&mut tree, &content_store).expect("hydrate");

    assert_eq!(tree["parameters"]["content"], "hello");
}

#[test]
fn leaves_tool_parameter_id_fields_unhydrated() {
    let content_store = ContentStore::default();
    let mut tree = json!({
        "id": "call_1",
        "componentType": "tool_call",
        "parameters": {"id": "t1", "status": "in_progress"}
    });

    hydrate_payload_refs(&mut tree, &content_store)
        .expect("plain tool parameter id is not a content ref");

    assert_eq!(tree["parameters"]["id"], "t1");
    assert_eq!(tree["parameters"]["status"], "in_progress");
}

#[test]
fn leaves_tool_parameter_content_ref_fields_unhydrated() {
    let content_store = ContentStore::default();
    let mut tree = json!({
        "id": "call_1",
        "componentType": "tool_call",
        "parameters": {
            "content_ref": "literal tool argument, not Pact storage",
            "path": "/tmp/example"
        }
    });

    hydrate_payload_refs(&mut tree, &content_store)
        .expect("tool parameters are ordinary JSON unless parameters_ref is used");

    assert_eq!(
        tree["parameters"]["content_ref"],
        "literal tool argument, not Pact storage"
    );
    assert_eq!(tree["parameters"]["path"], "/tmp/example");
}

#[test]
fn rejects_missing_valid_looking_content_ref_ids() {
    let content_store = ContentStore::default();
    let mut tree = json!({
        "id": "text_1",
        "componentType": "text_content",
        "content_ref": "cnt_missing"
    });

    let err = hydrate_payload_refs(&mut tree, &content_store)
        .expect_err("valid-looking missing refs must still fail");

    assert!(err.to_string().contains("cnt_missing"));
}

#[test]
fn leaves_plain_tool_error_strings_unhydrated() {
    let content_store = ContentStore::default();
    let message =
        "Parameter validation failed for execute: got an unexpected keyword argument 'max_chars'";
    let mut tree = json!({
        "id": "result_1",
        "componentType": "tool_result",
        "tool_call_id": "call_1",
        "tool_name": "shell_execute",
        "success": false,
        "content": message,
        "error_message": message
    });

    hydrate_payload_refs(&mut tree, &content_store)
        .expect("plain content/error strings are payloads, not refs");

    assert_eq!(tree["content"], message);
    assert_eq!(tree["error_message"], message);
}

#[test]
fn hydrates_explicit_tool_error_ref() {
    let mut content_store = ContentStore::default();
    let record = content_store.put_inline("node.extension", "validation failed", 16);
    let mut tree = json!({
        "id": "result_1",
        "componentType": "tool_result",
        "tool_call_id": "call_1",
        "tool_name": "shell_execute",
        "success": false,
        "error_ref": {"id": record.id, "field": "error_message"}
    });

    hydrate_payload_refs(&mut tree, &content_store).expect("hydrate explicit error ref");

    assert_eq!(tree["error_message"], "validation failed");
}
