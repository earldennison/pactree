use super::codec::{CommitMarker, decode_commit_marker_at, decode_transaction};
use super::envelope::RootSlot;
use super::layout::{
    FILE_IDENTITY_BYTES, FILE_IDENTITY_START, FILE_MAGIC, FILE_VERSION, HEADER_BYTES,
    ROOT_SLOT_BYTES, ROOT_SLOT_COUNT, ROOT_SLOT_DIGEST_BYTES, ROOT_SLOT_DIGEST_START,
    ROOT_SLOT_PAYLOAD_BYTES, ROOT_SLOT_START,
};
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RootSlotSelection {
    pub(crate) slot_index: usize,
    pub(crate) slot: RootSlot,
    pub(crate) marker: CommitMarker,
}

pub(crate) fn encode_empty_header(file_identity: [u8; FILE_IDENTITY_BYTES]) -> [u8; HEADER_BYTES] {
    let mut header = [0_u8; HEADER_BYTES];
    header[..FILE_MAGIC.len()].copy_from_slice(FILE_MAGIC);
    header[8..10].copy_from_slice(&FILE_VERSION.to_le_bytes());
    header[10..12].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
    header[FILE_IDENTITY_START..FILE_IDENTITY_START + FILE_IDENTITY_BYTES]
        .copy_from_slice(&file_identity);
    header
}

pub(crate) fn encode_root_slot(slot: &RootSlot) -> [u8; ROOT_SLOT_BYTES] {
    let mut bytes = [0_u8; ROOT_SLOT_BYTES];
    bytes[..8].copy_from_slice(&slot.generation.to_le_bytes());
    bytes[8..16].copy_from_slice(&slot.committed_transaction_address.to_le_bytes());
    bytes[16..24].copy_from_slice(&slot.index_root_address.to_le_bytes());
    bytes[24..32].copy_from_slice(&slot.file_length_watermark.to_le_bytes());
    bytes[32..40].copy_from_slice(&slot.segment_directory_address.to_le_bytes());
    bytes[40..48].copy_from_slice(&slot.segment_directory_len.to_le_bytes());
    bytes[48..56].copy_from_slice(&slot.allocator_address.to_le_bytes());
    bytes[56..64].copy_from_slice(&slot.allocator_len.to_le_bytes());
    bytes[64..80].copy_from_slice(&slot.segment_directory_digest);
    bytes[80..96].copy_from_slice(&slot.allocator_digest);
    let digest = blake3::hash(&bytes[..ROOT_SLOT_PAYLOAD_BYTES]);
    bytes[ROOT_SLOT_DIGEST_START..ROOT_SLOT_DIGEST_START + ROOT_SLOT_DIGEST_BYTES]
        .copy_from_slice(digest.as_bytes());
    bytes
}

pub(crate) fn select_root_slot(file: &[u8]) -> PactResult<RootSlotSelection> {
    let mut selections = Vec::new();
    for (slot_index, slot) in root_slot_candidates(file)? {
        if slot.file_length_watermark > file.len() as u64 {
            continue;
        }
        let Ok(marker) =
            decode_commit_marker_at(file, slot.committed_transaction_address, slot.generation)
        else {
            continue;
        };
        if marker.index_root_address != slot.index_root_address
            || marker.file_length_watermark != slot.file_length_watermark
            || marker.segment_directory_address != slot.segment_directory_address
            || marker.segment_directory_len != slot.segment_directory_len
            || marker.allocator_address != slot.allocator_address
            || marker.allocator_len != slot.allocator_len
            || marker.segment_directory_digest != slot.segment_directory_digest
            || marker.allocator_digest != slot.allocator_digest
        {
            continue;
        }
        let Ok(decoded) = decode_transaction(file, marker.transaction_address, slot.generation)
        else {
            continue;
        };
        if decoded.marker_address != slot.committed_transaction_address || decoded.marker != marker
        {
            continue;
        }
        selections.push(RootSlotSelection {
            slot_index,
            slot,
            marker,
        });
    }
    selections
        .into_iter()
        .max_by_key(|selection| selection.slot.generation)
        .ok_or_else(|| PactError::corrupt("Pact file envelope has no valid committed root slot"))
}

pub(crate) fn root_slot_candidates(header: &[u8]) -> PactResult<Vec<(usize, RootSlot)>> {
    super::envelope::classify(header)?;
    let header = header
        .get(..HEADER_BYTES)
        .ok_or_else(|| PactError::corrupt("truncated Pact file envelope"))?;
    if &header[..FILE_MAGIC.len()] != FILE_MAGIC {
        return Err(PactError::corrupt("unknown Pact file magic"));
    }
    Ok((0..ROOT_SLOT_COUNT)
        .filter_map(|slot_index| {
            decode_root_slot(header, slot_index).map(|slot| (slot_index, slot))
        })
        .collect())
}

pub(crate) fn slot_index_for_next(selection: Option<&RootSlotSelection>) -> usize {
    selection.map_or(0, |selection| (selection.slot_index + 1) % ROOT_SLOT_COUNT)
}

fn decode_root_slot(header: &[u8], slot_index: usize) -> Option<RootSlot> {
    let start = ROOT_SLOT_START.checked_add(slot_index.checked_mul(ROOT_SLOT_BYTES)?)?;
    let bytes = header.get(start..start + ROOT_SLOT_BYTES)?;
    let payload = &bytes[..ROOT_SLOT_PAYLOAD_BYTES];
    if payload.iter().all(|byte| *byte == 0)
        || blake3::hash(payload).as_bytes()
            != &bytes[ROOT_SLOT_DIGEST_START..ROOT_SLOT_DIGEST_START + ROOT_SLOT_DIGEST_BYTES]
    {
        return None;
    }
    let slot = RootSlot {
        generation: read_u64(payload, 0),
        committed_transaction_address: read_u64(payload, 8),
        index_root_address: read_u64(payload, 16),
        file_length_watermark: read_u64(payload, 24),
        segment_directory_address: read_u64(payload, 32),
        segment_directory_len: read_u64(payload, 40),
        allocator_address: read_u64(payload, 48),
        allocator_len: read_u64(payload, 56),
        segment_directory_digest: payload[64..80].try_into().ok()?,
        allocator_digest: payload[80..96].try_into().ok()?,
    };
    (slot.generation > 0
        && slot.committed_transaction_address >= HEADER_BYTES as u64
        && slot.index_root_address >= HEADER_BYTES as u64
        && slot.file_length_watermark >= HEADER_BYTES as u64
        && slot.segment_directory_len > 0
        && slot.allocator_len > 0
        && slot
            .segment_directory_address
            .checked_add(slot.segment_directory_len)
            .is_some_and(|end| end <= slot.file_length_watermark)
        && slot
            .allocator_address
            .checked_add(slot.allocator_len)
            .is_some_and(|end| end <= slot.file_length_watermark))
    .then_some(slot)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}
