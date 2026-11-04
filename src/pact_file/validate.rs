use std::fs;
use std::path::Path;

use super::backend::{IoCounters, ValidationReport};
use super::codec::{decode_content_body_record, decode_record_generation_at, decode_transaction};
use super::index::{HeadMetadata, IndexKey, read_index_value};
use super::root_slot::select_root_slot;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

pub(crate) fn fast_validate(path: &Path) -> PactResult<ValidationReport> {
    let file = fs::read(path)?;
    Ok(report(file.len(), || {
        let selected = select_root_slot(&file)?;
        let branch = selected.marker_branch(&file)?;
        if branch_head_value(&file, selected.slot.index_root_address, &branch)?.is_none() {
            return Err(PactError::corrupt(
                "Pact index does not contain the selected branch head",
            ));
        }
        Ok((2, 0))
    }))
}

pub(crate) fn full_validate(path: &Path) -> PactResult<ValidationReport> {
    let file = fs::read(path)?;
    Ok(report(file.len(), || validate_full_file(&file)))
}

fn validate_full_file(file: &[u8]) -> PactResult<(u64, u64)> {
    let selected = select_root_slot(file)?;
    let latest = decode_transaction(
        file,
        selected.marker.transaction_address,
        selected.slot.generation,
    )?;
    let branch = &latest.transaction.branch_id;
    let head = branch_head_value(file, selected.slot.index_root_address, branch)?
        .ok_or_else(|| PactError::corrupt("Pact index is missing branch head"))?;
    if head != latest.transaction.trace.to_rev {
        return Err(PactError::corrupt(
            "Pact branch-head index disagrees with the selected transaction",
        ));
    }

    let mut projected = serde_json::json!({
        "id": latest.transaction.public_trace.root_id,
        "componentType": "root",
        "children": []
    });
    let mut expected_from = 0;
    let mut records_decoded = 0_u64;
    let mut operations_replayed = 0_u64;
    for revision in 1..=head {
        let address = required_u64_index(
            file,
            selected.slot.index_root_address,
            &IndexKey::Revision {
                branch: branch.clone(),
                revision,
            },
            "revision address",
        )?;
        let generation = decode_record_generation_at(file, address)?;
        let decoded = decode_transaction(file, address, generation)?;
        let checkpoint = decoded.checkpoint;
        let transaction = decoded.transaction;
        if transaction.branch_id != *branch
            || transaction.trace.from_rev != expected_from
            || transaction.trace.to_rev != revision
        {
            return Err(PactError::corrupt(
                "Pact revision transaction is not contiguous on its indexed branch",
            ));
        }
        for content in &transaction.new_content {
            let payload = content_payload(content)?;
            let expected_hash = blake3::hash(payload);
            let indexed_hash = read_index_value(
                file,
                selected.slot.index_root_address,
                &IndexKey::ContentId {
                    id: content.id.clone(),
                },
            )?
            .ok_or_else(|| PactError::corrupt("Pact content id is missing from the index"))?;
            if indexed_hash.get(..32) != Some(expected_hash.as_bytes()) {
                return Err(PactError::corrupt(
                    "Pact content id index disagrees with its payload",
                ));
            }
            let body_address = required_u64_index(
                file,
                selected.slot.index_root_address,
                &IndexKey::ContentHash {
                    hash: *expected_hash.as_bytes(),
                },
                "content body address",
            )?;
            let stored_body = decode_content_body_record(file, body_address)?;
            if stored_body.as_slice() != payload {
                return Err(PactError::corrupt(
                    "Pact content hash index disagrees with its stored body",
                ));
            }
        }
        operations_replayed = operations_replayed
            .checked_add(transaction.trace.ops.len() as u64)
            .ok_or_else(|| PactError::corrupt("Pact replay operation count overflow"))?;
        projected = transaction.trace.replay_owned(projected)?;
        if let Some(checkpoint) = checkpoint
            && checkpoint.tree != projected
        {
            return Err(PactError::corrupt(
                "Pact semantic checkpoint disagrees with canonical replay",
            ));
        }
        expected_from = revision;
        records_decoded += 1;
    }
    Ok((records_decoded + 1, operations_replayed))
}

fn required_u64_index(file: &[u8], root: u64, key: &IndexKey, label: &str) -> PactResult<u64> {
    let bytes = read_index_value(file, root, key)?
        .ok_or_else(|| PactError::corrupt(format!("Pact index is missing {label}")))?;
    let raw: [u8; 8] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| PactError::corrupt(format!("Pact {label} has invalid width")))?;
    Ok(u64::from_le_bytes(raw))
}

fn branch_head_value(file: &[u8], root: u64, branch: &str) -> PactResult<Option<u64>> {
    if let Some(metadata) = read_index_value(file, root, &IndexKey::Metadata)? {
        return Ok(HeadMetadata::decode(&metadata)?
            .head_revisions
            .get(branch)
            .copied());
    }
    read_index_value(
        file,
        root,
        &IndexKey::BranchHead {
            branch: branch.to_string(),
        },
    )?
    .map(|bytes| {
        let raw: [u8; 8] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| PactError::corrupt("Pact branch head has invalid width"))?;
        Ok(u64::from_le_bytes(raw))
    })
    .transpose()
}

fn content_payload(content: &crate::store::ContentRecord) -> PactResult<&[u8]> {
    if let Some(bytes) = &content.storage.inline_bytes {
        return Ok(bytes);
    }
    if let Some(text) = &content.storage.inline {
        return Ok(text.as_bytes());
    }
    Err(PactError::corrupt(
        "candidate Pact content record has no inline payload",
    ))
}

fn report(
    bytes_read: usize,
    validate: impl FnOnce() -> PactResult<(u64, u64)>,
) -> ValidationReport {
    match validate() {
        Ok((records_decoded, operations_replayed)) => ValidationReport {
            ok: true,
            errors: Vec::new(),
            counters: IoCounters {
                bytes_read: bytes_read as u64,
                pages_read: 1,
                records_decoded,
                operations_replayed,
                ..IoCounters::default()
            },
        },
        Err(error) => ValidationReport {
            ok: false,
            errors: vec![error.to_string()],
            counters: IoCounters {
                bytes_read: bytes_read as u64,
                ..IoCounters::default()
            },
        },
    }
}

trait SelectedBranch {
    fn marker_branch(&self, file: &[u8]) -> PactResult<String>;
}

impl SelectedBranch for super::root_slot::RootSlotSelection {
    fn marker_branch(&self, file: &[u8]) -> PactResult<String> {
        Ok(
            decode_transaction(file, self.marker.transaction_address, self.slot.generation)?
                .transaction
                .branch_id,
        )
    }
}
