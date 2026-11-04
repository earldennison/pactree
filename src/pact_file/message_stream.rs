use serde_json::{Value, json};

use super::index::IndexKey;
use super::snapshot::Snapshot;
use crate::error::{PactError, PactResult};
use crate::file_format::{PACT_FILE_HEAD_SCHEMA, PactFileHead};
use crate::store::PactRef;

#[cfg(test)]
mod tests;

#[allow(clippy::too_many_arguments)]
pub(crate) fn window(
    snapshot: &Snapshot,
    path_display: &str,
    branch: &str,
    anchor_row_id: Option<&str>,
    anchor_seq: Option<u64>,
    direction: &str,
    limit: usize,
    source: &str,
) -> PactResult<Value> {
    let direction = normalize_direction(direction)?;
    let metadata = snapshot.metadata()?;
    if metadata.composition_branches.contains(branch) {
        return Err(PactError::unsupported(
            "message_surface requires a directly indexed branch",
        ));
    }
    let head_revision = snapshot
        .branch_head(branch)?
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch}")))?;
    let node_count = metadata
        .node_counts
        .get(branch)
        .copied()
        .ok_or_else(|| PactError::corrupt("Pact head metadata is missing branch node count"))?;
    let transaction_id = metadata.head_transaction_ids.get(branch).cloned();
    let derived_depth = metadata.derived_depth_branches.contains(branch);
    let head = PactFileHead {
        schema: PACT_FILE_HEAD_SCHEMA.to_string(),
        file_path: path_display.to_string(),
        file_kind: "branch".to_string(),
        root_id: metadata.root_id.clone(),
        branch_id: branch.to_string(),
        current_branch_id: metadata.current_branch_id.clone(),
        is_current_branch: branch == metadata.current_branch_id,
        revision: head_revision,
        transaction_id: transaction_id.clone(),
        pact_ref: PactRef {
            root_id: metadata.root_id,
            branch_id: branch.to_string(),
            revision: Some(head_revision),
            transaction_id,
            component_id: None,
            content_ref: None,
        },
        branch_count: metadata.branch_count,
        trace_count: metadata.trace_count,
        record_count: node_count + metadata.trace_count + 3,
        node_count,
    };
    let range_start = IndexKey::MessageOrder {
        branch: branch.to_string(),
        order: 0,
    };
    let range_end = IndexKey::MessageOrder {
        branch: branch.to_string(),
        order: u64::MAX,
    };
    let (total_rows, _) = snapshot.index_range_window(&range_start, &range_end, 0, 0)?;
    let anchor_index = resolve_anchor(
        snapshot,
        branch,
        &range_start,
        &range_end,
        total_rows,
        anchor_row_id,
        anchor_seq,
    )?;
    let limit = limit.clamp(1, 500);
    let (start, end) = window_bounds(direction, anchor_index, total_rows, limit);
    let (_, indexed_rows) =
        snapshot.index_range_window(&range_start, &range_end, start, end.saturating_sub(start))?;
    snapshot.record_rows_read(indexed_rows.len());
    let rows = indexed_rows
        .into_iter()
        .enumerate()
        .map(|(offset, (key, component_id))| {
            let IndexKey::MessageOrder { .. } = IndexKey::decode(&key)? else {
                return Err(PactError::corrupt(
                    "Pact message range returned a non-message key",
                ));
            };
            let component_id = String::from_utf8(component_id)
                .map_err(|_| PactError::corrupt("Pact message component id is not UTF-8"))?;
            let indexed = snapshot
                .component_shallow(branch, &component_id)?
                .ok_or_else(|| {
                    PactError::corrupt("Pact message-order index references a missing component")
                })?;
            let mut payload = snapshot
                .component_subtree(branch, &component_id)?
                .ok_or_else(|| {
                    PactError::corrupt("Pact message-order index references a missing subtree")
                })?;
            hydrate_content(snapshot, &mut payload)?;





            let seq = start.saturating_add(offset);
            if derived_depth {
                set_derived_depth(&mut payload, derived_depth_of_position(total_rows, seq));
            }
            let row_id = format!("node:{component_id}");
            let cycle = node_cycle(&payload);
            Ok(json!({
                "row_id": row_id,
                "kind": "node",
                "component_id": component_id,
                "node_type": payload.get("node_type").or_else(|| payload.get("componentType")).cloned().unwrap_or(Value::Null),
                "role": payload.get("role").cloned().unwrap_or(Value::Null),
                "cycle": cycle,
                "revision": indexed.revision,
                "payload": payload,
                "seq": seq,
                "cursor": {
                    "row_id": row_id,
                    "seq": seq,
                    "revision": head_revision,
                    "cycle": head_revision,
                },
            }))
        })
        .collect::<PactResult<Vec<_>>>()?;
    let before_cursor = rows
        .first()
        .and_then(|row| row.get("cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    let after_cursor = rows
        .last()
        .and_then(|row| row.get("cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "schema": "pact.stream.v1",
        "path": path_display,
        "surface": "context",
        "mode": "message_surface",
        "source": source,
        "fallback_used": false,
        "direction": direction,
        "head": head,
        "anchor": {"row_id": anchor_row_id, "seq": anchor_seq},
        "rows": rows,
        "cursors": {"before": before_cursor, "after": after_cursor},
        "position": {
            "start_index": start,
            "end_index": end.saturating_sub(1),
            "total_rows": total_rows,
            "estimated": false,
        },
        "has_before": start > 0,
        "has_after": end < total_rows,
    }))
}

fn normalize_direction(direction: &str) -> PactResult<&'static str> {
    match direction.trim().to_lowercase().as_str() {
        "before" | "up" => Ok("before"),
        "after" | "down" => Ok("after"),
        "around" => Ok("around"),
        other => Err(PactError::invalid(format!(
            "unsupported stream direction {other:?}; expected before, after, or around"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_anchor(
    snapshot: &Snapshot,
    branch: &str,
    range_start: &IndexKey,
    range_end: &IndexKey,
    total_rows: usize,
    anchor_row_id: Option<&str>,
    anchor_seq: Option<u64>,
) -> PactResult<Option<usize>> {
    if let Some(row_id) = anchor_row_id {
        let component_id = row_id.strip_prefix("node:").unwrap_or(row_id);
        let order = snapshot
            .component_shallow(branch, component_id)?
            .filter(|component| {
                component
                    .value
                    .get("node_type")
                    .or_else(|| component.value.get("componentType"))
                    .and_then(Value::as_str)
                    == Some("message_turn")
            })
            .and_then(|component| component.order)
            .ok_or_else(|| {
                PactError::not_found(format!("message stream anchor row not found: {row_id}"))
            })?;
        let anchor_key = IndexKey::MessageOrder {
            branch: branch.to_string(),
            order,
        };
        let (index, _) = snapshot.index_range_window(range_start, &anchor_key, 0, 0)?;
        let (_, candidate) = snapshot.index_range_window(range_start, range_end, index, 1)?;
        let matches = candidate
            .first()
            .is_some_and(|(_, value)| value.as_slice() == component_id.as_bytes());
        if !matches || index >= total_rows {
            return Err(PactError::not_found(format!(
                "message stream anchor row not found: {row_id}"
            )));
        }
        if let Some(requested_seq) = anchor_seq
            && requested_seq
                != u64::try_from(index)
                    .map_err(|_| PactError::invalid("message stream anchor position exceeds u64"))?
        {
            return Err(PactError::invalid(format!(
                "stale message stream anchor {row_id}: seq {requested_seq} is now {index}"
            )));
        }
        return Ok(Some(index));
    }
    let Some(_anchor_seq) = anchor_seq else {
        return Ok(None);
    };
    Err(PactError::unsupported(
        "message_surface anchor_seq requires anchor_row_id to prevent positional aliasing",
    ))
}

fn window_bounds(
    direction: &str,
    anchor_index: Option<usize>,
    total_rows: usize,
    limit: usize,
) -> (usize, usize) {
    match (direction, anchor_index) {
        ("before", Some(index)) => (index.saturating_sub(limit), index),
        ("after", Some(index)) => {
            let start = index.saturating_add(1).min(total_rows);
            (start, start.saturating_add(limit).min(total_rows))
        }
        ("around", Some(index)) => {
            let start = index.saturating_sub(limit / 2);
            (start, start.saturating_add(limit).min(total_rows))
        }
        ("after", None) => (0, limit.min(total_rows)),
        _ => {
            let start = total_rows.saturating_sub(limit);
            (start, total_rows)
        }
    }
}

pub(crate) fn hydrate_content(snapshot: &Snapshot, value: &mut Value) -> PactResult<()> {
    match value {
        Value::Object(object) => {
            let content_id = object
                .get("content_ref")
                .and_then(|reference| {
                    reference
                        .as_str()
                        .or_else(|| reference.get("id").and_then(Value::as_str))
                })
                .map(str::to_string);
            let content_is_missing = object
                .get("content")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty);
            if content_is_missing
                && let Some(content_id) = content_id
                && let Some(content) = snapshot.content(&content_id)?
            {
                let text = if let Some(text) = content.storage.inline {
                    Some(text)
                } else if let Some(bytes) = content.storage.inline_bytes {
                    String::from_utf8(bytes).ok()
                } else {
                    None
                };
                if let Some(text) = text {
                    object.insert("content".to_string(), Value::String(text));
                }
            }
            for child in object.values_mut() {
                hydrate_content(snapshot, child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                hydrate_content(snapshot, item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn derived_depth_of_position(total_rows: usize, position: usize) -> i64 {
    total_rows.saturating_sub(1).saturating_sub(position) as i64
}

pub(crate) fn derived_depth_of_order(
    snapshot: &Snapshot,
    branch: &str,
    order: u64,
) -> PactResult<i64> {
    let range_start = IndexKey::MessageOrder {
        branch: branch.to_string(),
        order: 0,
    };
    let range_end = IndexKey::MessageOrder {
        branch: branch.to_string(),
        order: u64::MAX,
    };
    let (total_rows, _) = snapshot.index_range_window(&range_start, &range_end, 0, 0)?;
    let anchor = IndexKey::MessageOrder {
        branch: branch.to_string(),
        order,
    };
    let (position, _) = snapshot.index_range_window(&range_start, &anchor, 0, 0)?;
    Ok(derived_depth_of_position(total_rows, position))
}

pub(crate) fn set_derived_depth(value: &mut Value, depth: i64) {
    let Value::Object(object) = value else {
        return;
    };
    object.insert("pact_depth".to_string(), Value::Number(depth.into()));
    if let Some(Value::Object(aux)) = object.get_mut("aux") {
        for key in ["pact_depth", "depth"] {
            if aux.contains_key(key) {
                aux.insert(key.to_string(), Value::Number(depth.into()));
            }
        }
    }
    if let Some(Value::Array(children)) = object.get_mut("children") {
        for child in children {
            set_derived_depth(child, depth);
        }
    }
}

fn node_cycle(node: &Value) -> Option<u64> {
    node.get("cycle")
        .and_then(Value::as_u64)
        .or_else(|| node.get("transaction_index").and_then(Value::as_u64))
        .or_else(|| node.pointer("/aux/cycle").and_then(Value::as_u64))
}
