use crate::error::{PactError, PactResult};
use crate::file_format::base64_encode;
use crate::store::ContentStore;
use serde_json::{Map, Value};

#[derive(Clone, Copy)]
enum PayloadShape {
    Text,
    Json,
}

pub fn hydrate_payload_refs(value: &mut Value, content_store: &ContentStore) -> PactResult<()> {
    match value {
        Value::Object(object) => {
            hydrate_object_ref_slot(
                object,
                "content_ref",
                "content",
                PayloadShape::Text,
                content_store,
            )?;
            hydrate_object_ref_slot(
                object,
                "parameters_ref",
                "parameters",
                PayloadShape::Json,
                content_store,
            )?;
            hydrate_object_ref_slot(
                object,
                "result_ref",
                "result",
                PayloadShape::Json,
                content_store,
            )?;
            hydrate_object_ref_slot(
                object,
                "error_ref",
                "error_message",
                PayloadShape::Text,
                content_store,
            )?;
            hydrate_media_content_ref(object, content_store)?;
            for (key, child) in object.iter_mut() {
                if is_opaque_payload_field(key) {
                    continue;
                }
                hydrate_payload_refs(child, content_store)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                hydrate_payload_refs(child, content_store)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_opaque_payload_field(key: &str) -> bool {
    matches!(key, "parameters" | "result" | "error_message")
}

fn hydrate_media_content_ref(
    object: &mut Map<String, Value>,
    content_store: &ContentStore,
) -> PactResult<()> {
    let Some(aux) = object.get_mut("aux").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if aux.get("media_content").is_some() {
        return Ok(());
    }
    let Some(ref_id) = aux.get("media_content_ref").and_then(content_ref_id) else {
        return Ok(());
    };
    let bytes = hydrate_ref_id_bytes(ref_id, content_store)?;
    aux.insert(
        "media_content".to_string(),
        serde_json::json!({
            "data": base64_encode(&bytes),
            "encoding": "base64",
        }),
    );
    Ok(())
}

fn hydrate_ref_id_bytes(ref_id: &str, content_store: &ContentStore) -> PactResult<Vec<u8>> {
    let record = content_store.records.get(ref_id).ok_or_else(|| {
        PactError::not_found(format!(
            "PACT content ref is not locally projectable: {ref_id}"
        ))
    })?;
    if let Some(bytes) = record.storage.inline_bytes.as_ref() {
        return Ok(bytes.clone());
    }
    if let Some(text) = record.storage.inline.as_deref() {
        return Ok(text.as_bytes().to_vec());
    }
    Err(PactError::not_found(format!(
        "PACT content ref is not locally projectable: {ref_id}"
    )))
}

fn hydrate_object_ref_slot(
    object: &mut Map<String, Value>,
    ref_key: &str,
    target_key: &str,
    shape: PayloadShape,
    content_store: &ContentStore,
) -> PactResult<()> {
    let Some(ref_id) = object.get(ref_key).and_then(content_ref_id) else {
        return Ok(());
    };
    let hydrated = hydrate_ref_id(ref_id, shape, content_store)?;
    object.insert(target_key.to_string(), hydrated);
    object.remove(ref_key);
    Ok(())
}

fn hydrate_ref_id(
    ref_id: &str,
    shape: PayloadShape,
    content_store: &ContentStore,
) -> PactResult<Value> {
    let content = content_store
        .records
        .get(ref_id)
        .and_then(|record| record.storage.inline.as_deref())
        .ok_or_else(|| {
            PactError::not_found(format!(
                "PACT content ref is not locally projectable: {ref_id}"
            ))
        })?;
    match shape {
        PayloadShape::Text => Ok(Value::String(content.to_string())),
        PayloadShape::Json => serde_json::from_str(content)
            .or_else(|_| Ok::<Value, serde_json::Error>(Value::String(content.to_string())))
            .map_err(|err| {
                PactError::codec(format!("failed to hydrate content ref {ref_id}: {err}"))
            }),
    }
}

fn content_ref_id(value: &Value) -> Option<&str> {
    match value {
        Value::String(id) => Some(id.as_str()),
        Value::Object(object) => object
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| object.get("content_ref").and_then(content_ref_id))
            .or_else(|| object.get("external_ref").and_then(content_ref_id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
