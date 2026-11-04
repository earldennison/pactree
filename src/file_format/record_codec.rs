use super::layout::RecordKind;
use crate::error::{PactError, PactResult};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn compact_canonical_record_payload(kind: RecordKind, payload: Value) -> Value {
    let payload = if kind == RecordKind::Trace {
        compact_trace_relative_refs(payload)
    } else {
        compact_pact_refs(payload)
    };
    match kind {
        RecordKind::Symbol => compact_object_fields(
            payload,
            &[
                "id",
                "value",
                "values",
                "occurrences",
                "estimated_saved_bytes",
                "file_local_ids",
                "file_local_id_kinds",
                "token_span_tokens",
                "token_span_dense_tokens",
                "token_span_hashes",
                "token_span_sources",
            ],
        ),
        RecordKind::Meta => compact_object_fields(
            payload,
            &[
                "schema",
                "version",
                "file_kind",
                "root_id",
                "current_branch_id",
                "record_count",
                "symbol_count",
                "content_record_count",
                "content_chunk_count",
                "content_manifest_count",
                "root_count",
                "branch_count",
                "node_count",
                "trace_count",
                "component_count",
                "component_transaction_count",
                "component_payload_count",
                "view_count",
                "commit_count",
                "frozen_cycle_count",
                "head_count",
                "cycle_count",
                "index_count",
                "footer_count",
                "last_valid_offset",
                "capabilities",
                "codec_substrate",
                "pages",
                "latest_footer",
            ],
        ),
        RecordKind::Content => compact_object_fields(
            payload,
            &[
                "id",
                "kind",
                "hash",
                "bytes",
                "chars",
                "preview_inline",
                "chunk_count",
                "codec",
                "encoding",
                "text",
            ],
        ),
        RecordKind::ContentChunk => compact_object_fields(
            payload,
            &[
                "content_id",
                "ordinal",
                "hash",
                "bytes",
                "codec",
                "encoding",
                "text",
                "ref_content_id",
                "ref_ordinal",
            ],
        ),
        RecordKind::ContentManifest => compact_object_fields(
            payload,
            &[
                "manifest_id",
                "kind",
                "hash",
                "bytes",
                "chars",
                "chunk_count",
                "chunks",
                "aux",
            ],
        ),
        RecordKind::Root => compact_object_fields(
            payload,
            &["root_id", "current_branch_id", "branches", "parent", "base"],
        ),
        RecordKind::Branch => compact_object_fields(
            payload,
            &[
                "branch_id",
                "root_id",
                "parent_branch_id",
                "fork_ref",
                "base_ref",
                "head_revision",
                "lifecycle_tick",
                "current",
                "aux",
                "transaction_traces",
            ],
        ),
        RecordKind::Node => compact_object_fields(
            payload,
            &[
                "id",
                "componentType",
                "type",
                "role",
                "parent_id",
                "offset",
                "pact_depth",
                "created_at_ns",
                "creation_index",
                "child_ids",
                "children",
                "attrs",
                "aux",
                "content",
                "content_ref",
                "content_preview",
                "ttl",
                "cad",
                "lifecycle",
                "key",
                "tags",
                "tool_call_id",
                "tool_name",
                "parameters",
                "parameters_ref",
                "success",
                "result",
                "result_ref",
                "error_message",
                "error_ref",
                "content_capture",
                "source_ref",
                "content_status",
            ],
        ),
        RecordKind::Component => compact_object_fields(
            payload,
            &[
                "component_id",
                "root_id",
                "branch_id",
                "kind",
                "schema_version",
                "internal_refs",
                "external_refs",
                "revision",
                "lifecycle",
                "aux",
            ],
        ),
        RecordKind::ComponentPayload => compact_object_fields(payload, &["payload_id", "payload"]),
        RecordKind::Trace => compact_object_fields(
            payload,
            &[
                "schema",
                "root_id",
                "branch_id",
                "transaction_id",
                "authored_ref",
                "pre_request_ref",
                "post_request_alteration_ref",
                "request_ref",
                "response_ref",
                "post_response_alteration_ref",
                "post_response_ref",
                "alteration_ids",
                "commit",
                "aux",
                TRACE_LOCAL_TRANSACTION_IDS_FIELD,
                "replay_trace",
            ],
        ),
        RecordKind::View => compact_object_fields(
            payload,
            &[
                "view_id",
                "root_id",
                "source_branch_id",
                "mode",
                "source_revision",
                "view_branch_id",
                "mount_boundary",
                "follow_cursor",
                "local_transaction_start",
                "local_transaction_end",
                "changed_component_ids",
                "touched_node_refs",
                "external_refs",
                "conflict_policy_hints",
                "aux",
            ],
        ),
        RecordKind::Commit => compact_commit_record_payload(payload),
        RecordKind::FrozenCycle => compact_object_fields(
            payload,
            &[
                "schema",
                "cycle",
                "revision",
                "root_ref",
                "index_refs",
                "hash",
            ],
        ),
        RecordKind::Index => compact_object_fields(payload, &["name", "entries"]),
        RecordKind::Head => compact_object_fields(
            payload,
            &[
                "schema",
                "branch_id",
                "head_revision",
                "head_cycle",
                "commit_offset",
                "root_node_ref",
                "changed_node_refs",
                "content_ref_ids",
                "record_sequence_ids",
                "lifecycle_tick",
            ],
        ),
    }
}

pub(super) fn expand_canonical_record_payload(kind: &str, payload: Value) -> PactResult<Value> {
    let expanded = match kind {
        "symbol" => expand_array_fields(
            payload,
            &[
                "id",
                "value",
                "values",
                "occurrences",
                "estimated_saved_bytes",
                "file_local_ids",
                "file_local_id_kinds",
                "token_span_tokens",
                "token_span_dense_tokens",
                "token_span_hashes",
                "token_span_sources",
            ],
        ),
        "meta" => expand_array_fields(
            payload,
            &[
                "schema",
                "version",
                "file_kind",
                "root_id",
                "current_branch_id",
                "record_count",
                "symbol_count",
                "content_record_count",
                "content_chunk_count",
                "content_manifest_count",
                "root_count",
                "branch_count",
                "node_count",
                "trace_count",
                "component_count",
                "component_transaction_count",
                "component_payload_count",
                "view_count",
                "commit_count",
                "frozen_cycle_count",
                "head_count",
                "cycle_count",
                "index_count",
                "footer_count",
                "last_valid_offset",
                "capabilities",
                "codec_substrate",
                "pages",
                "latest_footer",
            ],
        ),
        "content" => expand_array_fields(
            payload,
            &[
                "id",
                "kind",
                "hash",
                "bytes",
                "chars",
                "preview_inline",
                "chunk_count",
                "codec",
                "encoding",
                "text",
            ],
        ),
        "content_chunk" => expand_array_fields(
            payload,
            &[
                "content_id",
                "ordinal",
                "hash",
                "bytes",
                "codec",
                "encoding",
                "text",
                "ref_content_id",
                "ref_ordinal",
            ],
        ),
        "content_manifest" => expand_array_fields(
            payload,
            &[
                "manifest_id",
                "kind",
                "hash",
                "bytes",
                "chars",
                "chunk_count",
                "chunks",
                "aux",
            ],
        ),
        "root" => expand_array_fields(
            payload,
            &["root_id", "current_branch_id", "branches", "parent", "base"],
        ),
        "branch" => expand_array_fields(
            payload,
            &[
                "branch_id",
                "root_id",
                "parent_branch_id",
                "fork_ref",
                "base_ref",
                "head_revision",
                "lifecycle_tick",
                "current",
                "aux",
                "transaction_traces",
            ],
        ),
        "node" => expand_array_fields(
            payload,
            &[
                "id",
                "componentType",
                "type",
                "role",
                "parent_id",
                "offset",
                "pact_depth",
                "created_at_ns",
                "creation_index",
                "child_ids",
                "children",
                "attrs",
                "aux",
                "content",
                "content_ref",
                "content_preview",
                "ttl",
                "cad",
                "lifecycle",
                "key",
                "tags",
                "tool_call_id",
                "tool_name",
                "parameters",
                "parameters_ref",
                "success",
                "result",
                "result_ref",
                "error_message",
                "error_ref",
                "content_capture",
                "source_ref",
                "content_status",
            ],
        ),
        "component" => expand_array_fields(
            payload,
            &[
                "component_id",
                "root_id",
                "branch_id",
                "kind",
                "schema_version",
                "internal_refs",
                "external_refs",
                "revision",
                "lifecycle",
                "aux",
            ],
        ),
        "component_transaction" => expand_array_fields(
            payload,
            &[
                "transaction_id",
                "component_id",
                "branch_id",
                "kind",
                "operation",
                "actor_ref",
                "boundary",
                "args_ref",
                "result_ref",
                "before_state_ref",
                "after_state_ref",
                "emitted_node_refs",
                "emitted_projection_refs",
                "external_refs",
                "effects",
                "caused_by",
                "correlation_id",
                "aux",
            ],
        ),
        "component_payload" => expand_array_fields(payload, &["payload_id", "payload"]),
        "trace" => expand_array_fields(
            payload,
            &[
                "schema",
                "root_id",
                "branch_id",
                "transaction_id",
                "authored_ref",
                "pre_request_ref",
                "post_request_alteration_ref",
                "request_ref",
                "response_ref",
                "post_response_alteration_ref",
                "post_response_ref",
                "alteration_ids",
                "commit",
                "aux",
                TRACE_LOCAL_TRANSACTION_IDS_FIELD,
                "replay_trace",
            ],
        ),
        "view" => expand_array_fields(
            payload,
            &[
                "view_id",
                "root_id",
                "source_branch_id",
                "mode",
                "source_revision",
                "view_branch_id",
                "mount_boundary",
                "follow_cursor",
                "local_transaction_start",
                "local_transaction_end",
                "changed_component_ids",
                "touched_node_refs",
                "external_refs",
                "conflict_policy_hints",
                "aux",
            ],
        ),
        "commit" => expand_commit_record_payload(payload),
        "frozen_cycle" => expand_array_fields(
            payload,
            &[
                "schema",
                "cycle",
                "revision",
                "root_ref",
                "index_refs",
                "hash",
            ],
        ),
        "index" => expand_array_fields(payload, &["name", "entries"]),
        "head" => expand_array_fields(
            payload,
            &[
                "schema",
                "branch_id",
                "head_revision",
                "head_cycle",
                "commit_offset",
                "root_node_ref",
                "changed_node_refs",
                "content_ref_ids",
                "record_sequence_ids",
                "lifecycle_tick",
            ],
        ),
        _ => payload,
    };
    let expanded = if kind == "trace" {
        expand_trace_relative_refs(expanded)?
    } else {
        expanded
    };
    Ok(expand_pact_refs(expanded))
}

const PACT_REF_FIELD_ORDER: &[&str] = &[
    "root_id",
    "branch_id",
    "revision",
    "transaction_id",
    "component_id",
    "content_ref",
];

const TRACE_RELATIVE_REF_FIELD_ORDER: &[&str] =
    &["revision", "transaction_id", "component_id", "content_ref"];
const TRACE_OP_TAGS: &[(u64, &str)] = &[
    (1, "insert_component"),
    (2, "delete_component"),
    (3, "move_component"),
    (4, "set_field"),
    (5, "unset_field"),
    (6, "set_attr"),
    (7, "unset_attr"),
    (8, "set_content"),
    (9, "replace_content_range"),
    (10, "set_lifecycle"),
];

pub(super) const TRACE_BOUNDARY_REF_KEYS: &[&str] = &[
    "authored_ref",
    "pre_request_ref",
    "post_request_alteration_ref",
    "request_ref",
    "response_ref",
    "post_response_alteration_ref",
    "post_response_ref",
];
const TRACE_LOCAL_TRANSACTION_IDS_FIELD: &str = "_trace_local_transaction_ids";

#[derive(Default)]
struct LocalIdTable {
    ids: Vec<String>,
    index: BTreeMap<String, u64>,
}

impl LocalIdTable {
    fn intern(&mut self, id: &str) -> u64 {
        if let Some(slot) = self.index.get(id) {
            return *slot;
        }
        let slot = self.ids.len() as u64;
        self.ids.push(id.to_string());
        self.index.insert(id.to_string(), slot);
        slot
    }

    fn index(&self) -> &BTreeMap<String, u64> {
        &self.index
    }

    fn store(
        self,
        object: &mut serde_json::Map<String, Value>,
        field: &'static str,
        min_len: usize,
    ) {
        if self.ids.len() >= min_len {
            object.insert(
                field.to_string(),
                Value::Array(self.ids.into_iter().map(Value::String).collect()),
            );
        }
    }

    fn load(object: &mut serde_json::Map<String, Value>, field: &str) -> Vec<String> {
        object
            .remove(field)
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|value| value.as_str().map(str::to_string).unwrap_or_default())
            .collect()
    }

    fn resolve(table: &[String], slot: u64) -> Option<&str> {
        table
            .get(slot as usize)
            .map(String::as_str)
            .filter(|id| !id.is_empty())
    }
}

fn compact_trace_relative_refs(value: Value) -> Value {
    let Value::Object(mut object) = value else {
        return compact_pact_refs(value);
    };
    let Some(root_id) = object
        .get("root_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return compact_pact_refs(Value::Object(object));
    };
    let Some(branch_id) = object
        .get("branch_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return compact_pact_refs(Value::Object(object));
    };
    let local_transaction_ids = trace_local_transaction_ids(&object, &root_id, &branch_id);

    for key in TRACE_BOUNDARY_REF_KEYS {
        if let Some(ref_value) = object.remove(*key) {
            object.insert(
                (*key).to_string(),
                compact_trace_ref_value(
                    ref_value,
                    &root_id,
                    &branch_id,
                    Some(local_transaction_ids.index()),
                ),
            );
        }
    }
    if let Some(commit) = object.get_mut("commit").and_then(Value::as_object_mut)
        && let Some(ref_value) = commit.remove("pact_ref")
    {
        commit.insert(
            "pact_ref".to_string(),
            compact_trace_ref_value(
                ref_value,
                &root_id,
                &branch_id,
                Some(local_transaction_ids.index()),
            ),
        );
    }
    compact_trace_replay_ops(&mut object);
    local_transaction_ids.store(&mut object, TRACE_LOCAL_TRANSACTION_IDS_FIELD, 2);
    compact_pact_refs(Value::Object(object))
}

fn trace_local_transaction_ids(
    object: &serde_json::Map<String, Value>,
    root_id: &str,
    branch_id: &str,
) -> LocalIdTable {
    let mut table = LocalIdTable::default();
    if let Some(transaction_id) = object.get("transaction_id").and_then(Value::as_str) {
        table.intern(transaction_id);
    }
    for key in TRACE_BOUNDARY_REF_KEYS {
        collect_trace_ref_transaction_id(object.get(*key), root_id, branch_id, &mut table);
    }
    if let Some(ref_value) = object
        .get("commit")
        .and_then(Value::as_object)
        .and_then(|commit| commit.get("pact_ref"))
    {
        collect_trace_ref_transaction_id(Some(ref_value), root_id, branch_id, &mut table);
    }
    table
}

fn collect_trace_ref_transaction_id(
    value: Option<&Value>,
    root_id: &str,
    branch_id: &str,
    table: &mut LocalIdTable,
) {
    let Some(object) = value.and_then(Value::as_object) else {
        return;
    };
    if !object_looks_like_pact_ref(object)
        || object.get("root_id").and_then(Value::as_str) != Some(root_id)
        || object.get("branch_id").and_then(Value::as_str) != Some(branch_id)
    {
        return;
    }
    let Some(transaction_id) = object.get("transaction_id").and_then(Value::as_str) else {
        return;
    };
    table.intern(transaction_id);
}

pub(super) fn compact_trace_ref_value(
    value: Value,
    root_id: &str,
    branch_id: &str,
    local_transaction_index: Option<&BTreeMap<String, u64>>,
) -> Value {
    let Value::Object(object) = value else {
        return compact_pact_refs(value);
    };
    if !object_looks_like_pact_ref(&object)
        || object.get("root_id").and_then(Value::as_str) != Some(root_id)
        || object.get("branch_id").and_then(Value::as_str) != Some(branch_id)
    {
        return compact_pact_refs(Value::Object(object));
    }
    let mut object = object;
    if let Some(local_transaction_index) = local_transaction_index
        && let Some(slot) = object
            .get("transaction_id")
            .and_then(Value::as_str)
            .and_then(|transaction_id| local_transaction_index.get(transaction_id).copied())
    {
        object.insert("transaction_id".to_string(), Value::Number(slot.into()));
    }
    compact_object_fields(Value::Object(object), TRACE_RELATIVE_REF_FIELD_ORDER)
}

fn expand_trace_relative_refs(value: Value) -> PactResult<Value> {
    let Value::Object(mut object) = value else {
        return Ok(value);
    };
    let Some(root_id) = object
        .get("root_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(Value::Object(object));
    };
    let Some(branch_id) = object
        .get("branch_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(Value::Object(object));
    };
    let mut local_transaction_ids =
        LocalIdTable::load(&mut object, TRACE_LOCAL_TRANSACTION_IDS_FIELD);
    if local_transaction_ids.is_empty()
        && let Some(transaction_id) = object.get("transaction_id").and_then(Value::as_str)
    {
        local_transaction_ids.push(transaction_id.to_string());
    }

    for key in TRACE_BOUNDARY_REF_KEYS {
        if let Some(ref_value) = object.remove(*key) {
            object.insert(
                (*key).to_string(),
                expand_trace_ref_value(ref_value, &root_id, &branch_id, &local_transaction_ids),
            );
        }
    }
    if let Some(commit) = object.get_mut("commit").and_then(Value::as_object_mut)
        && let Some(ref_value) = commit.remove("pact_ref")
    {
        commit.insert(
            "pact_ref".to_string(),
            expand_trace_ref_value(ref_value, &root_id, &branch_id, &local_transaction_ids),
        );
    }
    expand_trace_replay_ops(&mut object)?;
    Ok(Value::Object(object))
}

fn compact_trace_replay_ops(object: &mut serde_json::Map<String, Value>) {
    let Some(ops) = object
        .get_mut("replay_trace")
        .and_then(Value::as_object_mut)
        .and_then(|replay| replay.get_mut("ops"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for op in ops {
        let Some(object) = op.as_object_mut() else {
            continue;
        };
        let Some(code) = object
            .get("op")
            .and_then(Value::as_str)
            .and_then(trace_op_tag_code)
        else {
            continue;
        };
        compact_trace_insert_component_id(object);
        object.insert("op".to_string(), Value::Number(code.into()));
    }
}

fn expand_trace_replay_ops(object: &mut serde_json::Map<String, Value>) -> PactResult<()> {
    let transaction_id = object
        .get("transaction_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "<unknown>".to_string());
    let Some(ops) = object
        .get_mut("replay_trace")
        .and_then(Value::as_object_mut)
        .and_then(|replay| replay.get_mut("ops"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for op in ops {
        let Some(code) = op
            .as_object_mut()
            .and_then(|object| object.get("op"))
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let Some(tag) = trace_op_tag_name(code) else {
            return Err(PactError::codec(format!(
                "trace record {transaction_id} contains unknown replay_trace op code {code}"
            )));
        };
        if let Some(object) = op.as_object_mut() {
            object.insert("op".to_string(), Value::String(tag.to_string()));
            expand_trace_insert_component_id(object);
        }
    }
    Ok(())
}

fn compact_trace_insert_component_id(object: &mut serde_json::Map<String, Value>) {
    if object.get("op").and_then(Value::as_str) != Some("insert_component") {
        return;
    }
    let Some(component_id) = object.get("component_id").and_then(Value::as_str) else {
        return;
    };
    let component_id = component_id.to_string();
    let Some(component) = object.get_mut("component").and_then(Value::as_object_mut) else {
        return;
    };
    if component.get("id").and_then(Value::as_str) == Some(component_id.as_str()) {
        component.remove("id");
    }
}

fn expand_trace_insert_component_id(object: &mut serde_json::Map<String, Value>) {
    if object.get("op").and_then(Value::as_str) != Some("insert_component") {
        return;
    }
    let Some(component_id) = object.get("component_id").and_then(Value::as_str) else {
        return;
    };
    let component_id = component_id.to_string();
    let Some(component) = object.get_mut("component").and_then(Value::as_object_mut) else {
        return;
    };
    component
        .entry("id")
        .or_insert_with(|| Value::String(component_id));
}

fn trace_op_tag_code(tag: &str) -> Option<u64> {
    TRACE_OP_TAGS
        .iter()
        .find_map(|(code, name)| (*name == tag).then_some(*code))
}

fn trace_op_tag_name(code: u64) -> Option<&'static str> {
    TRACE_OP_TAGS
        .iter()
        .find_map(|(candidate, name)| (*candidate == code).then_some(*name))
}

fn expand_trace_ref_value(
    value: Value,
    root_id: &str,
    branch_id: &str,
    local_transaction_ids: &[String],
) -> Value {
    if value
        .as_array()
        .is_none_or(|items| items.first().is_some_and(|first| first.is_string()))
    {
        return value;
    }
    let mut expanded = expand_array_fields(value, TRACE_RELATIVE_REF_FIELD_ORDER);
    if let Some(object) = expanded.as_object_mut() {
        object.insert("root_id".to_string(), Value::String(root_id.to_string()));
        object.insert(
            "branch_id".to_string(),
            Value::String(branch_id.to_string()),
        );
        if let Some(slot) = object.get("transaction_id").and_then(Value::as_u64)
            && let Some(transaction_id) = LocalIdTable::resolve(local_transaction_ids, slot)
        {
            object.insert(
                "transaction_id".to_string(),
                Value::String(transaction_id.to_string()),
            );
        }
    }
    expanded
}

fn compact_pact_refs(value: Value) -> Value {
    compact_pact_refs_in_context(None, value)
}

fn compact_pact_refs_in_context(key_hint: Option<&str>, value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            if key_hint.is_some_and(is_pact_ref_key) && object_looks_like_pact_ref(&object) {
                return compact_object_fields(Value::Object(object), PACT_REF_FIELD_ORDER);
            }
            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                if let Some(child) = object.remove(&key) {
                    object.insert(key.clone(), compact_pact_refs_in_context(Some(&key), child));
                }
            }
            Value::Object(object)
        }
        Value::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| compact_pact_refs_in_context(key_hint, item))
                .collect();
            Value::Array(items)
        }
        other => other,
    }
}

fn expand_pact_refs(value: Value) -> Value {
    expand_pact_refs_in_context(None, value)
}

fn expand_pact_refs_in_context(key_hint: Option<&str>, value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                if let Some(child) = object.remove(&key) {
                    object.insert(key.clone(), expand_pact_refs_in_context(Some(&key), child));
                }
            }
            Value::Object(object)
        }
        Value::Array(items) if key_hint.is_some_and(is_pact_ref_list_key) => {
            let items = items
                .into_iter()
                .map(|item| expand_pact_refs_in_context(Some("pact_ref"), item))
                .collect();
            Value::Array(items)
        }
        Value::Array(items) if key_hint.is_some_and(is_pact_ref_key) => {
            expand_array_fields(Value::Array(items), PACT_REF_FIELD_ORDER)
        }
        Value::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| expand_pact_refs_in_context(key_hint, item))
                .collect();
            Value::Array(items)
        }
        other => other,
    }
}

pub(super) fn object_looks_like_pact_ref(object: &serde_json::Map<String, Value>) -> bool {
    object.get("root_id").and_then(Value::as_str).is_some()
        && object.get("branch_id").and_then(Value::as_str).is_some()
        && object
            .keys()
            .all(|key| PACT_REF_FIELD_ORDER.contains(&key.as_str()))
}

fn is_pact_ref_key(key: &str) -> bool {
    matches!(
        key,
        "pact_ref"
            | "node_ref"
            | "base_ref"
            | "fork_ref"
            | "authored_ref"
            | "pre_request_ref"
            | "post_request_alteration_ref"
            | "request_ref"
            | "response_ref"
            | "post_response_alteration_ref"
            | "post_response_ref"
            | "before_ref"
            | "after_ref"
            | "parent_ref"
            | "branch_refs"
            | "touched_node_refs"
            | "produced_refs"
    )
}

fn is_pact_ref_list_key(key: &str) -> bool {
    matches!(key, "branch_refs" | "touched_node_refs" | "produced_refs")
}

fn compact_object_fields(payload: Value, fields: &[&str]) -> Value {
    let Some(object) = payload.as_object() else {
        return payload;
    };
    let mut compact = fields
        .iter()
        .map(|field| object.get(*field).cloned().unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    while compact.last().is_some_and(is_compact_empty_value) {
        compact.pop();
    }
    Value::Array(compact)
}

fn compact_commit_record_payload(mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut()
        && let Some(ops) = object.get_mut("ops").and_then(Value::as_array_mut)
    {
        for op in ops {
            *op = compact_commit_op(op.clone());
        }
    }
    compact_object_fields(
        payload,
        &[
            "schema",
            "branch_id",
            "cycle",
            "revision",
            "ops",
            "lifecycle_tick",
        ],
    )
}

fn compact_commit_op(op: Value) -> Value {
    let Some(object) = op.as_object() else {
        return op;
    };
    match object.get("op").and_then(Value::as_str).unwrap_or_default() {
        "insert_component" => json!([
            1,
            object.get("parent_id").cloned().unwrap_or(Value::Null),
            object.get("node_ref").cloned().unwrap_or(Value::Null),
            object.get("position").cloned().unwrap_or(Value::Null),
        ]),
        "replace_component" => json!([
            2,
            object.get("component_id").cloned().unwrap_or(Value::Null),
            object.get("node_ref").cloned().unwrap_or(Value::Null),
        ]),
        "delete_component" => json!([
            3,
            object.get("component_id").cloned().unwrap_or(Value::Null),
        ]),
        "append_components" => json!([
            4,
            object.get("parent_id").cloned().unwrap_or(Value::Null),
            object.get("node_refs").cloned().unwrap_or(Value::Null),
        ]),
        "set_component_aux" => json!([
            5,
            object.get("component_id").cloned().unwrap_or(Value::Null),
            object.get("key_symbol").cloned().unwrap_or(Value::Null),
            object.get("value_ref").cloned().unwrap_or(Value::Null),
        ]),
        "attach_content_ref" => json!([
            6,
            object.get("component_id").cloned().unwrap_or(Value::Null),
            object.get("blob_id").cloned().unwrap_or(Value::Null),
        ]),
        "replace_content_range" => json!([
            7,
            object.get("component_id").cloned().unwrap_or(Value::Null),
            object.get("range").cloned().unwrap_or(Value::Null),
            object.get("blob_id").cloned().unwrap_or(Value::Null),
        ]),
        _ => op,
    }
}

fn expand_array_fields(payload: Value, fields: &[&str]) -> Value {
    let Some(items) = payload.as_array() else {
        return payload;
    };
    let mut object = serde_json::Map::new();
    for (index, field) in fields.iter().enumerate() {
        let Some(value) = items.get(index) else {
            continue;
        };
        if is_compact_empty_value(value) {
            continue;
        }
        object.insert((*field).to_string(), value.clone());
    }
    Value::Object(object)
}

fn expand_commit_record_payload(payload: Value) -> Value {
    let mut expanded = expand_array_fields(
        payload,
        &[
            "schema",
            "branch_id",
            "cycle",
            "revision",
            "ops",
            "lifecycle_tick",
        ],
    );
    if let Some(object) = expanded.as_object_mut()
        && let Some(ops) = object.get_mut("ops").and_then(Value::as_array_mut)
    {
        for op in ops {
            *op = expand_commit_op(op.clone());
        }
    }
    expanded
}

fn expand_commit_op(op: Value) -> Value {
    let Some(items) = op.as_array() else {
        return op;
    };
    let code = items.first().and_then(Value::as_u64).unwrap_or_default();
    let at = |index: usize| items.get(index).cloned().unwrap_or(Value::Null);
    match code {
        1 => json!({
            "op": "insert_component",
            "parent_id": at(1),
            "node_ref": at(2),
            "position": at(3),
        }),
        2 => json!({
            "op": "replace_component",
            "component_id": at(1),
            "node_ref": at(2),
        }),
        3 => json!({
            "op": "delete_component",
            "component_id": at(1),
        }),
        4 => json!({
            "op": "append_components",
            "parent_id": at(1),
            "node_refs": at(2),
        }),
        5 => json!({
            "op": "set_component_aux",
            "component_id": at(1),
            "key_symbol": at(2),
            "value_ref": at(3),
        }),
        6 => json!({
            "op": "attach_content_ref",
            "component_id": at(1),
            "blob_id": at(2),
        }),
        7 => json!({
            "op": "replace_content_range",
            "component_id": at(1),
            "range": at(2),
            "blob_id": at(3),
        }),
        _ => op,
    }
}

fn is_compact_empty_value(value: &Value) -> bool {
    value.is_null()
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
}
