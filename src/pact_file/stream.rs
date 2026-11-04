use serde_json::Value;

use super::index::IndexKey;
use super::snapshot::Snapshot;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamDirection {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StreamRow {
    pub(crate) seq: u64,
    pub(crate) revision: u64,
    pub(crate) component_id: String,
    pub(crate) payload: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StreamWindow {
    pub(crate) rows: Vec<StreamRow>,
    pub(crate) has_before: bool,
    pub(crate) has_after: bool,
}

pub(crate) fn current_window(
    snapshot: &Snapshot,
    branch: &str,
    anchor_revision: Option<u64>,
    direction: StreamDirection,
    limit: usize,
) -> PactResult<StreamWindow> {
    let limit = limit.clamp(1, 500);
    let (start_order, end_order, reverse) = match (direction, anchor_revision) {
        (StreamDirection::After, Some(anchor)) => (
            anchor.saturating_add(1).saturating_mul(1 << 16),
            u64::MAX,
            false,
        ),
        (StreamDirection::After, None) => (0, u64::MAX, false),
        (StreamDirection::Before, Some(anchor)) => (0, anchor.saturating_mul(1 << 16), true),
        (StreamDirection::Before, None) => (0, u64::MAX, true),
    };
    let mut indexed = snapshot.index_range(
        &IndexKey::MessageOrder {
            branch: branch.to_string(),
            order: start_order,
        },
        &IndexKey::MessageOrder {
            branch: branch.to_string(),
            order: end_order,
        },
        limit + 1,
        reverse,
    )?;
    snapshot.record_rows_read(indexed.len());
    let has_more = indexed.len() > limit;
    indexed.truncate(limit);
    if reverse {
        indexed.reverse();
    }
    let mut rows = Vec::with_capacity(indexed.len());
    for (key, value) in indexed {
        let IndexKey::MessageOrder { order, .. } = IndexKey::decode(&key)? else {
            return Err(PactError::corrupt(
                "Pact message range returned a non-message key",
            ));
        };
        let component_id = String::from_utf8(value)
            .map_err(|_| PactError::corrupt("Pact message component id is not UTF-8"))?;
        let component = snapshot.component(branch, &component_id)?.ok_or_else(|| {
            PactError::corrupt("Pact message-order index references a missing component")
        })?;
        rows.push(StreamRow {
            seq: order >> 16,
            revision: component.revision,
            component_id,
            payload: component.value,
        });
    }
    let (has_before, has_after) = match direction {
        StreamDirection::Before => (has_more, anchor_revision.is_some()),
        StreamDirection::After => (anchor_revision.is_some(), has_more),
    };
    Ok(StreamWindow {
        rows,
        has_before,
        has_after,
    })
}

pub(crate) fn activity_window(
    snapshot: &Snapshot,
    branch: &str,
    anchor_revision: Option<u64>,
    direction: StreamDirection,
    limit: usize,
) -> PactResult<StreamWindow> {
    let limit = limit.clamp(1, 500);
    let (start_revision, end_revision, reverse) = revision_bounds(anchor_revision, direction);
    let message_entries = snapshot.index_range(
        &IndexKey::MessageOrder {
            branch: branch.to_string(),
            order: start_revision.saturating_mul(1 << 16),
        },
        &IndexKey::MessageOrder {
            branch: branch.to_string(),
            order: end_revision.saturating_mul(1 << 16),
        },
        limit + 1,
        reverse,
    )?;
    let revision_entries = snapshot.index_range(
        &IndexKey::Revision {
            branch: branch.to_string(),
            revision: start_revision,
        },
        &IndexKey::Revision {
            branch: branch.to_string(),
            revision: end_revision,
        },
        limit + 1,
        reverse,
    )?;
    let mut candidates = Vec::with_capacity(message_entries.len() + revision_entries.len());
    for (key, value) in message_entries {
        let IndexKey::MessageOrder { order, .. } = IndexKey::decode(&key)? else {
            return Err(PactError::corrupt("invalid Pact activity message key"));
        };
        candidates.push(ActivityCandidate {
            sort: (order >> 16, 0, order),
            kind: ActivityKind::Message(value),
        });
    }
    for (key, value) in revision_entries {
        let IndexKey::Revision { revision, .. } = IndexKey::decode(&key)? else {
            return Err(PactError::corrupt("invalid Pact activity revision key"));
        };
        candidates.push(ActivityCandidate {
            sort: (revision, 1, 0),
            kind: ActivityKind::Trace(decode_address(&value)?),
        });
    }
    candidates.sort_by_key(|candidate| candidate.sort);
    if reverse {
        candidates.reverse();
    }
    let has_more = candidates.len() > limit;
    candidates.truncate(limit + 1);
    snapshot.record_rows_read(candidates.len());
    candidates.truncate(limit);
    if reverse {
        candidates.reverse();
    }
    let mut rows = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match candidate.kind {
            ActivityKind::Message(component_id) => {
                let component_id = String::from_utf8(component_id)
                    .map_err(|_| PactError::corrupt("Pact message component id is not UTF-8"))?;
                let component = snapshot.component(branch, &component_id)?.ok_or_else(|| {
                    PactError::corrupt("Pact activity message references a missing component")
                })?;
                rows.push(StreamRow {
                    seq: candidate.sort.2 >> 16,
                    revision: component.revision,
                    component_id,
                    payload: component.value,
                });
            }
            ActivityKind::Trace(address) => {
                let transaction = snapshot.transaction_at(address)?;
                let changed = transaction
                    .trace
                    .ops
                    .iter()
                    .map(|op| op.component_id.clone())
                    .collect::<Vec<_>>();
                rows.push(StreamRow {
                    seq: transaction.trace.to_rev,
                    revision: transaction.trace.to_rev,
                    component_id: format!("trace:{}", transaction.transaction_id),
                    payload: serde_json::json!({
                        "kind": "trace",
                        "transaction_id": transaction.transaction_id,
                        "boundary": format!("{:?}", transaction.boundary),
                        "changed_component_ids": changed,
                    }),
                });
            }
        }
    }
    let (has_before, has_after) = match direction {
        StreamDirection::Before => (has_more, anchor_revision.is_some()),
        StreamDirection::After => (anchor_revision.is_some(), has_more),
    };
    Ok(StreamWindow {
        rows,
        has_before,
        has_after,
    })
}

struct ActivityCandidate {
    sort: (u64, u8, u64),
    kind: ActivityKind,
}

enum ActivityKind {
    Message(Vec<u8>),
    Trace(u64),
}

fn revision_bounds(anchor: Option<u64>, direction: StreamDirection) -> (u64, u64, bool) {
    match (direction, anchor) {
        (StreamDirection::After, Some(anchor)) => (anchor.saturating_add(1), u64::MAX, false),
        (StreamDirection::After, None) => (0, u64::MAX, false),
        (StreamDirection::Before, Some(anchor)) => (0, anchor, true),
        (StreamDirection::Before, None) => (0, u64::MAX, true),
    }
}

fn decode_address(bytes: &[u8]) -> PactResult<u64> {
    let raw: [u8; 8] = bytes
        .try_into()
        .map_err(|_| PactError::corrupt("Pact revision address has invalid width"))?;
    Ok(u64::from_le_bytes(raw))
}
