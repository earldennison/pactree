use crate::components::{
    Component, ComponentKind, DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES, PactContentRef,
};
use crate::error::{PactError, PactResult};
use crate::mutation::MutationRequest;
use crate::store::ContentStore;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn externalize_mutation_payloads(
    content: &mut ContentStore,
    request: &mut MutationRequest,
) -> PactResult<()> {
    if let Some(node) = request.node.as_mut() {
        externalize_node_payloads(content, node)?;
    }
    if let Some(text) = request.content.as_ref()
        && text.len() > DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES
        && request.content_ref.is_none()
    {
        request.content_ref = Some(content_ref_for_text(
            content,
            "mutation.content",
            "content",
            text,
        ));
        request.content = None;
    }
    externalize_extension_map(content, &mut request.attrs)?;
    Ok(())
}

pub(crate) fn externalize_node_payloads(
    content: &mut ContentStore,
    node: &mut Component,
) -> PactResult<()> {
    if let Some(text) = node.content().map(str::to_string)
        && !text.is_empty()
    {
        remove_duplicate_text_field(&mut node.aux, &text);

        if text.len() > DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES && !node.has_content_ref() {
            let cref = content_ref_for_text(content, "node.content", "content", &text);
            node.externalize_content(cref);
        }
    }

    if let ComponentKind::ToolCall(p) = &mut node.kind
        && let Some(parameters) = p.parameters.as_ref()
        && payload_value_bytes(parameters) > DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES
        && p.parameters_ref.is_none()
    {
        let encoded = serde_json::to_string(parameters).map_err(|err| {
            PactError::codec(format!("failed to encode tool parameters payload: {err}"))
        })?;
        p.parameters_ref = Some(
            serde_json::to_value(content_ref_for_text(
                content,
                "node.parameters",
                "parameters",
                &encoded,
            ))
            .map_err(|err| {
                PactError::codec(format!("failed to encode parameters content ref: {err}"))
            })?,
        );
        p.parameters = None;
    }

    if let ComponentKind::ToolResult(p) = &mut node.kind {
        if let Some(result) = p.result.as_ref()
            && payload_value_bytes(result) > DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES
            && p.result_ref.is_none()
        {
            let encoded = serde_json::to_string(result).map_err(|err| {
                PactError::codec(format!("failed to encode tool result payload: {err}"))
            })?;
            p.result_ref = Some(
                serde_json::to_value(content_ref_for_text(
                    content,
                    "node.result",
                    "result",
                    &encoded,
                ))
                .map_err(|err| {
                    PactError::codec(format!("failed to encode result content ref: {err}"))
                })?,
            );
            p.result = None;
        }
        if let Some(error_message) = p.error_message.as_ref()
            && error_message.len() > DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES
            && p.error_ref.is_none()
        {
            p.error_ref = Some(
                serde_json::to_value(content_ref_for_text(
                    content,
                    "node.error",
                    "error_message",
                    error_message,
                ))
                .map_err(|err| {
                    PactError::codec(format!("failed to encode error content ref: {err}"))
                })?,
            );
            p.error_message = None;
        }
    }
    externalize_extension_map(content, &mut node.aux)?;
    for child in node.children.iter_mut() {
        externalize_node_payloads(content, child)?;
    }
    Ok(())
}

pub(super) fn remove_duplicate_text_field(map: &mut BTreeMap<String, Value>, content: &str) {
    if map.get("text").and_then(Value::as_str) == Some(content) {
        map.remove("text");
    }
}

pub(super) fn payload_value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        _ => serde_json::to_vec(value).map_or(0, |bytes| bytes.len()),
    }
}

pub(super) fn externalize_extension_value(
    content: &mut ContentStore,
    value: &mut Value,
    field: &str,
) -> PactResult<()> {
    if value_is_refish(value) {
        return Ok(());
    }
    match value {
        Value::String(text) => {
            if text.len() <= DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES {
                return Ok(());
            }
            let content_ref = content_ref_for_text(content, "node.extension", field, text);
            *value = json!({ "content_ref": content_ref });
        }
        Value::Array(_) | Value::Object(_) => {
            let encoded = serde_json::to_string(value).map_err(|err| {
                PactError::codec(format!("failed to encode extension payload: {err}"))
            })?;
            if encoded.len() <= DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES {
                return Ok(());
            }
            let content_ref = content_ref_for_text(content, "node.extension", field, &encoded);
            *value = json!({ "content_ref": content_ref });
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn externalize_extension_map(
    content: &mut ContentStore,
    map: &mut BTreeMap<String, Value>,
) -> PactResult<()> {
    let inline_fields = inline_payload_policy_fields(map);
    for (key, value) in map.iter_mut() {
        if inline_fields.contains(key) {
            continue;
        }
        externalize_extension_value(content, value, key)?;
    }
    Ok(())
}

pub(super) fn inline_payload_policy_fields(map: &BTreeMap<String, Value>) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    for policies_key in ["payload_slot_policies", "payload_slots"] {
        let Some(Value::Object(policies)) = map.get(policies_key) else {
            continue;
        };
        for (field, policy) in policies {
            if payload_policy_declares_inline(policy) {
                fields.insert(canonical_payload_policy_field(field));
            }
        }
    }
    fields
}

pub(super) fn payload_policy_declares_inline(policy: &Value) -> bool {
    let Some(object) = policy.as_object() else {
        return policy.as_str() == Some("inline");
    };
    ["strategy", "mode", "overflow", "storage", "externalization"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .any(|value| value == "inline" || value == "inline_required")
}

pub(super) fn canonical_payload_policy_field(field: &str) -> String {
    field
        .trim_start_matches("$.")
        .rsplit('.')
        .next()
        .unwrap_or(field)
        .to_string()
}

pub(super) fn value_is_refish(value: &Value) -> bool {
    if value.as_str().is_some_and(|text| {
        text.starts_with("cnt_") || text.starts_with("blob_") || text.starts_with("ref_")
    }) {
        return true;
    }
    value.as_object().is_some_and(|object| {
        object.contains_key("content_ref")
            || object.contains_key("external_ref")
            || object.get("id").and_then(Value::as_str).is_some_and(|id| {
                (id.starts_with("cnt_") || id.starts_with("blob_") || id.starts_with("ref_"))
                    && (object.contains_key("source")
                        || object.contains_key("field")
                        || object.contains_key("preview")
                        || object.contains_key("preview_inline")
                        || object.contains_key("chars"))
            })
    })
}

pub(super) fn content_ref_for_text(
    content: &mut ContentStore,
    kind: &str,
    field: &str,
    text: &str,
) -> PactContentRef {
    let record = content.put_inline(kind, text, 512);
    PactContentRef {
        id: record.id,
        field: Some(field.to_string()),
        chars: record.chars.map(|chars| chars as u64),
        preview: Some(record.preview),
        source: Some(json!({
            "kind": "content_addressed",
            "field": field,
            "content_kind": kind,
            "hash": record.hash,
        })),
    }
}
