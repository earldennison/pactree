use super::*;

use crate::error::{PactError, PactResult};
use crate::store::PactView;

pub(super) fn node_records_by_id(records: &[ScannedRecord]) -> BTreeMap<String, Value> {
    records
        .iter()
        .filter(|record| record.kind == "node")
        .filter_map(|record| {
            record
                .payload
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), record.payload.clone()))
        })
        .collect()
}

pub(super) fn node_records_from_records(
    records: &[ScannedRecord],
) -> BTreeMap<String, ComponentRecord> {
    records
        .iter()
        .filter(|record| record.kind == "node")
        .filter_map(|record| serde_json::from_value::<ComponentRecord>(record.payload.clone()).ok())
        .map(|node| (node.id.clone(), node))
        .collect()
}

pub(super) fn cleaned_component_records(branch: &PactBranch) -> BTreeMap<String, ComponentRecord> {
    let mut records = branch.current.component_records();
    for node in records.values_mut() {
        strip_self_markup_from_node_record(node);
        strip_redundant_node_aux(node);
        externalize_media_content_node(node);
        strip_derived_operation_body_content(node);
        apply_capture_policy_to_node(node);
    }
    records
}

pub(super) fn file_view_payload(view: &PactView, include_components: bool) -> PactResult<Value> {
    let mut payload = serde_json::to_value(view)
        .map_err(|error| PactError::codec(format!("encode file view: {error}")))?;
    let records = view.component_records();
    let node_refs = records
        .keys()
        .map(|id| (id.clone(), Value::String(id.clone())))
        .collect::<serde_json::Map<_, _>>();
    if let Some(object) = payload.as_object_mut() {
        object.insert("first_seen".to_string(), json!(view.tree.first_seen));
        object.insert("tool_pairs".to_string(), json!(view.tree.tool_pairs));
        object.insert("node_refs".to_string(), Value::Object(node_refs));
        let components = if include_components {
            records
                .into_iter()
                .map(|(id, record)| (id, serde_json::to_value(record).unwrap_or(Value::Null)))
                .collect()
        } else {
            serde_json::Map::new()
        };
        object.insert("components".to_string(), Value::Object(components));
        if let Some(tree) = object.get_mut("tree").and_then(Value::as_object_mut) {
            tree.insert(
                "root".to_string(),
                json!({
                    "id": view.tree.root_id,
                    "componentType": view.tree.root.component_type(),
                    "children": [],
                }),
            );
        }
    }
    Ok(payload)
}

fn apply_capture_policy_to_node(node: &mut ComponentRecord) {
    if node
        .content_capture
        .is_some_and(|policy| !policy.persists_content())
    {
        node.content = None;
        node.content_ref = None;
        node.content_preview = None;
    }
}

pub(super) fn apply_capture_policy_to_node_value(node: &mut Value) {
    let Some(object) = node.as_object_mut() else {
        return;
    };
    let should_remove = object
        .get("content_capture")
        .and_then(Value::as_str)
        .and_then(crate::components::ContentCapturePolicy::parse)
        .is_some_and(|policy| !policy.persists_content());
    if should_remove {
        object.remove("content");
        object.remove("content_ref");
        object.remove("content_preview");
    }
}

pub(super) fn omitted_duplicate_branch_base_bytes(root_store: &ContextTreeLedger) -> usize {
    root_store
        .branches
        .values()
        .filter(|branch| {
            branch.base_ref.content_ref.is_some()
                && branch.head_revision == 0
                && branch.current.component_count() > 0
        })
        .filter_map(|branch| branch.base_ref.content_ref.as_deref())
        .filter_map(|content_id| root_store.content.records.get(content_id))
        .map(|record| record.bytes)
        .sum()
}

pub(super) fn collect_content_ref_ids_from_value(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(content_ref) = ref_id_from_content_ref_value(object.get("content_ref")) {
                out.insert(content_ref.to_string());
            }
            for key in [
                "parameters_ref",
                "result_ref",
                "error_ref",
                "media_content_ref",
                "media_data_ref",
            ] {
                if let Some(content_ref) = object.get(key).and_then(ref_id_from_refish_value) {
                    out.insert(content_ref);
                }
            }
            for child in object.values() {
                collect_content_ref_ids_from_value(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_content_ref_ids_from_value(child, out);
            }
        }
        _ => {}
    }
}

fn strip_self_markup_from_node_record(node: &mut ComponentRecord) {
    let rendered_content = node
        .content
        .as_deref()
        .is_some_and(|text| self_markup_for_node(text, &node.node_type, node.kind.as_deref()));
    if rendered_content {
        node.content = None;
        node.content_ref = None;
        node.content_preview = None;
    } else if node.content_ref.is_some() {
        node.content_preview = None;
    }
}

fn strip_redundant_node_aux(node: &mut ComponentRecord) {
    let candidates = [
        ("id", Value::String(node.id.clone())),
        ("componentType", Value::String(node.node_type.clone())),
        ("offset", Value::Number(node.offset.into())),
        ("created_at_ns", Value::Number(node.created_at_ns.into())),
        ("creation_index", Value::Number(node.creation_index.into())),
    ];
    for (key, value) in candidates {
        if node.aux.get(key) == Some(&value) {
            node.aux.remove(key);
        }
    }
    if let Some(depth) = node.pact_depth {
        for key in ["pact_depth", "depth"] {
            if node.aux.get(key).and_then(Value::as_i64) == Some(depth) {
                node.aux.remove(key);
            }
        }
    }
}

fn strip_derived_operation_body_content(node: &mut ComponentRecord) {
    let duplicated = match node.node_type.as_str() {
        "tool_call" => node
            .content
            .as_deref()
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .is_some_and(|content| node.parameters.as_ref() == Some(&content)),
        "tool_result" => node
            .content
            .as_deref()
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .is_some_and(|content| node.result.as_ref() == Some(&content)),
        _ => false,
    };
    if duplicated {
        node.content = None;
        node.content_ref = None;
        node.content_preview = None;
    }
}

fn externalize_media_content_node(node: &mut ComponentRecord) {
    let Some(media) = node.aux.get("media_content").and_then(Value::as_object) else {
        return;
    };
    let Some(data) = media.get("data").and_then(Value::as_str) else {
        return;
    };
    if media
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("base64")
        != "base64"
    {
        return;
    }
    let Ok(bytes) = base64_decode(data) else {
        return;
    };
    let family = node
        .kind
        .as_deref()
        .unwrap_or(&node.node_type)
        .strip_suffix("_content")
        .unwrap_or("media");
    let kind = format!("media.{family}");
    let hash = stable_hash(&kind, &bytes);
    node.aux.remove("media_content");
    node.aux.insert(
        "media_content_ref".to_string(),
        json!({
            "content_ref": format!("cnt_media_{}", &hash[..8]),
            "kind": kind,
        }),
    );
}

fn self_markup_for_node(text: &str, node_type: &str, kind: Option<&str>) -> bool {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('<')
        || trimmed.starts_with("</")
        || trimmed.starts_with("<!")
        || trimmed.starts_with("<?")
    {
        return false;
    }
    let tag = trimmed[1..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.'))
        .collect::<String>();
    !tag.is_empty() && (tag == node_type || kind.is_some_and(|value| tag == value))
}

pub(super) fn branch_record_payload(branch: &PactBranch) -> PactResult<Value> {
    let mut payload = serde_json::to_value(branch)
        .map_err(|error| PactError::codec(format!("encode branch facade: {error}")))?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "current".to_string(),
            file_view_payload(&branch.current, false)?,
        );
    }
    Ok(payload)
}
