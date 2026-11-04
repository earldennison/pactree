use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{PactError, PactResult};
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

#[cfg(test)]
mod tests;

const MAX_EVENT_METADATA_STRING_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PactSearchChangeKind {
    FieldUpsert,

    FieldDelete,

    ContextInvalidate,

    VisibilityChange,

    RelationshipChange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PactSearchDependencyScope {
    Component,

    ParentTurn,

    BranchContext,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchChange {
    pub kind: PactSearchChangeKind,

    pub component_id: String,

    pub parent_turn_id: Option<String>,

    pub field: Option<String>,

    pub content_ref: Option<String>,

    pub source_hash: Option<String>,

    pub dependency_scope: PactSearchDependencyScope,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchChangePage {
    pub changes: Vec<PactSearchChange>,

    pub next_cursor: Option<crate::PactSearchChangeCursor>,
}

pub fn file_search_changes(
    path: &Path,
    branch_id: Option<&str>,
    cursor: Option<&crate::PactSearchChangeCursor>,
    max_events: usize,
    max_metadata_bytes: usize,
) -> PactResult<PactSearchChangePage> {
    if max_events == 0 || max_events > 10_000 {
        return Err(PactError::invalid(
            "Pact search-change event limit must be between 1 and 10000",
        ));
    }
    if max_metadata_bytes == 0 || max_metadata_bytes > 16 * 1024 * 1024 {
        return Err(PactError::invalid(
            "Pact search-change metadata budget must be between 1 byte and 16 MiB",
        ));
    }

    let snapshot =
        super::snapshot::Snapshot::open(path, Arc::new(super::backend::IoCounterSet::default()))?;
    let metadata = snapshot.metadata()?;
    let branch_id = branch_id.unwrap_or(&metadata.current_branch_id);
    if snapshot.branch_head(branch_id)?.is_none() {
        return Err(PactError::not_found(format!(
            "branch not found: {branch_id}"
        )));
    }
    let source_file_identity = snapshot.source_file_identity();
    if let Some(cursor) = cursor {
        if cursor.source_file_identity != source_file_identity {
            return Err(PactError::invalid(
                "Pact search-change cursor belongs to another source file",
            ));
        }
        if cursor.root_id != metadata.root_id {
            return Err(PactError::invalid(
                "Pact search-change cursor belongs to another context root",
            ));
        }
        if cursor.branch_id != branch_id {
            return Err(PactError::invalid(
                "Pact search-change cursor belongs to another branch",
            ));
        }
    }
    if !snapshot.has_search_change_facet()? {
        return Err(PactError::unsupported(
            "Pact file predates the bounded search-change facet; rewrite it with the current candidate format",
        ));
    }

    let position = cursor.map(|cursor| (cursor.revision, cursor.ordinal));
    let indexed = snapshot.search_changes(branch_id, position, max_events.saturating_add(1))?;
    let mut changes = Vec::with_capacity(indexed.len().min(max_events));
    let mut metadata_bytes = 0_usize;
    let mut last_position = None;
    for indexed_change in &indexed {
        if changes.len() == max_events {
            break;
        }
        let event_bytes = serde_json::to_vec(&indexed_change.change)
            .map_err(|error| PactError::codec(format!("encode Pact search change: {error}")))?
            .len();
        if metadata_bytes.saturating_add(event_bytes) > max_metadata_bytes {
            if changes.is_empty() {
                return Err(PactError::invalid(
                    "Pact search-change metadata budget is smaller than the next event",
                ));
            }
            break;
        }
        metadata_bytes = metadata_bytes.saturating_add(event_bytes);
        last_position = Some((indexed_change.revision, indexed_change.ordinal));
        changes.push(indexed_change.change.clone());
    }
    let has_more = changes.len() < indexed.len();
    let next_cursor = if has_more {
        last_position.map(|(revision, ordinal)| crate::PactSearchChangeCursor {
            source_file_identity,
            root_id: metadata.root_id,
            branch_id: branch_id.to_string(),
            revision,
            ordinal,
        })
    } else {
        None
    };
    Ok(PactSearchChangePage {
        changes,
        next_cursor,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IndexedSearchChange {
    pub(crate) revision: u64,
    pub(crate) ordinal: u64,
    pub(crate) change: PactSearchChange,
}

pub(crate) fn indexed_search_changes_from_transaction(
    transaction: &super::transaction::PactTransaction,
) -> PactResult<Vec<IndexedSearchChange>> {
    indexed_search_changes_from_trace(&transaction.branch_id, &transaction.trace)
}

pub(crate) fn indexed_search_changes_from_ledger(
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<Vec<IndexedSearchChange>> {
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
    for (branch, _, trace) in traces {
        changes.extend(indexed_search_changes_from_trace(branch, trace)?);
    }
    Ok(changes)
}

const POSITIONAL_FIELDS: &[&str] = &["pact_depth", "depth", "offset", "child_ids"];

fn is_positional_field_op(op: &TraceOp) -> bool {
    match &op.kind {
        TraceOpKind::SetField { key, .. } | TraceOpKind::UnsetField { key } => {
            POSITIONAL_FIELDS.contains(&key.as_str())
        }
        _ => false,
    }
}

pub(crate) fn indexed_search_changes_from_trace(
    _branch: &str,
    trace: &TransactionTrace,
) -> PactResult<Vec<IndexedSearchChange>> {
    trace
        .ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !is_positional_field_op(op))
        .map(|(ordinal, op)| {
            Ok(IndexedSearchChange {
                revision: trace.to_rev,
                ordinal: u64::try_from(ordinal)
                    .map_err(|_| PactError::invalid("Pact search-change ordinal exceeds u64"))?,
                change: change_from_op(op)?,
            })
        })
        .collect()
}

pub(crate) fn search_change_from_op(op: &TraceOp) -> PactResult<Option<PactSearchChange>> {
    if is_positional_field_op(op) {
        return Ok(None);
    }
    change_from_op(op).map(Some)
}

fn change_from_op(op: &TraceOp) -> PactResult<PactSearchChange> {
    let component_id = op.component_id.clone();
    let change = match &op.kind {
        TraceOpKind::InsertComponent {
            parent_id,
            component,
            ..
        } => PactSearchChange {
            kind: PactSearchChangeKind::ContextInvalidate,
            component_id,
            parent_turn_id: component_type(component)
                .is_some_and(|kind| kind == "message_turn")
                .then(|| op.component_id.clone())
                .or_else(|| Some(parent_id.clone())),
            field: None,
            content_ref: content_ref(component)?,
            source_hash: Some(hash_json(component)?),
            dependency_scope: PactSearchDependencyScope::BranchContext,
        },
        TraceOpKind::DeleteComponent => PactSearchChange {
            kind: PactSearchChangeKind::FieldDelete,
            component_id,
            parent_turn_id: None,
            field: None,
            content_ref: None,
            source_hash: None,
            dependency_scope: PactSearchDependencyScope::BranchContext,
        },
        TraceOpKind::MoveComponent { parent_id, .. } => PactSearchChange {
            kind: PactSearchChangeKind::RelationshipChange,
            component_id,
            parent_turn_id: Some(parent_id.clone()),
            field: Some("parent_id".to_string()),
            content_ref: None,
            source_hash: Some(hash_json(&serde_json::json!(parent_id))?),
            dependency_scope: PactSearchDependencyScope::BranchContext,
        },
        TraceOpKind::SetField { key, value } => {
            field_change(component_id, key, Some(value), false)?
        }
        TraceOpKind::UnsetField { key } => field_change(component_id, key, None, true)?,
        TraceOpKind::SetAttr { key, value } => {
            field_change(component_id, &format!("aux.{key}"), Some(value), false)?
        }
        TraceOpKind::UnsetAttr { key } => {
            field_change(component_id, &format!("aux.{key}"), None, true)?
        }
        TraceOpKind::SetContent {
            content,
            content_ref,
            content_preview,
        } => {
            let value = serde_json::json!({
                "content": content,
                "content_ref": content_ref,
                "content_preview": content_preview,
            });
            PactSearchChange {
                kind: if content.is_none() && content_ref.is_none() && content_preview.is_none() {
                    PactSearchChangeKind::FieldDelete
                } else {
                    PactSearchChangeKind::FieldUpsert
                },
                component_id,
                parent_turn_id: None,
                field: Some("content".to_string()),
                content_ref: content_ref.as_ref().map(value_string).transpose()?,
                source_hash: Some(hash_json(&value)?),
                dependency_scope: PactSearchDependencyScope::Component,
            }
        }
        TraceOpKind::ReplaceContentRange { .. } => PactSearchChange {
            kind: PactSearchChangeKind::FieldUpsert,
            component_id,
            parent_turn_id: None,
            field: Some("content".to_string()),
            content_ref: None,
            source_hash: None,
            dependency_scope: PactSearchDependencyScope::Component,
        },
        TraceOpKind::SetLifecycle { value } => PactSearchChange {
            kind: PactSearchChangeKind::VisibilityChange,
            component_id,
            parent_turn_id: None,
            field: Some("lifecycle".to_string()),
            content_ref: None,
            source_hash: Some(hash_json(value)?),
            dependency_scope: PactSearchDependencyScope::ParentTurn,
        },
    };
    Ok(change)
}

fn field_change(
    component_id: String,
    field: &str,
    value: Option<&serde_json::Value>,
    deleted: bool,
) -> PactResult<PactSearchChange> {
    let bare_field = field.strip_prefix("aux.").unwrap_or(field);
    let (kind, dependency_scope) = if bare_field == "visibility" || bare_field == "hidden" {
        (
            PactSearchChangeKind::VisibilityChange,
            PactSearchDependencyScope::ParentTurn,
        )
    } else if bare_field == "tool_call_id" {
        (
            PactSearchChangeKind::RelationshipChange,
            PactSearchDependencyScope::BranchContext,
        )
    } else if matches!(bare_field, "role" | "tool_name" | "success") {
        (
            PactSearchChangeKind::ContextInvalidate,
            PactSearchDependencyScope::ParentTurn,
        )
    } else if deleted {
        (
            PactSearchChangeKind::FieldDelete,
            PactSearchDependencyScope::Component,
        )
    } else {
        (
            PactSearchChangeKind::FieldUpsert,
            PactSearchDependencyScope::Component,
        )
    };
    Ok(PactSearchChange {
        kind,
        component_id,
        parent_turn_id: None,
        field: Some(field.to_string()),
        content_ref: None,
        source_hash: value.map(hash_json).transpose()?,
        dependency_scope,
    })
}

fn component_type(value: &serde_json::Value) -> Option<&str> {
    value
        .get("componentType")
        .or_else(|| value.get("node_type"))
        .and_then(serde_json::Value::as_str)
}

fn content_ref(value: &serde_json::Value) -> PactResult<Option<String>> {
    value.get("content_ref").map(value_string).transpose()
}

fn value_string(value: &serde_json::Value) -> PactResult<String> {
    match value {
        serde_json::Value::String(value) => Ok(value.clone()),
        _ => serde_json::to_string(value)
            .map_err(|error| PactError::codec(format!("encode Pact search content ref: {error}"))),
    }
}

fn hash_json(value: &serde_json::Value) -> PactResult<String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| PactError::codec(format!("encode Pact search-change value: {error}")))?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

pub(crate) fn encode_index_value(change: &IndexedSearchChange) -> PactResult<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PSC2");
    out.push(kind_id(change.change.kind));
    out.push(scope_id(change.change.dependency_scope));
    write_optional_string(&mut out, change.change.parent_turn_id.as_deref())?;
    write_optional_string(&mut out, change.change.field.as_deref())?;
    write_optional_string(&mut out, change.change.content_ref.as_deref())?;
    write_optional_string(&mut out, change.change.source_hash.as_deref())?;
    Ok(out)
}

pub(crate) fn decode_index_value(
    revision: u64,
    ordinal: u64,
    component_id: String,
    bytes: &[u8],
) -> PactResult<IndexedSearchChange> {
    if bytes.get(..4) != Some(b"PSC2") {
        return Err(PactError::corrupt(
            "indexed Pact search change has invalid magic",
        ));
    }
    let kind = decode_kind(
        *bytes
            .get(4)
            .ok_or_else(|| PactError::corrupt("indexed Pact search change is truncated"))?,
    )?;
    let dependency_scope = decode_scope(
        *bytes
            .get(5)
            .ok_or_else(|| PactError::corrupt("indexed Pact search change is truncated"))?,
    )?;
    let mut cursor = 6;
    let parent_turn_id = read_optional_string(bytes, &mut cursor)?;
    let field = read_optional_string(bytes, &mut cursor)?;
    let content_ref = read_optional_string(bytes, &mut cursor)?;
    let source_hash = read_optional_string(bytes, &mut cursor)?;
    if cursor != bytes.len() {
        return Err(PactError::corrupt(
            "indexed Pact search change has trailing bytes",
        ));
    }
    Ok(IndexedSearchChange {
        revision,
        ordinal,
        change: PactSearchChange {
            kind,
            component_id,
            parent_turn_id,
            field,
            content_ref,
            source_hash,
            dependency_scope,
        },
    })
}

fn kind_id(kind: PactSearchChangeKind) -> u8 {
    match kind {
        PactSearchChangeKind::FieldUpsert => 0,
        PactSearchChangeKind::FieldDelete => 1,
        PactSearchChangeKind::ContextInvalidate => 2,
        PactSearchChangeKind::VisibilityChange => 3,
        PactSearchChangeKind::RelationshipChange => 4,
    }
}

fn decode_kind(value: u8) -> PactResult<PactSearchChangeKind> {
    match value {
        0 => Ok(PactSearchChangeKind::FieldUpsert),
        1 => Ok(PactSearchChangeKind::FieldDelete),
        2 => Ok(PactSearchChangeKind::ContextInvalidate),
        3 => Ok(PactSearchChangeKind::VisibilityChange),
        4 => Ok(PactSearchChangeKind::RelationshipChange),
        _ => Err(PactError::corrupt("unknown Pact search change kind")),
    }
}

fn scope_id(scope: PactSearchDependencyScope) -> u8 {
    match scope {
        PactSearchDependencyScope::Component => 0,
        PactSearchDependencyScope::ParentTurn => 1,
        PactSearchDependencyScope::BranchContext => 2,
    }
}

fn decode_scope(value: u8) -> PactResult<PactSearchDependencyScope> {
    match value {
        0 => Ok(PactSearchDependencyScope::Component),
        1 => Ok(PactSearchDependencyScope::ParentTurn),
        2 => Ok(PactSearchDependencyScope::BranchContext),
        _ => Err(PactError::corrupt("unknown Pact search dependency scope")),
    }
}

fn write_optional_string(out: &mut Vec<u8>, value: Option<&str>) -> PactResult<()> {
    let Some(value) = value else {
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        return Ok(());
    };
    if value.len() > MAX_EVENT_METADATA_STRING_BYTES {
        return Err(PactError::invalid(
            "Pact search-change metadata string exceeds 64 KiB",
        ));
    }
    let len = u32::try_from(value.len())
        .map_err(|_| PactError::invalid("Pact search-change metadata exceeds u32"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn read_optional_string(bytes: &[u8], cursor: &mut usize) -> PactResult<Option<String>> {
    let len_bytes = bytes
        .get(*cursor..cursor.saturating_add(4))
        .ok_or_else(|| PactError::corrupt("indexed Pact search change is truncated"))?;
    *cursor += 4;
    let len = u32::from_le_bytes(
        len_bytes
            .try_into()
            .map_err(|_| PactError::corrupt("indexed Pact search change length is invalid"))?,
    );
    if len == u32::MAX {
        return Ok(None);
    }
    let len = usize::try_from(len)
        .map_err(|_| PactError::corrupt("indexed Pact search change exceeds platform limits"))?;
    if len > MAX_EVENT_METADATA_STRING_BYTES {
        return Err(PactError::corrupt(
            "indexed Pact search-change metadata string exceeds 64 KiB",
        ));
    }
    let value = bytes
        .get(*cursor..cursor.saturating_add(len))
        .ok_or_else(|| PactError::corrupt("indexed Pact search change length overruns"))?;
    *cursor += len;
    String::from_utf8(value.to_vec())
        .map(Some)
        .map_err(|_| PactError::corrupt("indexed Pact search change is not UTF-8"))
}
