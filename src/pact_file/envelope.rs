use super::layout::{
    FILE_IDENTITY_BYTES, FILE_IDENTITY_START, FILE_MAGIC, FILE_VERSION, HEADER_BYTES,
    MAX_FILE_LENGTH_WATERMARK, ROOT_SLOT_BYTES, ROOT_SLOT_COUNT, ROOT_SLOT_DIGEST_BYTES,
    ROOT_SLOT_DIGEST_START, ROOT_SLOT_PAYLOAD_BYTES, ROOT_SLOT_START,
};
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvelopeKind {
    V0_1,
    UnsupportedPreV0_1,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RootSlot {
    pub(crate) generation: u64,
    pub(crate) committed_transaction_address: u64,
    pub(crate) index_root_address: u64,
    pub(crate) file_length_watermark: u64,
    pub(crate) segment_directory_address: u64,
    pub(crate) segment_directory_len: u64,
    pub(crate) allocator_address: u64,
    pub(crate) allocator_len: u64,
    pub(crate) segment_directory_digest: [u8; 16],
    pub(crate) allocator_digest: [u8; 16],
}

impl RootSlot {
    pub(crate) fn from_encoded(
        generation: u64,
        encoded: &super::codec::EncodedTransaction,
    ) -> Self {
        Self {
            generation,
            committed_transaction_address: encoded.commit_marker_address,
            index_root_address: encoded.index_root_address,
            file_length_watermark: encoded.committed_len,
            segment_directory_address: encoded.segment_directory_address,
            segment_directory_len: encoded.segment_directory_len,
            allocator_address: encoded.allocator_address,
            allocator_len: encoded.allocator_len,
            segment_directory_digest: encoded.segment_directory_digest,
            allocator_digest: encoded.allocator_digest,
        }
    }
}

pub(crate) fn encode_header(
    file_identity: [u8; FILE_IDENTITY_BYTES],
    root_slots: &[RootSlot],
) -> PactResult<[u8; HEADER_BYTES]> {
    if file_identity.iter().all(|byte| *byte == 0) {
        return Err(PactError::invalid("Pact file identity cannot be empty"));
    }
    if root_slots.is_empty() || root_slots.len() > ROOT_SLOT_COUNT {
        return Err(PactError::invalid(format!(
            "Pact file envelope requires 1..={ROOT_SLOT_COUNT} root slots"
        )));
    }
    let mut header = [0_u8; HEADER_BYTES];
    header[..FILE_MAGIC.len()].copy_from_slice(FILE_MAGIC);
    header[8..10].copy_from_slice(&FILE_VERSION.to_le_bytes());
    header[10..12].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
    header[FILE_IDENTITY_START..FILE_IDENTITY_START + FILE_IDENTITY_BYTES]
        .copy_from_slice(&file_identity);
    for (slot_index, slot) in root_slots.iter().enumerate() {
        let start = ROOT_SLOT_START + slot_index * ROOT_SLOT_BYTES;
        header[start..start + 8].copy_from_slice(&slot.generation.to_le_bytes());
        header[start + 8..start + 16]
            .copy_from_slice(&slot.committed_transaction_address.to_le_bytes());
        header[start + 16..start + 24].copy_from_slice(&slot.index_root_address.to_le_bytes());
        header[start + 24..start + 32].copy_from_slice(&slot.file_length_watermark.to_le_bytes());
        header[start + 32..start + 40]
            .copy_from_slice(&slot.segment_directory_address.to_le_bytes());
        header[start + 40..start + 48].copy_from_slice(&slot.segment_directory_len.to_le_bytes());
        header[start + 48..start + 56].copy_from_slice(&slot.allocator_address.to_le_bytes());
        header[start + 56..start + 64].copy_from_slice(&slot.allocator_len.to_le_bytes());
        header[start + 64..start + 80].copy_from_slice(&slot.segment_directory_digest);
        header[start + 80..start + 96].copy_from_slice(&slot.allocator_digest);
        let digest = blake3::hash(&header[start..start + ROOT_SLOT_PAYLOAD_BYTES]);
        header[start + ROOT_SLOT_DIGEST_START
            ..start + ROOT_SLOT_DIGEST_START + ROOT_SLOT_DIGEST_BYTES]
            .copy_from_slice(digest.as_bytes());
        if !root_slot_is_valid(&header, slot_index) {
            return Err(PactError::invalid(format!(
                "invalid Pact file root slot at index {slot_index}"
            )));
        }
    }
    Ok(header)
}

pub(crate) fn classify(bytes: &[u8]) -> PactResult<EnvelopeKind> {
    let magic = bytes
        .get(..FILE_MAGIC.len())
        .ok_or_else(|| PactError::corrupt("truncated Pact file envelope"))?;
    if magic == b"PACTBIN1" {
        return Err(PactError::unsupported(
            "pre-v0.1 Pact files are unsupported",
        ));
    }
    if magic != FILE_MAGIC {
        return Err(PactError::corrupt("unknown Pact file magic"));
    }
    let header = bytes
        .get(..HEADER_BYTES)
        .ok_or_else(|| PactError::corrupt("truncated Pact file envelope"))?;
    let version = read_u16(header, 8)?;
    if version != FILE_VERSION {
        return Err(PactError::unsupported(format!(
            "unsupported Pact file version: {version}"
        )));
    }
    let header_bytes = usize::from(read_u16(header, 10)?);
    if header_bytes != HEADER_BYTES {
        return Err(PactError::corrupt(format!(
            "invalid Pact file header length: {header_bytes}"
        )));
    }
    if !(0..ROOT_SLOT_COUNT).any(|slot| root_slot_is_valid(header, slot)) {
        return Err(PactError::corrupt(
            "Pact file envelope has no valid root slot",
        ));
    }
    Ok(EnvelopeKind::V0_1)
}

pub(crate) fn new_file_identity() -> PactResult<[u8; FILE_IDENTITY_BYTES]> {
    let mut identity = [0_u8; FILE_IDENTITY_BYTES];
    getrandom::fill(&mut identity)
        .map_err(|error| PactError::storage(format!("generate Pact file identity: {error}")))?;
    if identity.iter().all(|byte| *byte == 0) {
        return Err(PactError::storage(
            "generated Pact file identity was unexpectedly empty",
        ));
    }
    Ok(identity)
}

pub(crate) fn file_identity(bytes: &[u8]) -> PactResult<[u8; FILE_IDENTITY_BYTES]> {
    classify(bytes)?;
    let header = bytes
        .get(..HEADER_BYTES)
        .ok_or_else(|| PactError::corrupt("truncated Pact file envelope"))?;
    let mut identity = [0_u8; FILE_IDENTITY_BYTES];
    identity
        .copy_from_slice(&header[FILE_IDENTITY_START..FILE_IDENTITY_START + FILE_IDENTITY_BYTES]);
    if identity.iter().any(|byte| *byte != 0) {
        return Ok(identity);
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pactbin2-candidate-file-identity-v1\0");
    hasher.update(&header[..FILE_IDENTITY_START]);
    hasher.update(&header[FILE_IDENTITY_START + FILE_IDENTITY_BYTES..]);
    Ok(*hasher.finalize().as_bytes())
}

fn root_slot_is_valid(header: &[u8], slot: usize) -> bool {
    let start = ROOT_SLOT_START + slot * ROOT_SLOT_BYTES;
    let Some(payload) = header.get(start..start + ROOT_SLOT_PAYLOAD_BYTES) else {
        return false;
    };
    if payload.iter().all(|byte| *byte == 0) {
        return false;
    }
    let Some(stored_digest) = header.get(
        start + ROOT_SLOT_DIGEST_START..start + ROOT_SLOT_DIGEST_START + ROOT_SLOT_DIGEST_BYTES,
    ) else {
        return false;
    };
    if blake3::hash(payload).as_bytes() != stored_digest {
        return false;
    }
    let generation = read_u64_at(payload, 0);
    let committed_transaction_address = read_u64_at(payload, 8);
    let index_root_address = read_u64_at(payload, 16);
    let index_batch_address = super::codec::index_page_batch_address(index_root_address);
    let file_length_watermark = read_u64_at(payload, 24);
    let segment_directory_address = read_u64_at(payload, 32);
    let segment_directory_len = read_u64_at(payload, 40);
    let allocator_address = read_u64_at(payload, 48);
    let allocator_len = read_u64_at(payload, 56);
    generation > 0
        && file_length_watermark >= HEADER_BYTES as u64
        && file_length_watermark <= MAX_FILE_LENGTH_WATERMARK
        && committed_transaction_address >= HEADER_BYTES as u64
        && committed_transaction_address <= file_length_watermark
        && index_batch_address >= HEADER_BYTES as u64
        && index_batch_address <= file_length_watermark
        && segment_directory_len > 0
        && allocator_len > 0
        && segment_directory_address >= HEADER_BYTES as u64
        && segment_directory_address
            .checked_add(segment_directory_len)
            .is_some_and(|end| end <= file_length_watermark)
        && allocator_address >= HEADER_BYTES as u64
        && allocator_address
            .checked_add(allocator_len)
            .is_some_and(|end| end <= file_length_watermark)
}

fn read_u16(bytes: &[u8], offset: usize) -> PactResult<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| PactError::corrupt("truncated Pact file envelope"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}
