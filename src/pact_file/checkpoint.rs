use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{PactError, PactResult};
use crate::tree::ComponentTree;

#[cfg(test)]
mod tests;

const CHECKPOINT_MAGIC: &[u8; 8] = b"PCKPTV2\0";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SemanticCheckpoint {
    pub(crate) branch_id: String,
    pub(crate) revision: u64,
    pub(crate) tree: Value,
    pub(crate) first_seen: BTreeMap<String, u64>,
    pub(crate) lifecycle_seen: BTreeMap<String, u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CheckpointPolicy {
    interval: u64,
}

impl CheckpointPolicy {
    pub(crate) fn new(interval: u64) -> PactResult<Self> {
        if interval == 0 {
            return Err(PactError::invalid(
                "Pact checkpoint interval must be positive",
            ));
        }
        Ok(Self { interval })
    }

    pub(crate) fn capture(
        &self,
        branch_id: &str,
        revision: u64,
        tree: &ComponentTree,
        first_seen: &BTreeMap<String, u64>,
        lifecycle_seen: &BTreeMap<String, u64>,
    ) -> PactResult<Option<Vec<u8>>> {
        if !revision.is_multiple_of(self.interval) {
            return Ok(None);
        }
        let checkpoint = SemanticCheckpoint {
            branch_id: branch_id.to_string(),
            revision,
            tree: tree.project_value(),
            first_seen: first_seen.clone(),
            lifecycle_seen: lifecycle_seen.clone(),
        };
        Ok(Some(encode_checkpoint(&checkpoint)?))
    }
}

pub(crate) fn encode_checkpoint(checkpoint: &SemanticCheckpoint) -> PactResult<Vec<u8>> {
    let payload = serde_json::to_vec(checkpoint)
        .map_err(|error| PactError::codec(format!("encode Pact checkpoint: {error}")))?;
    let len = u64::try_from(payload.len())
        .map_err(|_| PactError::codec("Pact checkpoint length overflow"))?;
    let mut bytes = Vec::with_capacity(48 + payload.len());
    bytes.extend_from_slice(CHECKPOINT_MAGIC);
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(blake3::hash(&payload).as_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

pub(crate) fn decode_checkpoint(bytes: &[u8]) -> PactResult<SemanticCheckpoint> {
    if bytes.len() < 48 || &bytes[..8] != CHECKPOINT_MAGIC {
        return Err(PactError::corrupt("invalid Pact checkpoint header"));
    }
    let mut raw_len = [0_u8; 8];
    raw_len.copy_from_slice(&bytes[8..16]);
    let len = usize::try_from(u64::from_le_bytes(raw_len))
        .map_err(|_| PactError::corrupt("Pact checkpoint length exceeds platform limits"))?;
    let payload = bytes
        .get(48..)
        .filter(|payload| payload.len() == len)
        .ok_or_else(|| PactError::corrupt("Pact checkpoint payload length mismatch"))?;
    if blake3::hash(payload).as_bytes() != &bytes[16..48] {
        return Err(PactError::corrupt("Pact checkpoint checksum mismatch"));
    }
    serde_json::from_slice(payload)
        .map_err(|error| PactError::corrupt(format!("invalid Pact checkpoint: {error}")))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CheckpointReplay {
    pub(crate) tree: Value,
    pub(crate) operations_replayed: u64,
}

pub(crate) fn replay_revision(
    file: &[u8],
    index_root: u64,
    branch: &str,
    revision: u64,
    interval: u64,
) -> PactResult<CheckpointReplay> {
    if interval == 0 {
        return Err(PactError::invalid(
            "Pact checkpoint replay interval must be positive",
        ));
    }
    let checkpoint_revision = revision - (revision % interval);
    let mut tree;
    let first_revision;
    if checkpoint_revision == 0 {
        let first = indexed_transaction(file, index_root, branch, 1)?;
        tree = serde_json::json!({
            "id": first.transaction.public_trace.root_id,
            "componentType": "root",
            "children": []
        });
        first_revision = 1;
    } else {
        let checkpoint_transaction =
            indexed_transaction(file, index_root, branch, checkpoint_revision)?;
        let checkpoint = checkpoint_transaction.checkpoint.ok_or_else(|| {
            PactError::corrupt(format!(
                "Pact revision {checkpoint_revision} is missing its scheduled checkpoint"
            ))
        })?;
        tree = checkpoint.tree;
        first_revision = checkpoint_revision.saturating_add(1);
    }

    let mut operations_replayed = 0_u64;
    for current in first_revision..=revision {
        let decoded = indexed_transaction(file, index_root, branch, current)?;
        operations_replayed = operations_replayed
            .checked_add(decoded.transaction.trace.ops.len() as u64)
            .ok_or_else(|| PactError::corrupt("Pact checkpoint replay count overflow"))?;
        tree = decoded.transaction.trace.replay_owned(tree)?;
    }
    Ok(CheckpointReplay {
        tree,
        operations_replayed,
    })
}

fn indexed_transaction(
    file: &[u8],
    index_root: u64,
    branch: &str,
    revision: u64,
) -> PactResult<super::codec::DecodedTransaction> {
    let address = super::index::read_index_value(
        file,
        index_root,
        &super::index::IndexKey::Revision {
            branch: branch.to_string(),
            revision,
        },
    )?
    .ok_or_else(|| PactError::corrupt(format!("Pact revision {revision} is not indexed")))?;
    let raw: [u8; 8] = address
        .as_slice()
        .try_into()
        .map_err(|_| PactError::corrupt("Pact revision address has invalid width"))?;
    let address = u64::from_le_bytes(raw);
    let generation = super::codec::decode_record_generation_at(file, address)?;
    super::codec::decode_transaction(file, address, generation)
}
