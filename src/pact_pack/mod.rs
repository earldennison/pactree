use crate::error::{PactError, PactResult};
use crate::transaction_trace::{self, TransactionTrace};
use serde_json::{Value, json};

const REMOVED_MAGIC: &[u8; 8] = b"PACTPKV1";
const REMOVED_MESSAGE: &str = "pre-v0.1 packed Pact files are unsupported";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackProfile {
    HotWrite,
    ColdRepack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictionaryPolicy {
    Auto,
    Force,
    Disabled,
}

pub struct PackedV1 {
    pub root_id: String,
    pub head_rev: u64,
    dict_id: [u8; 32],
}

impl PackedV1 {
    pub fn read(_bytes: &[u8]) -> PactResult<Self> {
        Err(PactError::unsupported(REMOVED_MESSAGE))
    }

    pub fn project_head(&self) -> PactResult<Value> {
        Err(PactError::unsupported(REMOVED_MESSAGE))
    }

    pub fn project_revision(&self, _revision: u64) -> PactResult<Value> {
        Err(PactError::unsupported(REMOVED_MESSAGE))
    }

    pub fn ledgers(&self) -> PactResult<Vec<TransactionTrace>> {
        Err(PactError::unsupported(REMOVED_MESSAGE))
    }

    pub fn cached_head(&self) -> Option<Value> {
        None
    }

    pub fn dict_id(&self) -> &[u8; 32] {
        &self.dict_id
    }

    pub fn has_dictionary(&self) -> bool {
        false
    }

    pub fn has_head_cache(&self) -> bool {
        false
    }

    pub fn pack_count(&self) -> usize {
        0
    }
}

pub fn is_packed_v1(bytes: &[u8]) -> bool {
    bytes.starts_with(REMOVED_MAGIC)
}

pub fn write_packed_v1(_root_id: &str, _ledgers: &[TransactionTrace]) -> PactResult<Vec<u8>> {
    Err(PactError::unsupported(REMOVED_MESSAGE))
}

pub fn write_packed_v1_with_policy(
    _root_id: &str,
    _ledgers: &[TransactionTrace],
    _policy: DictionaryPolicy,
) -> PactResult<Vec<u8>> {
    Err(PactError::unsupported(REMOVED_MESSAGE))
}

pub fn write_packed_v1_cold_repack(
    _root_id: &str,
    _ledgers: &[TransactionTrace],
) -> PactResult<Vec<u8>> {
    Err(PactError::unsupported(REMOVED_MESSAGE))
}

pub fn write_packed_v1_with_head_cache(
    _root_id: &str,
    _ledgers: &[TransactionTrace],
    _policy: DictionaryPolicy,
) -> PactResult<Vec<u8>> {
    Err(PactError::unsupported(REMOVED_MESSAGE))
}

pub fn ledgers_from_revision_trees(root_id: &str, trees: &[Value]) -> Vec<TransactionTrace> {
    let mut ledgers = Vec::with_capacity(trees.len());
    let mut previous = json!({ "id": root_id, "children": [] });
    for (index, tree) in trees.iter().enumerate() {
        let to_rev = index as u64;
        let from_rev = index.saturating_sub(1) as u64;
        let normalized = transaction_trace::normalize_for_transaction_trace(tree);
        ledgers.push(TransactionTrace::diff(
            from_rev,
            to_rev,
            &previous,
            &normalized,
        ));
        previous = normalized;
    }
    ledgers
}
