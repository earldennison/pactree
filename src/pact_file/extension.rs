use crate::error::{PactError, PactResult};
use crate::extension::{
    IndexedExtensionLocator, IndexedExtensionValue, PACT_EXTENSION_PAGE_SCHEMA,
    PactExtensionCursor, PactExtensionPage, PactExtensionRow,
};

use super::index::IndexKey;
use super::snapshot::Snapshot;

pub(crate) fn window(
    snapshot: &Snapshot,
    branch: &str,
    kind: Option<&str>,
    index_key: Option<&str>,
    anchor: Option<&PactExtensionCursor>,
    direction: &str,
    limit: usize,
) -> PactResult<PactExtensionPage> {
    window_at(
        snapshot, branch, kind, index_key, anchor, direction, limit, None,
    )
}

pub(crate) fn window_at(
    snapshot: &Snapshot,
    branch: &str,
    kind: Option<&str>,
    index_key: Option<&str>,
    anchor: Option<&PactExtensionCursor>,
    direction: &str,
    limit: usize,
    at_revision: Option<u64>,
) -> PactResult<PactExtensionPage> {
    let limit = limit.clamp(1, 500);
    let direction = direction.trim().to_ascii_lowercase();
    if direction != "before" && direction != "after" {
        return Err(PactError::invalid(format!(
            "Pact extension direction must be before or after, got {direction}"
        )));
    }
    if index_key.is_some() && kind.is_none() {
        return Err(PactError::invalid(
            "Pact extension index_key requires a kind",
        ));
    }
    let reverse = direction == "before";
    let branch_head = snapshot
        .branch_head(branch)?
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch}")))?;
    let head = at_revision.unwrap_or(branch_head);
    if head > branch_head {
        return Err(PactError::invalid(format!(
            "requested Pact extension revision {head} exceeds branch head {branch_head}"
        )));
    }
    if let Some(anchor) = anchor
        && anchor.revision > head
    {
        return Err(PactError::invalid(format!(
            "stale Pact extension cursor revision {} exceeds head {}",
            anchor.revision, head
        )));
    }
    if let Some(anchor) = anchor {
        let anchored = record(snapshot, branch, &anchor.record_id).map_err(|_| {
            PactError::invalid("Pact extension cursor record is not present at this head")
        })?;
        if anchored.revision != anchor.revision || anchored.ordinal != anchor.ordinal {
            return Err(PactError::invalid(
                "Pact extension cursor does not match the indexed record position",
            ));
        }
        if let Some(kind) = kind
            && anchored.record.kind != kind
        {
            return Err(PactError::invalid(
                "Pact extension cursor belongs to another kind stream",
            ));
        }
        if let Some(index_key) = index_key
            && !anchored
                .record
                .index_keys
                .iter()
                .any(|key| key == index_key)
        {
            return Err(PactError::invalid(
                "Pact extension cursor belongs to another secondary-index stream",
            ));
        }
    }

    let (start, end, anchor_key) = range_keys(branch, kind, index_key, anchor, head, &direction)?;
    let entries = snapshot.index_range(&start, &end, limit + 2, reverse)?;
    let mut rows = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let (revision, ordinal) = extension_position(&IndexKey::decode(&key)?)?;
        let row = if index_key.is_some() {
            let locator =
                serde_json::from_slice::<IndexedExtensionLocator>(&value).map_err(|error| {
                    PactError::corrupt(format!(
                        "invalid Pact extension secondary-index locator: {error}"
                    ))
                })?;
            let row = record(snapshot, branch, &locator.record_id)?;
            if row.revision != revision || row.ordinal != ordinal {
                return Err(PactError::corrupt(
                    "Pact extension secondary index points at another logical position",
                ));
            }
            row
        } else {
            let indexed =
                serde_json::from_slice::<IndexedExtensionValue>(&value).map_err(|error| {
                    PactError::corrupt(format!("invalid Pact extension index value: {error}"))
                })?;
            PactExtensionRow {
                branch_id: branch.to_string(),
                revision,
                transaction_id: indexed.transaction_id,
                ordinal,
                record: indexed.record,
            }
        };
        if anchor_key
            .as_ref()
            .is_some_and(|anchor_key| anchor_key == &key)
        {
            continue;
        }
        if let Some(index_key) = index_key
            && !row.record.index_keys.iter().any(|key| key == index_key)
        {
            continue;
        }
        rows.push(row);
    }
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    if reverse {
        rows.reverse();
    }
    snapshot.record_rows_read(rows.len());

    let (has_before, has_after) = if reverse {
        (has_more, anchor.is_some())
    } else {
        (anchor.is_some(), has_more)
    };
    Ok(PactExtensionPage {
        schema: PACT_EXTENSION_PAGE_SCHEMA.to_string(),
        branch_id: branch.to_string(),
        kind: kind.map(str::to_string),
        index_key: index_key.map(str::to_string),
        before_cursor: rows.first().map(PactExtensionRow::cursor),
        after_cursor: rows.last().map(PactExtensionRow::cursor),
        rows,
        has_before,
        has_after,
    })
}

fn range_keys(
    branch: &str,
    kind: Option<&str>,
    index_key: Option<&str>,
    anchor: Option<&PactExtensionCursor>,
    head: u64,
    direction: &str,
) -> PactResult<(IndexKey, IndexKey, Option<Vec<u8>>)> {
    let start_position = PactExtensionCursor {
        revision: 0,
        ordinal: 0,
        record_id: String::new(),
    };
    let end_position = PactExtensionCursor {
        revision: head,
        ordinal: u64::MAX,
        record_id: String::new(),
    };
    let mut start = extension_key(branch, kind, index_key, &start_position);
    let mut end = extension_key(branch, kind, index_key, &end_position);
    if let Some(anchor) = anchor {
        if direction == "before" {
            end = extension_key(branch, kind, index_key, anchor);
        } else {
            start = extension_key(branch, kind, index_key, anchor);
        }
    }
    let anchor_key = anchor
        .map(|cursor| extension_key(branch, kind, index_key, cursor).encode())
        .transpose()?;
    Ok((start, end, anchor_key))
}

fn extension_key(
    branch: &str,
    kind: Option<&str>,
    index_key: Option<&str>,
    cursor: &PactExtensionCursor,
) -> IndexKey {
    match (kind, index_key) {
        (Some(kind), Some(index_key)) => IndexKey::ExtensionKey {
            branch: branch.to_string(),
            kind: kind.to_string(),
            key_hash: IndexKey::extension_key_hash(index_key),
            revision: cursor.revision,
            ordinal: cursor.ordinal,
        },
        (Some(kind), None) => IndexKey::ExtensionKind {
            branch: branch.to_string(),
            kind: kind.to_string(),
            revision: cursor.revision,
            ordinal: cursor.ordinal,
        },
        (None, None) => IndexKey::ExtensionOrder {
            branch: branch.to_string(),
            revision: cursor.revision,
            ordinal: cursor.ordinal,
        },
        (None, Some(_)) => unreachable!("index_key is validated with kind"),
    }
}

fn extension_position(key: &IndexKey) -> PactResult<(u64, u64)> {
    match key {
        IndexKey::ExtensionOrder {
            revision, ordinal, ..
        }
        | IndexKey::ExtensionKind {
            revision, ordinal, ..
        }
        | IndexKey::ExtensionKey {
            revision, ordinal, ..
        } => Ok((*revision, *ordinal)),
        _ => Err(PactError::corrupt(
            "Pact extension range returned another index namespace",
        )),
    }
}

pub(crate) fn record(
    snapshot: &Snapshot,
    branch: &str,
    record_id: &str,
) -> PactResult<PactExtensionRow> {
    record_at(snapshot, branch, record_id, None)
}

pub(crate) fn record_at(
    snapshot: &Snapshot,
    branch: &str,
    record_id: &str,
    at_revision: Option<u64>,
) -> PactResult<PactExtensionRow> {
    let branch_head = snapshot
        .branch_head(branch)?
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch}")))?;
    if at_revision.is_some_and(|revision| revision > branch_head) {
        return Err(PactError::invalid(format!(
            "requested Pact extension revision exceeds branch head {branch_head}"
        )));
    }
    let key = IndexKey::ExtensionRecord {
        branch: branch.to_string(),
        record_hash: IndexKey::extension_key_hash(record_id),
    };
    let value = snapshot
        .index_value(&key)?
        .ok_or_else(|| PactError::not_found(format!("extension record not found: {record_id}")))?;
    let indexed = serde_json::from_slice::<IndexedExtensionValue>(&value).map_err(|error| {
        PactError::corrupt(format!(
            "invalid Pact extension record index value: {error}"
        ))
    })?;
    if indexed.branch_id != branch || indexed.record.record_id != record_id {
        return Err(PactError::corrupt(
            "Pact extension record hash collision or mismatched index value",
        ));
    }
    let row = PactExtensionRow {
        branch_id: indexed.branch_id,
        revision: indexed.revision,
        transaction_id: indexed.transaction_id,
        ordinal: indexed.ordinal,
        record: indexed.record,
    };
    if at_revision.is_some_and(|revision| row.revision > revision) {
        return Err(PactError::not_found(format!(
            "extension record not visible at requested revision: {record_id}"
        )));
    }
    Ok(row)
}
