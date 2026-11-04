use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{PactError, PactResult};
use crate::store::{ContentRecord, ContextTreeLedger};
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactTextChangeCursor {
    pub file_identity: String,
    pub generation: u64,
    pub ordinal: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactTextChange {
    pub node_id: String,
    pub branch_id: String,
    pub cycle: Option<u64>,
    pub field: String,
    pub text_hash: String,
    pub content_ref: Option<String>,
    pub tombstone: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactTextChangePage {
    pub changes: Vec<PactTextChange>,
    pub next_cursor: Option<PactTextChangeCursor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct IndexedTextChange {
    pub(crate) revision: u64,
    pub(crate) ordinal: u64,
    pub(crate) change: PactTextChange,
}

pub fn file_text_changes(
    path: &Path,
    cursor: Option<&PactTextChangeCursor>,
    limit: usize,
) -> PactResult<PactTextChangePage> {
    stream_text_changes(path, cursor, limit)
}

pub(crate) fn stream_text_changes(
    path: &Path,
    cursor: Option<&PactTextChangeCursor>,
    limit: usize,
) -> PactResult<PactTextChangePage> {
    if limit == 0 || limit > 10_000 {
        return Err(PactError::invalid(
            "Pact text change limit must be between 1 and 10000",
        ));
    }
    let snapshot =
        super::snapshot::Snapshot::open(path, Arc::new(super::backend::IoCounterSet::default()))?;
    let metadata = snapshot.metadata()?;
    let file_identity = metadata.root_id.clone();
    if cursor.is_some_and(|cursor| cursor.file_identity != file_identity) {
        return Err(PactError::invalid(
            "Pact text change cursor belongs to another logical file",
        ));
    }
    let branch_id = metadata.current_branch_id;
    if snapshot.has_text_change_facet()? {
        let position = cursor.map(|cursor| (cursor.generation, cursor.ordinal));
        let changes = snapshot.text_changes(&branch_id, position, limit.saturating_add(1))?;
        return Ok(page_from_indexed(file_identity, changes, limit));
    }

    let mut traces = snapshot.public_traces(&branch_id)?;
    traces.sort_by(|left, right| {
        let left_revision = left
            .replay_trace
            .as_ref()
            .map_or(u64::MAX, |trace| trace.to_rev);
        let right_revision = right
            .replay_trace
            .as_ref()
            .map_or(u64::MAX, |trace| trace.to_rev);
        (left_revision, left.transaction_id.as_str())
            .cmp(&(right_revision, right.transaction_id.as_str()))
    });
    let mut changes = Vec::with_capacity(limit.saturating_add(1));
    for trace in traces {
        let Some(replay) = trace.replay_trace else {
            continue;
        };
        let mut indexed =
            indexed_text_changes_from_trace(&branch_id, &replay, &mut |content_id| {
                snapshot
                    .content(content_id)?
                    .map(|record| content_record_bytes(&record))
                    .transpose()
            })?;
        indexed.retain(|change| {
            cursor.is_none_or(|cursor| {
                (change.revision, change.ordinal) > (cursor.generation, cursor.ordinal)
            })
        });
        let remaining = limit.saturating_add(1).saturating_sub(changes.len());
        changes.extend(indexed.into_iter().take(remaining));
        if changes.len() > limit {
            break;
        }
    }
    Ok(page_from_indexed(file_identity, changes, limit))
}

pub(crate) fn indexed_text_changes_from_transaction(
    transaction: &super::transaction::PactTransaction,
) -> PactResult<Vec<IndexedTextChange>> {
    let content = transaction
        .new_content
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    indexed_text_changes_from_trace(
        &transaction.branch_id,
        &transaction.trace,
        &mut |content_id| {
            content
                .get(content_id)
                .map(|record| content_record_bytes(record))
                .transpose()
        },
    )
}

pub(crate) fn indexed_text_changes_from_ledger(
    ledger: &ContextTreeLedger,
) -> PactResult<Vec<IndexedTextChange>> {
    let mut traces = ledger
        .event_index
        .transaction_traces
        .values()
        .filter_map(|trace| {
            trace.replay_trace.as_ref().map(|replay| {
                (
                    trace.branch_id.as_str(),
                    trace.transaction_id.as_str(),
                    replay,
                )
            })
        })
        .collect::<Vec<_>>();
    traces.sort_by(|left, right| {
        (left.0, left.2.to_rev, left.1).cmp(&(right.0, right.2.to_rev, right.1))
    });
    let mut changes = Vec::new();
    for (branch_id, _, trace) in traces {
        changes.extend(indexed_text_changes_from_trace(
            branch_id,
            trace,
            &mut |content_id| {
                ledger
                    .content
                    .records
                    .get(content_id)
                    .map(content_record_bytes)
                    .transpose()
            },
        )?);
    }
    Ok(changes)
}

pub(crate) fn encode_index_value(change: &IndexedTextChange) -> PactResult<Vec<u8>> {
    let mut flags = 0_u8;
    if change.change.tombstone {
        flags |= 1;
    }
    if change.change.content_ref.is_some() {
        flags |= 2;
    }
    if !change.change.tombstone && change.change.content_ref.is_none() {
        flags |= 4;
    }
    if change.change.tombstone && !change.change.text_hash.is_empty() {
        return Err(PactError::invalid(
            "Pact text-change tombstone cannot carry a text hash",
        ));
    }
    if flags & 4 != 0 && change.change.text_hash.len() != 64 {
        return Err(PactError::invalid(
            "Pact inline text change requires a BLAKE3 hash",
        ));
    }
    let content_ref = change.change.content_ref.as_deref().unwrap_or_default();
    let content_ref_len = u32::try_from(content_ref.len())
        .map_err(|_| PactError::invalid("Pact text-change content ref exceeds u32"))?;
    let mut bytes = Vec::with_capacity(9 + content_ref.len());
    bytes.extend_from_slice(b"PTC2");
    bytes.push(flags);
    bytes.extend_from_slice(&content_ref_len.to_le_bytes());
    bytes.extend_from_slice(content_ref.as_bytes());
    Ok(bytes)
}

pub(crate) fn decode_index_value(
    revision: u64,
    ordinal: u64,
    branch: &str,
    node: &str,
    field: &str,
    bytes: &[u8],
) -> PactResult<IndexedTextChange> {
    if bytes.get(..4) != Some(b"PTC2") {
        return Err(PactError::corrupt(
            "indexed Pact text change has invalid magic",
        ));
    }
    let flags = *bytes
        .get(4)
        .ok_or_else(|| PactError::corrupt("indexed Pact text change is truncated"))?;
    if flags & !7 != 0 {
        return Err(PactError::corrupt(
            "indexed Pact text change has unknown flags",
        ));
    }
    let has_stored_hash = flags & 7 == 0;
    let ref_offset = if has_stored_hash { 37 } else { 5 };
    let hash = if has_stored_hash {
        Some(
            bytes
                .get(5..37)
                .ok_or_else(|| PactError::corrupt("indexed Pact text-change hash is truncated"))?
                .try_into()
                .map_err(|_| {
                    PactError::corrupt("indexed Pact text-change hash has invalid width")
                })?,
        )
    } else {
        None
    };
    let content_ref_len = u32::from_le_bytes(
        bytes
            .get(ref_offset..ref_offset + 4)
            .ok_or_else(|| PactError::corrupt("indexed Pact text-change ref is truncated"))?
            .try_into()
            .map_err(|_| PactError::corrupt("indexed Pact text-change ref length is invalid"))?,
    ) as usize;
    let content_ref_bytes = bytes
        .get(ref_offset + 4..)
        .filter(|value| value.len() == content_ref_len)
        .ok_or_else(|| PactError::corrupt("indexed Pact text-change ref length mismatches"))?;
    let content_ref = if flags & 2 == 0 {
        if !content_ref_bytes.is_empty() {
            return Err(PactError::corrupt(
                "indexed Pact text change has an unflagged content ref",
            ));
        }
        None
    } else {
        Some(
            std::str::from_utf8(content_ref_bytes)
                .map_err(|_| PactError::corrupt("indexed Pact text-change ref is not UTF-8"))?
                .to_string(),
        )
    };
    let tombstone = flags & 1 != 0;
    Ok(IndexedTextChange {
        revision,
        ordinal,
        change: PactTextChange {
            node_id: node.to_string(),
            branch_id: branch.to_string(),
            cycle: Some(revision),
            field: field.to_string(),
            text_hash: if tombstone {
                String::new()
            } else if let Some(hash) = hash {
                blake3::Hash::from_bytes(hash).to_hex().to_string()
            } else {
                String::new()
            },
            content_ref,
            tombstone,
        },
    })
}

fn page_from_indexed(
    file_identity: String,
    mut changes: Vec<IndexedTextChange>,
    limit: usize,
) -> PactTextChangePage {
    let has_more = changes.len() > limit;
    if has_more {
        changes.pop();
    }
    let next_cursor = has_more.then(|| {
        let change = changes
            .last()
            .expect("a page with more changes has a last returned change");
        PactTextChangeCursor {
            file_identity,
            generation: change.revision,
            ordinal: change.ordinal,
        }
    });
    PactTextChangePage {
        changes: changes.into_iter().map(|change| change.change).collect(),
        next_cursor,
    }
}

pub(crate) fn indexed_text_changes_from_trace<F>(
    branch_id: &str,
    trace: &TransactionTrace,
    resolve_content: &mut F,
) -> PactResult<Vec<IndexedTextChange>>
where
    F: FnMut(&str) -> PactResult<Option<Vec<u8>>>,
{
    let mut changes = Vec::new();
    for op in &trace.ops {
        for change in changes_from_op(branch_id, trace.to_rev, op, resolve_content)? {
            changes.push(IndexedTextChange {
                revision: trace.to_rev,
                ordinal: changes.len() as u64,
                change,
            });
        }
    }
    Ok(changes)
}

pub(crate) fn text_changes_from_op<F>(
    branch_id: &str,
    revision: u64,
    op: &TraceOp,
    resolve_content: &mut F,
) -> PactResult<Vec<PactTextChange>>
where
    F: FnMut(&str) -> PactResult<Option<Vec<u8>>>,
{
    changes_from_op(branch_id, revision, op, resolve_content)
}

fn changes_from_op<F>(
    branch_id: &str,
    revision: u64,
    op: &TraceOp,
    resolve_content: &mut F,
) -> PactResult<Vec<PactTextChange>>
where
    F: FnMut(&str) -> PactResult<Option<Vec<u8>>>,
{
    let mut changes = Vec::new();
    match &op.kind {
        TraceOpKind::InsertComponent { component, .. } => {
            for (field, reference_field) in [
                ("content", "content_ref"),
                ("parameters", "parameters_ref"),
                ("result", "result_ref"),
                ("error_message", "error_ref"),
            ] {
                if let Some(value) = component.get(field) {
                    changes.push(inline_change(branch_id, revision, op, field, value)?);
                } else if let Some(content_id) =
                    component.get(reference_field).and_then(content_ref_id)
                {
                    changes.push(content_change(
                        branch_id,
                        revision,
                        op,
                        field,
                        content_id,
                        resolve_content,
                    )?);
                }
            }
        }
        TraceOpKind::SetContent {
            content,
            content_ref,
            ..
        } => {
            if let Some(content) = content {
                changes.push(text_change(branch_id, revision, op, "content", content));
            } else if let Some(content_id) = content_ref.as_ref().and_then(content_ref_id) {
                changes.push(content_change(
                    branch_id,
                    revision,
                    op,
                    "content",
                    content_id,
                    resolve_content,
                )?);
            } else {
                changes.push(tombstone(branch_id, revision, op, "content"));
            }
        }
        TraceOpKind::SetField { key, value } if canonical_text_field(key).is_some() => {
            let field = canonical_text_field(key).expect("guarded text field");
            if key.ends_with("_ref") {
                if let Some(content_id) = content_ref_id(value) {
                    changes.push(content_change(
                        branch_id,
                        revision,
                        op,
                        field,
                        content_id,
                        resolve_content,
                    )?);
                }
            } else {
                changes.push(inline_change(branch_id, revision, op, field, value)?);
            }
        }
        TraceOpKind::UnsetField { key } if canonical_text_field(key).is_some() => {
            changes.push(tombstone(
                branch_id,
                revision,
                op,
                canonical_text_field(key).expect("guarded text field"),
            ));
        }
        TraceOpKind::ReplaceContentRange { .. } => {
            changes.push(tombstone(branch_id, revision, op, "content"));
        }
        TraceOpKind::DeleteComponent => {
            changes.push(tombstone(branch_id, revision, op, "*"));
        }
        _ => {}
    }
    Ok(changes)
}

fn canonical_text_field(key: &str) -> Option<&'static str> {
    match key {
        "content" | "content_ref" => Some("content"),
        "parameters" | "parameters_ref" => Some("parameters"),
        "result" | "result_ref" => Some("result"),
        "error_message" | "error_ref" => Some("error_message"),
        _ => None,
    }
}

fn inline_change(
    branch_id: &str,
    revision: u64,
    op: &TraceOp,
    field: &str,
    value: &serde_json::Value,
) -> PactResult<PactTextChange> {
    let text = value.as_str().map(str::to_string).map_or_else(
        || {
            serde_json::to_string(value)
                .map_err(|error| PactError::codec(format!("encode Pact text change: {error}")))
        },
        Ok,
    )?;
    Ok(text_change(branch_id, revision, op, field, &text))
}

fn content_change<F>(
    branch_id: &str,
    revision: u64,
    op: &TraceOp,
    field: &str,
    content_id: String,
    resolve_content: &mut F,
) -> PactResult<PactTextChange>
where
    F: FnMut(&str) -> PactResult<Option<Vec<u8>>>,
{
    let mut change = resolve_content(&content_id)?.map_or_else(
        || PactTextChange {
            node_id: op.component_id.clone(),
            branch_id: branch_id.to_string(),
            cycle: Some(revision),
            field: field.to_string(),
            text_hash: String::new(),
            content_ref: None,
            tombstone: false,
        },
        |bytes| text_change_bytes(branch_id, revision, op, field, &bytes),
    );
    change.content_ref = Some(content_id);
    Ok(change)
}

pub(crate) fn content_record_bytes(content: &ContentRecord) -> PactResult<Vec<u8>> {
    if let Some(text) = &content.storage.inline {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(bytes) = &content.storage.inline_bytes {
        return Ok(bytes.clone());
    }
    Err(PactError::corrupt(
        "Pact text change content has no local payload",
    ))
}

fn text_change(
    branch_id: &str,
    revision: u64,
    op: &TraceOp,
    field: &str,
    text: &str,
) -> PactTextChange {
    text_change_bytes(branch_id, revision, op, field, text.as_bytes())
}

fn text_change_bytes(
    branch_id: &str,
    revision: u64,
    op: &TraceOp,
    field: &str,
    bytes: &[u8],
) -> PactTextChange {
    PactTextChange {
        node_id: op.component_id.clone(),
        branch_id: branch_id.to_string(),
        cycle: Some(revision),
        field: field.to_string(),
        text_hash: blake3::hash(bytes).to_hex().to_string(),
        content_ref: None,
        tombstone: false,
    }
}

fn tombstone(branch_id: &str, revision: u64, op: &TraceOp, field: &str) -> PactTextChange {
    PactTextChange {
        node_id: op.component_id.clone(),
        branch_id: branch_id.to_string(),
        cycle: Some(revision),
        field: field.to_string(),
        text_hash: String::new(),
        content_ref: None,
        tombstone: true,
    }
}

fn content_ref_id(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string).or_else(|| {
        let object = value.as_object()?;
        ["id", "content_id", "ref_id", "content_ref"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(serde_json::Value::as_str))
            .map(str::to_string)
    })
}
