use std::collections::BTreeMap;

use crate::error::{PactError, PactResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RecordLocator {
    pub(crate) segment_id: u64,
    pub(crate) ordinal: u32,
}

impl RecordLocator {
    pub(crate) const ENCODED_BYTES: usize = 12;

    pub(crate) fn encode(self) -> [u8; Self::ENCODED_BYTES] {
        let mut bytes = [0_u8; Self::ENCODED_BYTES];
        bytes[..8].copy_from_slice(&self.segment_id.to_le_bytes());
        bytes[8..].copy_from_slice(&self.ordinal.to_le_bytes());
        bytes
    }

    pub(crate) fn decode(bytes: &[u8]) -> PactResult<Self> {
        if bytes.len() != Self::ENCODED_BYTES {
            return Err(PactError::corrupt("Pact record locator has invalid width"));
        }
        Ok(Self {
            segment_id: u64::from_le_bytes(bytes[..8].try_into().map_err(|_| {
                PactError::corrupt("Pact record locator segment has invalid width")
            })?),
            ordinal: u32::from_le_bytes(bytes[8..].try_into().map_err(|_| {
                PactError::corrupt("Pact record locator ordinal has invalid width")
            })?),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SegmentExtent {
    pub(crate) segment_id: u64,
    pub(crate) offset: u64,
    pub(crate) len: u64,
    pub(crate) first_generation: u64,
    pub(crate) last_generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SegmentDirectory {
    pub(crate) generation: u64,
    pub(crate) segments: Vec<SegmentExtent>,
    pub(crate) transactions: Vec<RecordLocator>,
    pub(crate) branch_heads: BTreeMap<String, u64>,
    pub(crate) head_checkpoint: Option<RecordLocator>,
}

impl SegmentDirectory {
    pub(crate) fn open(bytes: &[u8]) -> PactResult<Self> {
        let selected = super::root_slot::select_root_slot(bytes)?;
        let addresses = super::index::generation_addresses(
            bytes,
            selected.slot.index_root_address,
            selected.slot.generation,
        )?;
        let metadata = super::index::read_index_value(
            bytes,
            selected.slot.index_root_address,
            &super::index::IndexKey::Metadata,
        )?
        .map(|value| super::index::HeadMetadata::decode(&value))
        .transpose()?
        .ok_or_else(|| PactError::corrupt("Pact segment directory has no head metadata"))?;
        let mut segments = Vec::with_capacity(addresses.len());
        let mut transactions = Vec::new();
        for (ordinal, address) in addresses.into_iter().enumerate() {
            let generation = super::codec::decode_record_generation_at(bytes, address)?;
            let decoded = super::codec::decode_transaction(bytes, address, generation)?;
            segments.push(SegmentExtent {
                segment_id: generation,
                offset: address,
                len: decoded.marker.file_length_watermark.saturating_sub(address),
                first_generation: generation,
                last_generation: generation,
            });
            if !decoded.transaction.is_genesis() {
                transactions.push(RecordLocator {
                    segment_id: generation,
                    ordinal: u32::try_from(ordinal).map_err(|_| {
                        PactError::corrupt("Pact segment transaction ordinal exceeds u32")
                    })?,
                });
            }
        }
        Ok(Self {
            generation: selected.slot.generation,
            segments,
            transactions,
            branch_heads: metadata.head_revisions,
            head_checkpoint: None,
        })
    }
}

#[cfg(test)]
mod tests;
