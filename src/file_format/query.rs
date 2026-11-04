use super::*;
use crate::error::{PactError, PactResult};

const HOT_QUERY_INDEX_NAME: &str = "pact.hot_query_index.v1";

pub(super) fn try_fast_query_json(
    path: &Path,
    query: &str,
    key: Option<&str>,
) -> PactResult<Option<PactFileQueryResult>> {
    let buckets = match query {
        "node" => key
            .map(|key| vec![("node_by_id", Some(hot_query_shard_key(key)))])
            .unwrap_or_default(),
        "node-birth" | "birth" => key
            .map(|key| vec![("node_birth", Some(hot_query_shard_key(key)))])
            .unwrap_or_default(),
        "tool-pair" => vec![("tool_pair", None)],
        "message-order" => vec![("message_order", None)],
        "type" => vec![("type", None)],
        "parent" => vec![("parent", None)],
        "depth" => vec![("depth", None)],
        "attr" => vec![("attr", None)],
        "head-id" => vec![("head_id", None)],
        "content" => key
            .map(|key| vec![("content_by_id", Some(hot_query_shard_key(key)))])
            .unwrap_or_default(),
        "text-window" => key
            .and_then(|key| key.split(':').next())
            .map(|content_id| vec![("content_by_id", Some(hot_query_shard_key(content_id)))])
            .unwrap_or_default(),
        _ => return Ok(None),
    };
    if buckets.is_empty() {
        return Ok(None);
    }
    let Some(index) = load_hot_query_index(path, &buckets)? else {
        return Ok(None);
    };
    let matches = match query {
        "node" => fast_query_node(path, &index, key)?,
        "node-birth" | "birth" => fast_query_index_object(&index, "node_birth", key),
        "tool-pair" => fast_query_index_object(&index, "tool_pair", key),
        "message-order" => fast_query_index_object(&index, "message_order", key),
        "type" => fast_query_index_object(&index, "type", key),
        "parent" => fast_query_index_object(&index, "parent", key),
        "depth" => fast_query_index_object(&index, "depth", key),
        "attr" => fast_query_index_object(&index, "attr", key),
        "head-id" => fast_query_index_object(&index, "head_id", key),
        "content" => fast_query_content(path, &index, key)?,
        "text-window" => fast_query_text_window(path, &index, key)?,
        _ => return Ok(None),
    };
    Ok(Some(PactFileQueryResult {
        schema: format!("{PACT_FILE_SCHEMA}.query"),
        query: query.to_string(),
        key: key.map(str::to_string),
        matches,
    }))
}

fn load_hot_query_index(
    path: &Path,
    buckets: &[(&str, Option<String>)],
) -> PactResult<Option<Value>> {
    let file_len = fs::metadata(path)
        .map_err(|err| PactError::storage(format!("failed to stat .pact file: {err}")))?
        .len();
    let mut file = fs::File::open(path)
        .map_err(|err| PactError::storage(format!("failed to open .pact file: {err}")))?;
    let mut bytes_read = 0_u64;
    let Some(footer) = read_footer_from_end(&mut file, file_len, &mut bytes_read)? else {
        return Ok(None);
    };
    if footer.hot_index_offset == 0 {
        return Ok(None);
    }
    if footer.last_commit_offset != 0 {
        return Ok(None);
    }
    let symbols = read_symbol_table_prefix_from_file(&mut file, &mut bytes_read)?;
    let Some(directory) = read_index_record_named_from_file(
        &mut file,
        footer.hot_index_offset,
        &mut bytes_read,
        &symbols,
        HOT_QUERY_INDEX_NAME,
    )?
    else {
        return Ok(None);
    };
    let Some(index_addresses) = directory
        .payload
        .get("entries")
        .and_then(|entries| entries.get("indexes"))
        .and_then(Value::as_object)
    else {
        return Ok(None);
    };
    let mut index = serde_json::Map::new();
    for (bucket, shard) in buckets {
        let Some(address) = index_addresses.get(*bucket).and_then(|value| {
            if let Some(shard) = shard {
                value.get(shard)
            } else {
                Some(value)
            }
        }) else {
            continue;
        };
        let Some(record) = read_record_from_file(
            &mut file,
            &serde_json::from_value::<PactRecordAddress>(address.clone()).map_err(|err| {
                PactError::codec(format!(
                    "invalid hot query subindex address for {bucket}: {err}"
                ))
            })?,
            &mut bytes_read,
            &symbols,
        )?
        else {
            continue;
        };
        if let Some(entries) = record.payload.get("entries") {
            index.insert((*bucket).to_string(), entries.clone());
        }
    }
    if index.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(index)))
    }
}

fn fast_query_index_object(index: &Value, bucket: &str, key: Option<&str>) -> Vec<Value> {
    let Some(object) = index.get(bucket).and_then(Value::as_object) else {
        return Vec::new();
    };
    if let Some(key) = key {
        object.get(key).cloned().into_iter().collect()
    } else {
        object.values().cloned().collect()
    }
}

fn fast_query_node(path: &Path, index: &Value, key: Option<&str>) -> PactResult<Vec<Value>> {
    let Some(key) = key else {
        return Ok(Vec::new());
    };
    let Some(address_value) = index
        .get("node_by_id")
        .and_then(Value::as_object)
        .and_then(|nodes| nodes.get(key))
    else {
        return Ok(Vec::new());
    };
    let Some(record) = read_addressed_record(path, address_value)? else {
        return Ok(Vec::new());
    };
    let mut payload = record.payload;
    hydrate_fast_query_node_payload_refs(path, index, &mut payload)?;
    Ok(vec![payload])
}

fn fast_query_content(path: &Path, index: &Value, key: Option<&str>) -> PactResult<Vec<Value>> {
    let Some(key) = key else {
        return Ok(Vec::new());
    };
    let Some(entry) = index
        .get("content_by_id")
        .and_then(Value::as_object)
        .and_then(|content| content.get(key))
    else {
        return Ok(Vec::new());
    };
    let Some(record) = entry
        .get("address")
        .map(|address| read_addressed_record(path, address))
        .transpose()?
        .flatten()
    else {
        return Ok(Vec::new());
    };
    Ok(vec![record.payload])
}

fn fast_query_text_window(path: &Path, index: &Value, key: Option<&str>) -> PactResult<Vec<Value>> {
    let Some(key) = key else {
        return Err(PactError::invalid(
            ".pact text-window query requires content_id:start:end".to_string(),
        ));
    };
    let parts = key.split(':').collect::<Vec<_>>();
    if !(parts.len() == 3 || parts.len() == 4) {
        return Err(PactError::invalid(
            ".pact text-window key must be content_id:start:end[:decoded_hash]".to_string(),
        ));
    }
    let content_id = parts[0];
    let start = parts[1]
        .parse::<usize>()
        .map_err(|_| PactError::invalid(format!("invalid text-window start byte: {}", parts[1])))?;
    let end = parts[2]
        .parse::<usize>()
        .map_err(|_| PactError::invalid(format!("invalid text-window end byte: {}", parts[2])))?;
    if start > end {
        return Err(PactError::invalid(
            "text-window start byte is after end byte".to_string(),
        ));
    }
    let Some(content_entry) = index
        .get("content_by_id")
        .and_then(Value::as_object)
        .and_then(|content| content.get(content_id))
    else {
        return Ok(Vec::new());
    };

    let window_bytes = if let Some(chunks) = content_entry.get("chunks").and_then(Value::as_array) {
        let mut bytes = Vec::new();
        for chunk in chunks {
            let address: PactContentChunkAddress =
                serde_json::from_value(chunk.clone()).map_err(|err| {
                    PactError::codec(format!(
                        "invalid content chunk address in hot query index: {err}"
                    ))
                })?;
            if address.decoded_end_byte <= start || address.decoded_start_byte >= end {
                continue;
            }
            let chunk_bytes = read_content_chunk_bytes_by_address(path, index, &address)?;
            let chunk_start = start.max(address.decoded_start_byte) - address.decoded_start_byte;
            let chunk_end = end.min(address.decoded_end_byte) - address.decoded_start_byte;
            if chunk_end > chunk_bytes.len() {
                return Err(PactError::corrupt(format!(
                    "content chunk {}:{} shorter than indexed byte range",
                    address.content_id, address.ordinal
                )));
            }
            bytes.extend_from_slice(&chunk_bytes[chunk_start..chunk_end]);
        }
        bytes
    } else {
        let Some(record) = content_entry
            .get("address")
            .map(|address| read_addressed_record(path, address))
            .transpose()?
            .flatten()
        else {
            return Ok(Vec::new());
        };
        let payload: ContentPayload = serde_json::from_value(record.payload).map_err(|err| {
            PactError::codec(format!("invalid content record for {content_id}: {err}"))
        })?;
        let decoded = decode_content_payload_bytes(&payload)?;
        if end > decoded.len() {
            return Err(PactError::invalid(format!(
                "text-window range {start}..{end} is not valid for content {content_id}"
            )));
        }
        decoded[start..end].to_vec()
    };
    let window = String::from_utf8(window_bytes).map_err(|_| {
        PactError::invalid(format!(
            "text-window range {start}..{end} is not valid UTF-8 for content {content_id}"
        ))
    })?;
    let decoded_hash = stable_hash("text.address", window.as_bytes());
    if let Some(expected_hash) = parts.get(3)
        && *expected_hash != decoded_hash
    {
        return Err(PactError::invalid(format!(
            "text-window decoded hash mismatch for {content_id}:{start}:{end}"
        )));
    }
    Ok(vec![json!({
        "content_id": content_id,
        "byte_start": start,
        "byte_end": end,
        "text": window,
        "decoded_hash": decoded_hash,
    })])
}

fn read_content_chunk_bytes_by_address(
    path: &Path,
    index: &Value,
    address: &PactContentChunkAddress,
) -> PactResult<Vec<u8>> {
    let record = read_addressed_record(
        path,
        &json!({
            "page_offset": address.page_offset,
            "page_seq": address.page_seq,
            "record_seq": address.record_seq,
            "record_kind": RecordKind::ContentChunk as u16,
        }),
    )?
    .ok_or_else(|| {
        PactError::not_found(format!(
            "missing content chunk {}:{} at addressed record {}",
            address.content_id, address.ordinal, address.record_seq
        ))
    })?;
    if record.payload.get("codec").and_then(Value::as_str) == Some("ref") {
        let ref_content_id = record
            .payload
            .get("ref_content_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "content chunk {}:{} missing ref_content_id",
                    address.content_id, address.ordinal
                ))
            })?;
        let ref_ordinal = record
            .payload
            .get("ref_ordinal")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "content chunk {}:{} missing ref_ordinal",
                    address.content_id, address.ordinal
                ))
            })? as usize;
        let Some(ref_address) = find_content_chunk_address(index, ref_content_id, ref_ordinal)
        else {
            return Err(PactError::not_found(format!(
                "missing referenced content chunk {ref_content_id}:{ref_ordinal}"
            )));
        };
        return read_content_chunk_bytes_by_address(path, index, &ref_address);
    }
    decode_content_chunk_bytes(&record.payload)
}

fn find_content_chunk_address(
    index: &Value,
    content_id: &str,
    ordinal: usize,
) -> Option<PactContentChunkAddress> {
    index
        .get("content_by_id")?
        .get(content_id)?
        .get("chunks")?
        .as_array()?
        .iter()
        .filter_map(|value| serde_json::from_value::<PactContentChunkAddress>(value.clone()).ok())
        .find(|address| address.ordinal == ordinal)
}

fn read_addressed_record(path: &Path, address: &Value) -> PactResult<Option<ScannedRecord>> {
    let address: PactRecordAddress = serde_json::from_value(address.clone()).map_err(|err| {
        PactError::codec(format!("invalid record address in hot query index: {err}"))
    })?;
    let mut file = fs::File::open(path)
        .map_err(|err| PactError::storage(format!("failed to open .pact file: {err}")))?;
    let mut bytes_read = 0_u64;
    let symbols = read_symbol_table_prefix_from_file(&mut file, &mut bytes_read)?;
    read_record_from_file(&mut file, &address, &mut bytes_read, &symbols)
}

fn hydrate_fast_query_node_payload_refs(
    path: &Path,
    index: &Value,
    node: &mut Value,
) -> PactResult<()> {
    hydrate_fast_query_payload_ref_slot(path, index, node, "parameters_ref", "parameters", true)?;
    hydrate_fast_query_payload_ref_slot(path, index, node, "result_ref", "result", true)?;
    hydrate_fast_query_payload_ref_slot(path, index, node, "error_ref", "error_message", false)?;
    Ok(())
}

fn hydrate_fast_query_payload_ref_slot(
    path: &Path,
    index: &Value,
    node: &mut Value,
    ref_key: &str,
    target_key: &str,
    json_payload: bool,
) -> PactResult<()> {
    let Some(object) = node.as_object_mut() else {
        return Ok(());
    };
    if object.contains_key(target_key) {
        return Ok(());
    }
    let Some(content_id) = object.get(ref_key).and_then(ref_id_from_refish_value) else {
        return Ok(());
    };
    let owned_content_index;
    let content_index = if index.get("content_by_id").is_some() {
        index
    } else {
        owned_content_index = load_hot_query_index(
            path,
            &[("content_by_id", Some(hot_query_shard_key(&content_id)))],
        )?
        .unwrap_or(Value::Null);
        &owned_content_index
    };
    let Some(content_entry) = content_index
        .get("content_by_id")
        .and_then(Value::as_object)
        .and_then(|content| content.get(&content_id))
    else {
        return Ok(());
    };
    let Some(record) = content_entry
        .get("address")
        .map(|address| read_addressed_record(path, address))
        .transpose()?
        .flatten()
    else {
        return Ok(());
    };
    let payload: ContentPayload = serde_json::from_value(record.payload)
        .map_err(|err| PactError::codec(format!("invalid content record {content_id}: {err}")))?;
    let bytes = decode_content_payload_bytes(&payload)?;
    let text = String::from_utf8(bytes)
        .map_err(|err| PactError::codec(format!("content {content_id} is not UTF-8: {err}")))?;
    let value = if json_payload {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    } else {
        Value::String(text)
    };
    object.insert(target_key.to_string(), value);
    Ok(())
}

fn hot_query_shard_key(value: &str) -> String {
    value.chars().take(4).collect::<String>()
}

pub(super) fn query_records(
    records: &[ScannedRecord],
    query: &str,
    key: Option<&str>,
) -> PactResult<PactFileQueryResult> {
    let matches = match query {
        "cycles" => cycle_ranges_from_records(records)
            .into_iter()
            .map(|range| serde_json::to_value(range).unwrap_or_else(|_| json!({})))
            .collect(),
        "cycle" => {
            let Some(key) = key else {
                return Err(PactError::invalid(
                    ".pact cycle query requires a cycle number".to_string(),
                ));
            };
            let cycle = key
                .parse::<u64>()
                .map_err(|_| PactError::invalid(format!("invalid cycle number: {key}")))?;
            filter_nodes_by_cycle(records, cycle)
        }
        "node" => query_current_head_node(records, key),
        "node-birth" | "birth" => query_node_birth(records, key),
        "branch" => filter_record(records, "branch", key, "branch_id"),
        "branch-head" => prefer_scanned_index(
            query_branch_heads(records, key),
            query_index(records, "branch_head", key),
        ),
        "transaction" | "trace" => filter_record(records, "trace", key, "transaction_id"),
        "component" => filter_latest_component_record(records, key),
        "component-kind" => prefer_scanned_index(
            query_component_field(records, key, "kind"),
            query_index(records, "component_by_kind", key),
        ),
        "component-transaction" => {
            filter_record(records, "component_transaction", key, "transaction_id")
        }
        "component-transactions" => prefer_scanned_index(
            query_component_transaction_field(records, key, "component_id"),
            query_index(records, "component_transaction_by_component", key),
        ),
        "component-boundary" => prefer_scanned_index(
            query_component_transaction_field(records, key, "boundary"),
            query_index(records, "component_transaction_by_boundary", key),
        ),
        "component-payload" => filter_record(records, "component_payload", key, "payload_id"),
        "view" => filter_record(records, "view", key, "view_id"),
        "views" => filter_record(records, "view", key, "source_branch_id"),
        "view-branch" => prefer_scanned_index(
            query_view_field(records, key, "view_branch_id"),
            query_index(records, "view_by_view_branch", key),
        ),
        "content" => filter_record(records, "content", key, "id"),
        "content-hash" => filter_record(records, "content", key, "hash"),
        "content-manifest" => filter_record(records, "content_manifest", key, "manifest_id"),
        "content-manifest-hash" => filter_record(records, "content_manifest", key, "hash"),
        "symbol" => filter_record(records, "symbol", key, "text"),
        "tool-pair" => prefer_scanned_index(
            query_current_head_tool_pairs(records, key),
            prefer_scanned_index(
                query_branch_tool_pairs(records, key),
                query_index(records, "tool_pair", key),
            ),
        ),
        "message-order" => prefer_scanned_index(
            query_branch_message_order(records, key),
            query_index(records, "message_order", key),
        ),
        "depth" => prefer_current_head_selector_index(
            query_head_selector_bucket(records, "depth", key),
            prefer_scanned_index(
                query_projected_head_selector_bucket(records, "depth", key),
                query_nodes_by_depth(records, key),
            ),
        ),
        "type" => prefer_current_head_selector_index(
            query_head_selector_bucket(records, "type", key),
            prefer_scanned_index(
                query_projected_head_selector_bucket(records, "type", key),
                query_nodes_by_type(records, key),
            ),
        ),
        "parent" => prefer_current_head_selector_index(
            query_head_selector_bucket(records, "parent", key),
            prefer_scanned_index(
                query_projected_head_selector_bucket(records, "parent", key),
                query_nodes_by_parent(records, key),
            ),
        ),
        "head-id" => prefer_current_head_selector_index(
            query_head_selector_bucket(records, "id", key),
            prefer_scanned_index(
                query_projected_head_selector_bucket(records, "id", key),
                key.and_then(|component_id| {
                    filter_record(records, "node", Some(component_id), "id")
                        .into_iter()
                        .next()
                })
                .into_iter()
                .collect(),
            ),
        ),
        "attr" => prefer_scanned_index(
            query_nodes_by_attr(records, key),
            query_index(records, "attr", key),
        ),
        "aux" | "aux-namespace" => prefer_scanned_index(
            query_nodes_by_aux_namespace(records, key),
            query_index(records, "by_aux_key", key),
        ),
        "index" => filter_record(records, "index", key, "name"),
        "text-address" | "text-addresses" => query_index(records, "text_address", key),
        "text-window" => query_text_window(records, key)?,
        "json-leaf" => query_json_leaf(records, key)?,
        other => {
            return Err(PactError::invalid(format!(
                "unsupported .pact query: {other}"
            )));
        }
    };
    Ok(PactFileQueryResult {
        schema: format!("{PACT_FILE_SCHEMA}.query"),
        query: query.to_string(),
        key: key.map(str::to_string),
        matches,
    })
}

fn query_json_leaf(records: &[ScannedRecord], key: Option<&str>) -> PactResult<Vec<Value>> {
    let Some(key) = key else {
        return Err(PactError::invalid(
            ".pact json-leaf query requires content_id:/json/pointer".to_string(),
        ));
    };
    let (content_id, pointer) = key.split_once(':').ok_or_else(|| {
        PactError::invalid(".pact json-leaf key must be content_id:/json/pointer".to_string())
    })?;
    let content = content_texts_from_records(records);
    let Some(text) = content.get(content_id) else {
        return Ok(Vec::new());
    };
    let value: Value = serde_json::from_str(text)
        .map_err(|err| PactError::codec(format!("content {content_id} is not JSON: {err}")))?;
    let Some(leaf) = value.pointer(pointer) else {
        return Ok(Vec::new());
    };
    let Some(text) = leaf.as_str() else {
        return Err(PactError::invalid(format!(
            "JSON pointer {pointer} in {content_id} is not a string leaf"
        )));
    };
    Ok(vec![json!({
        "content_id": content_id,
        "json_pointer": pointer,
        "text": text,
        "decoded_hash": stable_hash("text.address", text.as_bytes()),
    })])
}

fn query_text_window(records: &[ScannedRecord], key: Option<&str>) -> PactResult<Vec<Value>> {
    let Some(key) = key else {
        return Err(PactError::invalid(
            ".pact text-window query requires content_id:start:end".to_string(),
        ));
    };
    let parts = key.split(':').collect::<Vec<_>>();
    if !(parts.len() == 3 || parts.len() == 4) {
        return Err(PactError::invalid(
            ".pact text-window key must be content_id:start:end[:decoded_hash]".to_string(),
        ));
    }
    let content_id = parts[0];
    let start = parts[1]
        .parse::<usize>()
        .map_err(|_| PactError::invalid(format!("invalid text-window start byte: {}", parts[1])))?;
    let end = parts[2]
        .parse::<usize>()
        .map_err(|_| PactError::invalid(format!("invalid text-window end byte: {}", parts[2])))?;
    if start > end {
        return Err(PactError::invalid(
            "text-window start byte is after end byte".to_string(),
        ));
    }
    let content = content_texts_from_records(records);
    let Some(text) = content.get(content_id) else {
        return Ok(Vec::new());
    };
    if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return Err(PactError::invalid(format!(
            "text-window range {start}..{end} is not valid for content {content_id}"
        )));
    }
    let window = &text[start..end];
    let decoded_hash = stable_hash("text.address", window.as_bytes());
    if let Some(expected_hash) = parts.get(3)
        && *expected_hash != decoded_hash
    {
        return Err(PactError::invalid(format!(
            "text-window decoded hash mismatch for {content_id}:{start}:{end}"
        )));
    }
    Ok(vec![json!({
        "content_id": content_id,
        "byte_start": start,
        "byte_end": end,
        "text": window,
        "decoded_hash": decoded_hash,
    })])
}

pub(super) fn filter_record(
    records: &[ScannedRecord],
    kind: &str,
    key: Option<&str>,
    field: &str,
) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.kind == kind)
        .filter(|record| {
            key.map(|key| record.payload.get(field).and_then(Value::as_str) == Some(key))
                .unwrap_or(true)
        })
        .map(|record| record.payload.clone())
        .collect()
}

pub(super) fn query_component_payload_records(
    records: &[ScannedRecord],
    key: Option<&str>,
) -> PactResult<PactFileQueryResult> {
    let matches = records
        .iter()
        .filter(|record| record.kind == "component_payload")
        .filter(|record| {
            key.map(|key| record.payload.get("payload_id").and_then(Value::as_str) == Some(key))
                .unwrap_or(true)
        })
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    Ok(PactFileQueryResult {
        schema: format!("{PACT_FILE_SCHEMA}.query"),
        query: "component-payload".to_string(),
        key: key.map(str::to_string),
        matches,
    })
}

fn prefer_scanned_index(scanned: Vec<Value>, indexed: Vec<Value>) -> Vec<Value> {
    if scanned.is_empty() { indexed } else { scanned }
}

fn prefer_current_head_selector_index(indexed: Vec<Value>, fallback: Vec<Value>) -> Vec<Value> {
    if indexed.is_empty() {
        fallback
    } else {
        indexed
    }
}

fn query_current_head_node(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let Some(key) = key else {
        let projected = query_projected_head_nodes(records, None);
        return if projected.is_empty() {
            filter_record(records, "node", None, "id")
        } else {
            projected
        };
    };
    let head_entry = query_head_selector_bucket(records, "id", Some(key))
        .into_iter()
        .next();
    let component_ref = head_entry
        .as_ref()
        .and_then(|entry| entry.get("component_ref"))
        .or_else(|| head_entry.as_ref().and_then(|entry| entry.get("node_ref")))
        .and_then(Value::as_str)
        .unwrap_or(key);
    let scanned = filter_record(records, "node", Some(component_ref), "id");
    if scanned.is_empty() {
        query_projected_head_nodes(records, Some(key))
    } else {
        hydrate_query_node_payload_refs(scanned, records)
    }
}

fn hydrate_query_node_payload_refs(mut nodes: Vec<Value>, records: &[ScannedRecord]) -> Vec<Value> {
    let Ok(root_store) = ledger_from_records(records) else {
        return nodes;
    };
    for node in &mut nodes {
        hydrate_query_payload_ref_slot(
            node,
            &root_store.content,
            "parameters_ref",
            "parameters",
            true,
        );
        hydrate_query_payload_ref_slot(node, &root_store.content, "result_ref", "result", true);
        hydrate_query_payload_ref_slot(
            node,
            &root_store.content,
            "error_ref",
            "error_message",
            false,
        );
    }
    nodes
}

fn hydrate_query_payload_ref_slot(
    node: &mut Value,
    content: &ContentStore,
    ref_key: &str,
    target_key: &str,
    json_payload: bool,
) {
    let Some(object) = node.as_object_mut() else {
        return;
    };
    if object.contains_key(target_key) {
        return;
    }
    let Some(content_id) = object.get(ref_key).and_then(ref_id_from_refish_value) else {
        return;
    };
    let Some(text) = content
        .records
        .get(&content_id)
        .and_then(|record| record.storage.inline.as_deref())
    else {
        return;
    };
    let value = if json_payload {
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
    } else {
        Value::String(text.to_string())
    };
    object.insert(target_key.to_string(), value);
}

fn query_node_birth(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let Ok(root_store) = ledger_from_records(records) else {
        return Vec::new();
    };
    let Some(branch) = root_store.branches.get(&root_store.current_branch_id) else {
        return Vec::new();
    };
    branch
        .current
        .tree
        .first_seen
        .iter()
        .filter(|(component_id, _)| key.map(|target| *component_id == target).unwrap_or(true))
        .map(|(component_id, revision)| {
            let current = !query_current_head_node(records, Some(component_id)).is_empty();
            json!({
                "component_id": component_id,
                "first_seen": revision,
                "current_surface": current,
                "branch_id": &branch.branch_id,
            })
        })
        .collect()
}

fn query_projected_head_nodes(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let Ok(root_store) = ledger_from_records(records) else {
        return Vec::new();
    };
    let Some(branch) = root_store.branches.get(&root_store.current_branch_id) else {
        return Vec::new();
    };
    crate::store::project_branch_component_window(
        &root_store,
        branch,
        Some(branch.head_revision),
        key,
        None,
    )
    .map(|components| {
        components
            .into_iter()
            .map(|component| component.project_component_value())
            .collect()
    })
    .unwrap_or_default()
}

fn query_head_selector_bucket(
    records: &[ScannedRecord],
    bucket: &str,
    key: Option<&str>,
) -> Vec<Value> {
    let Some(entries) = latest_head_selector_entries(records) else {
        return Vec::new();
    };
    let Some(bucket_value) = entries.get(bucket) else {
        return Vec::new();
    };
    match key {
        Some(key) => bucket_value
            .get(key)
            .cloned()
            .into_iter()
            .collect::<Vec<_>>(),
        None => vec![bucket_value.clone()],
    }
}

fn query_projected_head_selector_bucket(
    records: &[ScannedRecord],
    bucket: &str,
    key: Option<&str>,
) -> Vec<Value> {
    query_native_head_selector_bucket(records, bucket, key).unwrap_or_default()
}

fn query_native_head_selector_bucket(
    records: &[ScannedRecord],
    bucket: &str,
    key: Option<&str>,
) -> Option<Vec<Value>> {
    let root_store = ledger_from_records(records).ok()?;
    let branch = root_store.branches.get(&root_store.current_branch_id)?;
    if branch.current.component_count() == 0 {
        return None;
    }
    match (bucket, key) {
        ("id", Some(component_id)) => branch
            .current
            .component(component_id)
            .and_then(|component| {
                native_head_id_entry(&branch.current.tree, component_id, component)
            })
            .map(|entry| vec![entry]),
        ("parent", Some(parent_id)) => branch
            .current
            .tree
            .index
            .child_ids
            .get(parent_id)
            .map(|ids| vec![string_array_value(ids)]),
        ("type", Some(component_type)) => branch
            .current
            .tree
            .index
            .by_type
            .get(component_type)
            .map(|ids| vec![string_array_value(ids)]),
        ("depth", Some(depth_key)) => native_head_depth_ids(&branch.current.tree, depth_key)
            .map(|ids| vec![string_array_value(&ids)]),
        ("id", None) => Some(vec![native_head_id_entries(&branch.current.tree)]),
        ("parent", None) => Some(vec![string_array_object_value(
            branch.current.tree.index.child_ids.iter(),
        )]),
        ("type", None) => Some(vec![string_array_object_value(
            branch.current.tree.index.by_type.iter(),
        )]),
        ("depth", None) => Some(vec![native_head_depth_entries(&branch.current.tree)]),
        _ => None,
    }
}

fn native_head_id_entries(tree: &crate::store::ContextTreeView) -> Value {
    let entries = tree
        .index
        .by_id
        .keys()
        .filter_map(|component_id| {
            tree.component(component_id)
                .and_then(|component| native_head_id_entry(tree, component_id, component))
                .map(|entry| (component_id.clone(), entry))
        })
        .collect();
    Value::Object(entries)
}

fn native_head_id_entry(
    tree: &crate::store::ContextTreeView,
    component_id: &str,
    component: &crate::components::Component,
) -> Option<Value> {
    let path = tree.index.by_id.get(component_id)?;
    Some(json!({
        "component_ref": component_id,
        "type": component.component_type(),
        "depth": path.path.len(),
        "parent_ref": path.parent_id,
    }))
}

fn native_head_depth_ids(
    tree: &crate::store::ContextTreeView,
    depth_key: &str,
) -> Option<Vec<String>> {
    let depth = parse_depth_key(depth_key)?;
    Some(
        tree.index
            .by_id
            .iter()
            .filter(|(_, path)| path.path.len() == depth)
            .map(|(component_id, _)| component_id.clone())
            .collect(),
    )
}

fn native_head_depth_entries(tree: &crate::store::ContextTreeView) -> Value {
    let mut by_depth: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (component_id, path) in &tree.index.by_id {
        by_depth
            .entry(format!("d{}", path.path.len()))
            .or_default()
            .push(component_id.clone());
    }
    string_array_object_value(by_depth.iter())
}

fn parse_depth_key(depth_key: &str) -> Option<usize> {
    depth_key.strip_prefix('d')?.parse::<usize>().ok()
}

fn string_array_value(ids: &[String]) -> Value {
    Value::Array(ids.iter().cloned().map(Value::String).collect())
}

fn string_array_object_value<'a>(
    entries: impl Iterator<Item = (&'a String, &'a Vec<String>)>,
) -> Value {
    Value::Object(
        entries
            .map(|(key, ids)| (key.clone(), string_array_value(ids)))
            .collect(),
    )
}

fn latest_head_selector_entries(
    records: &[ScannedRecord],
) -> Option<&serde_json::Map<String, Value>> {
    records
        .iter()
        .rev()
        .filter(|record| record.kind == "index")
        .find_map(|record| {
            if record.payload.get("name").and_then(Value::as_str) != Some("head_selector") {
                return None;
            }
            record.payload.get("entries").and_then(Value::as_object)
        })
}

fn filter_latest_component_record(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    if key.is_none() {
        return filter_record(records, "component", None, "component_id");
    }
    let Some(key) = key else {
        return Vec::new();
    };
    records
        .iter()
        .rev()
        .find(|record| {
            record.kind == "component"
                && record.payload.get("component_id").and_then(Value::as_str) == Some(key)
        })
        .map(|record| vec![record.payload.clone()])
        .unwrap_or_default()
}

fn query_component_field(records: &[ScannedRecord], key: Option<&str>, field: &str) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "component") {
        let Some(group_key) = record.payload.get(field).and_then(Value::as_str) else {
            continue;
        };
        let Some(component_id) = record.payload.get("component_id").and_then(Value::as_str) else {
            continue;
        };
        grouped
            .entry(group_key.to_string())
            .or_default()
            .push(Value::String(component_id.to_string()));
    }
    if let Some(key) = key {
        grouped.remove(key).map(Value::Array).into_iter().collect()
    } else {
        grouped.into_values().map(Value::Array).collect()
    }
}

fn query_component_transaction_field(
    records: &[ScannedRecord],
    key: Option<&str>,
    field: &str,
) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| record.kind == "component_transaction")
    {
        let Some(group_key) = record.payload.get(field).and_then(Value::as_str) else {
            continue;
        };
        let Some(transaction_id) = record.payload.get("transaction_id").and_then(Value::as_str)
        else {
            continue;
        };
        grouped
            .entry(group_key.to_string())
            .or_default()
            .push(Value::String(transaction_id.to_string()));
    }
    if let Some(key) = key {
        grouped.remove(key).map(Value::Array).into_iter().collect()
    } else {
        grouped.into_values().map(Value::Array).collect()
    }
}

fn query_branch_heads(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.kind == "branch")
        .filter(|record| {
            key.map(|key| record.payload.get("branch_id").and_then(Value::as_str) == Some(key))
                .unwrap_or(true)
        })
        .map(|record| {
            json!({
                "root_id": record.payload.get("root_id").cloned().unwrap_or(Value::Null),
                "branch_id": record.payload.get("branch_id").cloned().unwrap_or(Value::Null),
                "head_revision": record.payload.get("head_revision").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn query_view_field(records: &[ScannedRecord], key: Option<&str>, field: &str) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "view") {
        let Some(group_key) = record.payload.get(field).and_then(Value::as_str) else {
            continue;
        };
        let Some(view_id) = record.payload.get("view_id").and_then(Value::as_str) else {
            continue;
        };
        grouped
            .entry(group_key.to_string())
            .or_default()
            .push(Value::String(view_id.to_string()));
    }
    if let Some(key) = key {
        grouped.remove(key).map(Value::Array).into_iter().collect()
    } else {
        grouped.into_values().map(Value::Array).collect()
    }
}

fn query_branch_tool_pairs(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let mut matches = Vec::new();
    for record in records.iter().filter(|record| record.kind == "branch") {
        let Some(tool_pairs) = record
            .payload
            .get("current")
            .and_then(|current| current.get("tool_pairs"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        if let Some(key) = key {
            if let Some(value) = tool_pairs.get(key) {
                matches.push(value.clone());
            }
        } else {
            matches.extend(tool_pairs.values().cloned());
        }
    }
    matches
}

fn query_current_head_tool_pairs(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let Ok(root_store) = ledger_from_records(records) else {
        return Vec::new();
    };
    let Some(branch) = root_store.branches.get(&root_store.current_branch_id) else {
        return Vec::new();
    };
    if let Some(key) = key {
        branch
            .current
            .tree
            .tool_pairs
            .get(key)
            .and_then(|pair| serde_json::to_value(pair).ok())
            .into_iter()
            .collect()
    } else {
        branch
            .current
            .tree
            .tool_pairs
            .values()
            .filter_map(|pair| serde_json::to_value(pair).ok())
            .collect()
    }
}

fn query_branch_message_order(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.kind == "branch")
        .filter(|record| {
            key.map(|key| record.payload.get("branch_id").and_then(Value::as_str) == Some(key))
                .unwrap_or(true)
        })
        .filter_map(|record| {
            record
                .payload
                .get("current")
                .and_then(|current| current.get("message_order"))
                .cloned()
        })
        .collect()
}

fn query_nodes_by_depth(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for node in records
        .iter()
        .filter(|record| record.kind == "node")
        .map(|record| &record.payload)
    {
        let Some(component_id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        for depth_key in node_depth_keys(node) {
            push_index_value(&mut grouped, &depth_key, component_id);
        }
    }
    if let Some(key) = key {
        grouped.remove(key).into_iter().collect()
    } else {
        grouped.into_values().collect()
    }
}

fn query_nodes_by_type(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for node in records
        .iter()
        .filter(|record| record.kind == "node")
        .map(|record| &record.payload)
    {
        let Some(component_id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(kind) = node_kind(node) else {
            continue;
        };
        push_index_value(&mut grouped, kind, component_id);
    }
    if let Some(key) = key {
        grouped.remove(key).into_iter().collect()
    } else {
        grouped.into_values().collect()
    }
}

fn query_nodes_by_parent(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for node in records
        .iter()
        .filter(|record| record.kind == "node")
        .map(|record| &record.payload)
    {
        let Some(parent_id) = node.get("parent_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(component_id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        push_index_value(&mut grouped, parent_id, component_id);
    }
    if let Some(key) = key {
        grouped.remove(key).into_iter().collect()
    } else {
        grouped.into_values().collect()
    }
}

fn query_nodes_by_attr(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for node in records
        .iter()
        .filter(|record| record.kind == "node")
        .map(|record| &record.payload)
    {
        let Some(component_id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(aux) = node.get("aux").and_then(Value::as_object) {
            for (attr_key, attr_value) in aux {
                push_node_attr_index_values(&mut grouped, component_id, attr_key, attr_value);
            }
        }
        for attr_key in ["ttl", "cad", "key"] {
            if let Some(attr_value) = node.get(attr_key) {
                push_node_attr_index_values(&mut grouped, component_id, attr_key, attr_value);
            }
        }
        if let Some(attr_value) = node.get("tags") {
            push_node_attr_index_values(&mut grouped, component_id, "tags", attr_value);
            push_node_attr_index_values(&mut grouped, component_id, "tag", attr_value);
        }
    }
    if let Some(key) = key {
        grouped.remove(key).into_iter().collect()
    } else {
        grouped.into_values().collect()
    }
}

fn push_node_attr_index_values(
    grouped: &mut BTreeMap<String, Value>,
    component_id: &str,
    attr_key: &str,
    attr_value: &Value,
) {
    push_index_value(grouped, attr_key, component_id);
    match attr_value {
        Value::String(text) => {
            push_index_value(grouped, &format!("{attr_key}={text}"), component_id);
        }
        Value::Number(number) => {
            push_index_value(grouped, &format!("{attr_key}={number}"), component_id);
        }
        Value::Bool(value) => {
            push_index_value(grouped, &format!("{attr_key}={value}"), component_id);
        }
        Value::Null => {
            push_index_value(grouped, &format!("{attr_key}=null"), component_id);
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    push_index_value(grouped, &format!("{attr_key}={text}"), component_id);
                }
            }
        }
        Value::Object(_) => {}
    }
}

fn query_nodes_by_aux_namespace(records: &[ScannedRecord], key: Option<&str>) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for node in records
        .iter()
        .filter(|record| record.kind == "node")
        .map(|record| &record.payload)
    {
        let Some(component_id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(aux) = node.get("aux").and_then(|aux| aux.get("aux")) {
            collect_aux_namespaces(aux, component_id, &mut grouped);
        }
    }
    if let Some(key) = key {
        grouped.remove(key).into_iter().collect()
    } else {
        grouped.into_values().collect()
    }
}

fn node_depth_keys(node: &Value) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(depth) = node.get("pact_depth").and_then(Value::as_i64) {
        keys.push(format!("d{depth}"));
        keys.push(format!("pact_depth={depth}"));
        keys.push(format!("depth={depth}"));
    }
    if let Some(depth) = node
        .get("aux")
        .and_then(|aux| aux.get("pact_depth").or_else(|| aux.get("depth")))
        .and_then(Value::as_i64)
    {
        keys.push(format!("d{depth}"));
        keys.push(format!("pact_depth={depth}"));
        keys.push(format!("depth={depth}"));
    }
    keys.sort();
    keys.dedup();
    keys
}

pub(super) fn enrich_manifest_from_records(
    manifest: &mut PactFileManifest,
    records: &[ScannedRecord],
) {
    manifest.file_kind = detect_file_kind(records);
    manifest.cycle_ranges = cycle_ranges_from_records(records);
    manifest.cycle_count = manifest.cycle_ranges.len();
    manifest.capabilities = detect_file_capabilities(records, manifest);
}

fn detect_file_kind(records: &[ScannedRecord]) -> String {
    for record in records.iter().filter(|record| record.kind == "branch") {
        if let Some(kind) = record
            .payload
            .get("aux")
            .and_then(|aux| aux.get("pact_file_kind"))
            .and_then(Value::as_str)
        {
            return kind.to_string();
        }
    }
    let branch_count = records
        .iter()
        .filter(|record| record.kind == "branch")
        .count();
    if branch_count <= 1 {
        "branch".to_string()
    } else {
        "full_root".to_string()
    }
}

fn detect_file_capabilities(records: &[ScannedRecord], manifest: &PactFileManifest) -> Vec<String> {
    let mut capabilities = vec![
        "query".to_string(),
        "projection".to_string(),
        "project_head".to_string(),
        "project_revision".to_string(),
        "repair".to_string(),
        format!("content_hash:{CONTENT_HASH_SCHEME}"),
    ];
    if manifest.file_kind == "slice" {
        capabilities.push("slice".to_string());
        capabilities.push("slice_open".to_string());
    }
    if records
        .iter()
        .filter(|record| record.kind == "branch")
        .any(|record| {
            record
                .payload
                .get("aux")
                .and_then(|aux| aux.get("pact_file_healed"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    {
        capabilities.push("is_healed".to_string());
    }
    if manifest.component_count > 0 || manifest.component_transaction_count > 0 {
        capabilities.push("components".to_string());
    }
    if manifest.view_count > 0 {
        capabilities.push("views".to_string());
    }
    if manifest.content_manifest_count > 0 {
        capabilities.push("content_manifests".to_string());
    }
    if records.iter().any(|record| record.kind == "trace") {
        capabilities.push("transaction_traces".to_string());
    }
    if manifest.cycle_count > 0 {
        capabilities.push("cycles".to_string());
    }
    if manifest
        .capabilities
        .iter()
        .any(|capability| capability == TRACE_COMPACT_V2)
    {
        capabilities.push(TRACE_COMPACT_V2.to_string());
    }
    capabilities.sort();
    capabilities.dedup();
    capabilities
}

pub(super) fn cycle_ranges_from_records(records: &[ScannedRecord]) -> Vec<PactFileCycleRange> {
    let mut by_cycle: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "node") {
        if let Some(cycle) = node_cycle(&record.payload) {
            by_cycle.entry(cycle).or_default().push(&record.payload);
        }
    }
    by_cycle
        .into_iter()
        .filter_map(|(cycle, mut nodes)| {
            nodes.sort_by_key(|node| {
                (
                    node.get("created_at_ns")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    node.get("creation_index")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    node.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            });
            let first = nodes.first()?;
            let last = nodes.last()?;
            Some(PactFileCycleRange {
                cycle,
                first_component_id: first
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                last_component_id: last
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                first_created_at_ns: first
                    .get("created_at_ns")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                last_created_at_ns: last
                    .get("created_at_ns")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                node_count: nodes.len(),
                message_count: nodes.iter().filter(|node| is_message_node(node)).count(),
                tool_call_count: nodes
                    .iter()
                    .filter(|node| node_kind(node) == Some("tool_call"))
                    .count(),
                tool_result_count: nodes
                    .iter()
                    .filter(|node| node_kind(node) == Some("tool_result"))
                    .count(),
            })
        })
        .collect()
}

fn filter_nodes_by_cycle(records: &[ScannedRecord], cycle: u64) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.kind == "node")
        .filter(|record| node_cycle(&record.payload) == Some(cycle))
        .map(|record| record.payload.clone())
        .collect()
}

pub(super) fn node_cycle(node: &Value) -> Option<u64> {
    node.get("cycle").and_then(Value::as_u64).or_else(|| {
        node.get("aux")
            .and_then(|aux| aux.get("cycle"))
            .and_then(Value::as_u64)
    })
}

fn node_kind(node: &Value) -> Option<&str> {
    node.get("type")
        .and_then(Value::as_str)
        .or_else(|| node.get("componentType").and_then(Value::as_str))
}

fn is_message_node(node: &Value) -> bool {
    node_kind(node) == Some("message_turn")
}

fn query_index(records: &[ScannedRecord], name: &str, key: Option<&str>) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.kind == "index")
        .filter(|record| record.payload.get("name").and_then(Value::as_str) == Some(name))
        .flat_map(|record| {
            let entries = record
                .payload
                .get("entries")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if let Some(key) = key {
                entries.get(key).cloned().into_iter().collect::<Vec<_>>()
            } else {
                vec![Value::Object(entries)]
            }
        })
        .collect()
}

pub(super) fn filter_projection_array(
    value: Option<&Value>,
    key: Option<&str>,
    field: &str,
) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            key.map(|key| item.get(field).and_then(Value::as_str) == Some(key))
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

pub(super) fn filter_projection_array_any(
    value: Option<&Value>,
    key: Option<&str>,
    fields: &[&str],
) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            key.map(|key| {
                fields
                    .iter()
                    .any(|field| item.get(field).and_then(Value::as_str) == Some(key))
            })
            .unwrap_or(true)
        })
        .cloned()
        .collect()
}
