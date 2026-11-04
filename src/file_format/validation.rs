use super::*;

pub fn validation_from_scan(scanned: ScannedFile) -> PactFileValidation {
    validation_from_scan_ref(&scanned)
}

pub fn validation_from_scan_ref(scanned: &ScannedFile) -> PactFileValidation {
    let mut errors = scanned.errors.clone();
    let latest_valid_record_sequence = scanned
        .records
        .iter()
        .filter_map(|record| record.payload.get("_record_seq").and_then(Value::as_u64))
        .max()
        .or(Some(scanned.manifest.record_count as u64))
        .filter(|value| *value > 0);
    let latest_valid_transaction_id = scanned
        .records
        .iter()
        .filter(|record| record.kind == "trace")
        .filter_map(|record| {
            record
                .payload
                .get("transaction_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .next_back();
    let latest_valid_branch_heads = branch_heads_from_records(&scanned.records);
    if errors.is_empty() {
        errors.extend(validate_semantic_records(&scanned.records));
    }
    let recoverability = if errors.is_empty() {
        "fully_recoverable"
    } else if scanned.manifest.last_valid_offset > FILE_HEADER_LEN {
        "partially_recoverable"
    } else {
        "unrecoverable"
    }
    .to_string();
    PactFileValidation {
        ok: errors.is_empty(),
        status: if errors.is_empty() {
            "ok".to_string()
        } else {
            "invalid".to_string()
        },
        manifest: scanned.manifest.clone(),
        latest_valid_record_sequence,
        latest_valid_transaction_id,
        latest_valid_branch_heads,
        recoverability,
        errors,
    }
}

pub(super) fn validate_semantic_records(records: &[ScannedRecord]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut content_ids = BTreeMap::new();
    let mut branch_content_ids = BTreeMap::new();
    let mut chunk_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut branch_ids = BTreeMap::new();
    let mut node_ids = BTreeMap::new();
    let mut tool_pair_nodes = BTreeMap::new();
    let mut view_ids = BTreeMap::new();

    for record in records {
        validate_no_forbidden_hot_payload(record, &mut errors);
        match record.kind.as_str() {
            "content" => {
                if let Some(id) = record.payload.get("id").and_then(Value::as_str) {
                    validate_content_hash_scheme("content", id, &record.payload, &mut errors);
                    content_ids.insert(id.to_string(), record.payload.clone());
                } else {
                    errors.push("content record missing id".to_string());
                }
            }
            "content_chunk" => {
                if let Some(id) = record.payload.get("content_id").and_then(Value::as_str) {
                    validate_content_hash_scheme("content_chunk", id, &record.payload, &mut errors);
                    *chunk_counts.entry(id.to_string()).or_default() += 1;
                } else {
                    errors.push("content chunk missing content_id".to_string());
                }
            }
            "content_manifest" => {
                let Some(manifest_id) = record.payload.get("manifest_id").and_then(Value::as_str)
                else {
                    errors.push("content manifest missing manifest_id".to_string());
                    continue;
                };
                validate_content_hash_scheme(
                    "content_manifest",
                    manifest_id,
                    &record.payload,
                    &mut errors,
                );
                let chunks = record
                    .payload
                    .get("chunks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for chunk in chunks {
                    let Some(content_id) = chunk.get("content_id").and_then(Value::as_str) else {
                        errors.push(format!(
                            "content manifest {manifest_id} has chunk without content_id"
                        ));
                        continue;
                    };
                    if !content_ids.contains_key(content_id) {
                        errors.push(format!(
                            "content manifest {manifest_id} references missing content {content_id}"
                        ));
                    }
                }
            }
            "branch" => {
                if let Some(id) = record.payload.get("branch_id").and_then(Value::as_str) {
                    branch_ids.insert(id.to_string(), record.payload.clone());
                    if let Some(base_ref) = record.payload.get("base_ref")
                        && let Some(content_ref) =
                            base_ref.get("content_ref").and_then(Value::as_str)
                        && !content_ids.contains_key(content_ref)
                    {
                        errors.push(format!(
                            "branch {id} references missing base content {content_ref}"
                        ));
                    }
                    collect_branch_content_ref_ids(&record.payload, &mut branch_content_ids);
                    validate_branch_hot_node_shapes(id, &record.payload, &mut errors);
                } else {
                    errors.push("branch record missing branch_id".to_string());
                }
            }
            "node" => {
                if let Some(id) = record.payload.get("id").and_then(Value::as_str) {
                    node_ids.insert(id.to_string(), true);
                    validate_hot_node_payload_shape(id, &record.payload, &mut errors);
                    if let Some(content_ref) =
                        record.payload.get("content_ref").and_then(Value::as_str)
                        && !content_ids.contains_key(content_ref)
                        && !branch_content_ids.contains_key(content_ref)
                    {
                        errors.push(format!(
                            "node {id} references missing content {content_ref}"
                        ));
                    }
                } else {
                    errors.push("node record missing id".to_string());
                }
            }
            "trace" => {
                let branch_id = record.payload.get("branch_id").and_then(Value::as_str);
                if let Some(branch_id) = branch_id
                    && !branch_ids.contains_key(branch_id)
                {
                    errors.push(format!("trace references missing branch {branch_id}"));
                }
                if record
                    .payload
                    .get("transaction_id")
                    .and_then(Value::as_str)
                    .is_none()
                {
                    errors.push("trace record missing transaction_id".to_string());
                }
            }
            "view" => match serde_json::from_value::<PactViewRecord>(record.payload.clone()) {
                Ok(view) => {
                    if view.view_id.trim().is_empty() {
                        errors.push("view record missing view_id".to_string());
                    } else if view_ids.insert(view.view_id.clone(), true).is_some() {
                        errors.push(format!("duplicate view_id {}", view.view_id));
                    }
                    if view.root_id.trim().is_empty() {
                        errors.push(format!("view {} missing root_id", view.view_id));
                    }
                    if view.source_branch_id.trim().is_empty() {
                        errors.push(format!("view {} missing source_branch_id", view.view_id));
                    } else if !branch_ids.contains_key(&view.source_branch_id) {
                        errors.push(format!(
                            "view {} references missing source branch {}",
                            view.view_id, view.source_branch_id
                        ));
                    }
                    if !matches!(
                        view.mode.as_str(),
                        "readonly" | "live" | "mutable" | "mutable_live"
                    ) {
                        errors.push(format!(
                            "view {} has unsupported mode {}",
                            view.view_id, view.mode
                        ));
                    }
                    if matches!(view.mode.as_str(), "mutable" | "mutable_live")
                        && view
                            .view_branch_id
                            .as_deref()
                            .unwrap_or_default()
                            .is_empty()
                    {
                        errors.push(format!(
                            "mutable view {} requires view_branch_id",
                            view.view_id
                        ));
                    }
                    if let Some(view_branch_id) = &view.view_branch_id
                        && !branch_ids.contains_key(view_branch_id)
                    {
                        errors.push(format!(
                            "view {} references missing view branch {}",
                            view.view_id, view_branch_id
                        ));
                    }
                }
                Err(err) => errors.push(format!("invalid view record payload: {err}")),
            },
            "index" => {
                if record.payload.get("name").and_then(Value::as_str) == Some("tool_pair")
                    && let Some(entries) = record.payload.get("entries").and_then(Value::as_object)
                {
                    for (tool_call_id, value) in entries {
                        tool_pair_nodes.insert(tool_call_id.clone(), value.clone());
                    }
                }
            }
            _ => {}
        }
    }

    for (content_id, payload) in &content_ids {
        if payload.get("text").and_then(Value::as_str).is_some() {
            continue;
        }
        let expected = payload
            .get("chunk_count")
            .and_then(Value::as_u64)
            .unwrap_or_default() as usize;
        let actual = chunk_counts.get(content_id).copied().unwrap_or_default();
        if expected > 0 && actual != expected {
            errors.push(format!(
                "content {content_id} expected {expected} chunk(s), found {actual}"
            ));
        }
    }
    for (tool_call_id, value) in tool_pair_nodes {
        for field in ["call_ids", "result_ids"] {
            for component_id in value
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !node_ids.contains_key(component_id) {
                    errors.push(format!(
                        "tool pair {tool_call_id} references missing {field} node {component_id}"
                    ));
                }
            }
        }
    }
    errors
}

fn validate_no_forbidden_hot_payload(record: &ScannedRecord, errors: &mut Vec<String>) {
    collect_forbidden_hot_payloads(&record.payload, &record.kind, errors);
}

fn collect_forbidden_hot_payloads(value: &Value, path: &str, errors: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let next_path = format!("{path}.{key}");
                if forbidden_hot_payload_key(key) {
                    errors.push(format!(
                        "hot Pact record stores forbidden derived/provider payload at {next_path}"
                    ));
                }
                collect_forbidden_hot_payloads(child, &next_path, errors);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_forbidden_hot_payloads(item, &format!("{path}[{index}]"), errors);
            }
        }
        _ => {}
    }
}

fn forbidden_hot_payload_key(key: &str) -> bool {
    const FORBIDDEN_HOT_PAYLOAD_KEYS: &[&str] = &[
        concat!("rendered", "_", "xml"),
        "rendered_view",
        "ui_rows",
        "projection_rows",
        "provider_request",
        "provider_response",
        "provider_payload",
        "provider_request_payload",
        "provider_response_payload",
        "raw_provider_request",
        "raw_provider_response",
        "full_context",
        "full_head",
        "head_payload",
    ];
    FORBIDDEN_HOT_PAYLOAD_KEYS.contains(&key)
}

fn validate_content_hash_scheme(kind: &str, id: &str, payload: &Value, errors: &mut Vec<String>) {
    let Some(hash) = payload.get("hash").and_then(Value::as_str) else {
        errors.push(format!("{kind} {id} missing hash"));
        return;
    };
    if hash.len() != 32 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        errors.push(format!(
            "{kind} {id} uses unsupported content hash scheme; expected {CONTENT_HASH_SCHEME}"
        ));
    }
}

fn collect_branch_content_ref_ids(branch: &Value, out: &mut BTreeMap<String, bool>) {
    let Some(content_refs) = branch
        .get("current")
        .and_then(|current| current.get("content_refs"))
        .and_then(Value::as_object)
    else {
        return;
    };
    for content_id in content_refs.keys() {
        out.insert(content_id.clone(), true);
    }
}

fn validate_branch_hot_node_shapes(branch_id: &str, branch: &Value, errors: &mut Vec<String>) {
    let Some(nodes) = branch
        .get("current")
        .and_then(|current| current.get("components"))
        .and_then(Value::as_object)
    else {
        return;
    };
    for (component_id, node) in nodes {
        let id = node
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(component_id.as_str());
        let owner = format!("branch {branch_id} node {id}");
        validate_hot_node_payload_shape(&owner, node, errors);
    }
}

fn validate_hot_node_payload_shape(owner: &str, node: &Value, errors: &mut Vec<String>) {
    let has_content_ref = node
        .get("content_ref")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    if !has_content_ref {
        return;
    }
    let has_inline_content = node
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    if has_inline_content {
        errors.push(format!("{owner} stores both content and content_ref"));
    }
    let has_preview = node
        .get("content_preview")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    if has_preview {
        errors.push(format!("{owner} stores content_preview beside content_ref"));
    }
}

pub(super) fn ref_id_from_refish_value(value: &Value) -> Option<String> {
    if let Some(content_ref) = ref_id_from_content_ref_value(Some(value)) {
        return Some(content_ref);
    }
    if let Some(content_ref) = ref_id_from_content_ref_value(value.get("content_ref")) {
        return Some(content_ref);
    }
    None
}

pub(super) fn ref_id_from_content_ref_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    let object = value.as_object()?;
    object
        .get("id")
        .or_else(|| object.get("ref_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}
