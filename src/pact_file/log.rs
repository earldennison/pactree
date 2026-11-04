use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::backend::{CommitReceipt, IoCounterSet, IoCounters};
use super::codec::{
    ContentBodySource, DecodedTransaction, RECORD_HEADER_BYTES, component_payload_batch_record_len,
    content_body_record_len, decode_commit_marker_region, decode_content_body_record_region,
    decode_transaction_region, encode_transaction, encode_transaction_checkpointed,
    encode_transaction_checkpointed_cold, encode_transaction_checkpointed_cold_prepared,
    index_page_record_len,
};
use super::envelope::RootSlot;
use super::layout::{HEADER_BYTES, ROOT_SLOT_BYTES, ROOT_SLOT_START};
use super::root_slot::{
    RootSlotSelection, encode_empty_header, encode_root_slot, root_slot_candidates,
    slot_index_for_next,
};
use super::transaction::PactTransaction;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FaultPoint {
    DirectoryWritten,
    AllocatorWritten,
    RecordsWritten,
    DataSynced,
    RootSlotWritten,
    RootSynced,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CandidateSnapshot {
    pub(crate) generation: u64,
    pub(crate) transaction_address: u64,
    pub(crate) index_root_address: u64,
    pub(crate) committed_len: u64,
    pub(crate) transaction: PactTransaction,
    pub(crate) checkpoint: Option<super::checkpoint::SemanticCheckpoint>,
}

pub(crate) fn create(
    path: &Path,
    genesis: &PactTransaction,
    counters: &IoCounterSet,
    fault: Option<FaultPoint>,
) -> PactResult<CommitReceipt> {
    create_checkpointed(path, genesis, counters, fault, None)
}

pub(crate) fn encode_genesis(genesis: &PactTransaction) -> PactResult<Vec<u8>> {
    encode_genesis_mode(genesis)
}

pub(crate) fn encode_cold_genesis(genesis: PactTransaction) -> PactResult<Vec<u8>> {
    encode_cold_genesis_mode(genesis, None, super::envelope::new_file_identity()?)
}

pub(crate) fn encode_cold_genesis_prepared(
    genesis: PactTransaction,
    baseline_branches: &[super::index::BaselineBranchOps],
    text_changes: &[super::change_feed::IndexedTextChange],
    search_changes: &[super::search_change_feed::IndexedSearchChange],
) -> PactResult<Vec<u8>> {
    encode_cold_genesis_prepared_with_identity(
        genesis,
        baseline_branches,
        text_changes,
        search_changes,
        super::envelope::new_file_identity()?,
    )
}

pub(crate) fn encode_cold_genesis_prepared_with_identity(
    genesis: PactTransaction,
    baseline_branches: &[super::index::BaselineBranchOps],
    text_changes: &[super::change_feed::IndexedTextChange],
    search_changes: &[super::search_change_feed::IndexedSearchChange],
    file_identity: [u8; super::layout::FILE_IDENTITY_BYTES],
) -> PactResult<Vec<u8>> {
    encode_cold_genesis_mode(
        genesis,
        Some((baseline_branches, text_changes, search_changes)),
        file_identity,
    )
}

fn encode_cold_genesis_mode(
    genesis: PactTransaction,
    prepared: Option<(
        &[super::index::BaselineBranchOps],
        &[super::change_feed::IndexedTextChange],
        &[super::search_change_feed::IndexedSearchChange],
    )>,
    file_identity: [u8; super::layout::FILE_IDENTITY_BYTES],
) -> PactResult<Vec<u8>> {
    if !genesis.is_genesis() {
        return Err(PactError::invalid(
            "candidate Pact encoded genesis must snapshot revision 0",
        ));
    }
    genesis.validate()?;
    let mut bytes = encode_empty_header(file_identity).to_vec();
    let encoded = if let Some((baseline_branches, text_changes, search_changes)) = prepared {
        encode_transaction_checkpointed_cold_prepared(
            genesis,
            1,
            HEADER_BYTES as u64,
            baseline_branches,
            text_changes,
            search_changes,
        )?
    } else {
        encode_transaction_checkpointed_cold(genesis, 1, HEADER_BYTES as u64)?
    };
    bytes.extend_from_slice(&encoded.bytes);
    let slot = RootSlot::from_encoded(1, &encoded);
    bytes[ROOT_SLOT_START..ROOT_SLOT_START + ROOT_SLOT_BYTES]
        .copy_from_slice(&encode_root_slot(&slot));
    Ok(bytes)
}

fn encode_genesis_mode(genesis: &PactTransaction) -> PactResult<Vec<u8>> {
    if !genesis.is_genesis() {
        return Err(PactError::invalid(
            "candidate Pact encoded genesis must snapshot revision 0",
        ));
    }
    genesis.validate()?;
    let identity = super::envelope::new_file_identity()?;
    let mut bytes = encode_empty_header(identity).to_vec();
    let encoded = encode_transaction(genesis, 1, HEADER_BYTES as u64)?;
    bytes.extend_from_slice(&encoded.bytes);
    let slot = RootSlot::from_encoded(1, &encoded);
    let slot_bytes = encode_root_slot(&slot);
    let end = ROOT_SLOT_START + ROOT_SLOT_BYTES;
    bytes[ROOT_SLOT_START..end].copy_from_slice(&slot_bytes);
    Ok(bytes)
}

pub(crate) fn create_checkpointed(
    path: &Path,
    genesis: &PactTransaction,
    counters: &IoCounterSet,
    fault: Option<FaultPoint>,
    checkpoint: Option<&super::checkpoint::SemanticCheckpoint>,
) -> PactResult<CommitReceipt> {
    if !genesis.is_genesis() && (genesis.trace.from_rev != 0 || genesis.trace.to_rev != 1) {
        return Err(PactError::invalid(
            "candidate Pact genesis must snapshot revision 0 or commit revision 0 -> 1",
        ));
    }
    let identity = super::envelope::new_file_identity()?;
    let header = encode_empty_header(identity);
    let encoded = if checkpoint.is_some() {
        encode_transaction_checkpointed(genesis, 1, HEADER_BYTES as u64, None, None, checkpoint)?
    } else {
        encode_transaction(genesis, 1, HEADER_BYTES as u64)?
    };
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&header)?;
    write_encoded_records(&mut file, &encoded, fault)?;
    file.sync_data()?;
    inject(fault, FaultPoint::DataSynced)?;

    let slot = RootSlot::from_encoded(1, &encoded);
    write_slot(&mut file, 0, &slot)?;
    inject(fault, FaultPoint::RootSlotWritten)?;
    file.sync_all()?;
    inject(fault, FaultPoint::RootSynced)?;

    let delta = IoCounters {
        bytes_written: (HEADER_BYTES + encoded.bytes.len() + ROOT_SLOT_BYTES) as u64,
        pages_written: 1,
        ..IoCounters::default()
    };
    counters.record(&delta);
    Ok(receipt(&encoded, 1, delta))
}

pub(crate) fn commit_with_fault(
    path: &Path,
    transaction: &PactTransaction,
    counters: &IoCounterSet,
    fault: Option<FaultPoint>,
) -> PactResult<CommitReceipt> {
    commit_checkpointed(path, transaction, counters, fault, None)
}

pub(crate) fn commit_checkpointed(
    path: &Path,
    transaction: &PactTransaction,
    counters: &IoCounterSet,
    fault: Option<FaultPoint>,
    checkpoint: Option<&super::checkpoint::SemanticCheckpoint>,
) -> PactResult<CommitReceipt> {
    let mut read_file = File::open(path)?;
    let (current, previous, mut delta) = select_bounded(&mut read_file)?;
    if transaction.trace.from_rev != previous.transaction.trace.to_rev {
        return Err(PactError::invalid(format!(
            "candidate Pact commit revision {} does not follow current revision {}",
            transaction.trace.from_rev, previous.transaction.trace.to_rev
        )));
    }

    let generation = current.slot.generation + 1;
    let pages = FilePageReader::open(path)?;
    let encoded = encode_transaction_checkpointed(
        transaction,
        generation,
        current.slot.file_length_watermark,
        Some(&pages),
        Some(current.slot.index_root_address),
        checkpoint,
    )?;
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let identity_persisted = persist_missing_file_identity(&mut file)?;
    if file.metadata()?.len() != current.slot.file_length_watermark {
        file.set_len(current.slot.file_length_watermark)?;
    }
    file.seek(SeekFrom::Start(current.slot.file_length_watermark))?;
    write_encoded_records(&mut file, &encoded, fault)?;
    file.sync_data()?;
    inject(fault, FaultPoint::DataSynced)?;

    let slot_index = slot_index_for_next(Some(&current));
    let slot = RootSlot::from_encoded(generation, &encoded);
    write_slot(&mut file, slot_index, &slot)?;
    inject(fault, FaultPoint::RootSlotWritten)?;
    file.sync_all()?;
    inject(fault, FaultPoint::RootSynced)?;

    delta.bytes_read = delta.bytes_read.saturating_add(pages.bytes_read.get());
    delta.bytes_written = (encoded.bytes.len()
        + ROOT_SLOT_BYTES
        + if identity_persisted {
            super::layout::FILE_IDENTITY_BYTES
        } else {
            0
        }) as u64;
    delta.pages_read = delta.pages_read.saturating_add(pages.pages_read.get());
    delta.pages_written = 1;
    delta.records_decoded = delta.records_decoded.saturating_add(1);
    let delta = IoCounters {
        bytes_read: delta.bytes_read,
        bytes_written: delta.bytes_written,
        pages_read: delta.pages_read,
        pages_written: 1,
        records_decoded: delta.records_decoded,
        ..delta
    };
    counters.record(&delta);
    Ok(receipt(&encoded, generation, delta))
}

fn persist_missing_file_identity(file: &mut File) -> PactResult<bool> {
    let mut header = [0_u8; HEADER_BYTES];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    let start = super::layout::FILE_IDENTITY_START;
    let end = start + super::layout::FILE_IDENTITY_BYTES;
    if header[start..end].iter().any(|byte| *byte != 0) {
        return Ok(false);
    }
    let identity = super::envelope::file_identity(&header)?;
    file.seek(SeekFrom::Start(start as u64))?;
    file.write_all(&identity)?;
    Ok(true)
}

pub(crate) fn open_snapshot(path: &Path, counters: &IoCounterSet) -> PactResult<CandidateSnapshot> {
    let mut file = File::open(path)?;
    let (selected, decoded, delta) = select_bounded(&mut file)?;
    counters.record(&delta);
    Ok(CandidateSnapshot {
        generation: selected.slot.generation,
        transaction_address: selected.marker.transaction_address,
        index_root_address: selected.slot.index_root_address,
        committed_len: selected.slot.file_length_watermark,
        transaction: decoded.transaction,
        checkpoint: decoded.checkpoint,
    })
}

pub(crate) fn write_slot(file: &mut File, slot_index: usize, slot: &RootSlot) -> PactResult<()> {
    let offset = ROOT_SLOT_START
        .checked_add(
            slot_index
                .checked_mul(ROOT_SLOT_BYTES)
                .ok_or_else(|| PactError::invalid("candidate Pact root slot index overflow"))?,
        )
        .ok_or_else(|| PactError::invalid("candidate Pact root slot offset overflow"))?;
    file.seek(SeekFrom::Start(offset as u64))?;
    file.write_all(&encode_root_slot(slot))?;
    Ok(())
}

fn write_encoded_records(
    file: &mut File,
    encoded: &super::codec::EncodedTransaction,
    fault: Option<FaultPoint>,
) -> PactResult<()> {
    let directory_end = encoded
        .segment_directory_address
        .checked_add(encoded.segment_directory_len)
        .and_then(|end| end.checked_sub(encoded.transaction_address))
        .and_then(|end| usize::try_from(end).ok())
        .ok_or_else(|| PactError::internal("Pact directory write boundary is invalid"))?;
    let allocator_end = encoded
        .allocator_address
        .checked_add(encoded.allocator_len)
        .and_then(|end| end.checked_sub(encoded.transaction_address))
        .and_then(|end| usize::try_from(end).ok())
        .ok_or_else(|| PactError::internal("Pact allocator write boundary is invalid"))?;
    if directory_end > allocator_end || allocator_end > encoded.bytes.len() {
        return Err(PactError::internal(
            "Pact metadata write boundaries exceed the encoded transaction",
        ));
    }
    file.write_all(&encoded.bytes[..directory_end])?;
    inject(fault, FaultPoint::DirectoryWritten)?;
    file.write_all(&encoded.bytes[directory_end..allocator_end])?;
    inject(fault, FaultPoint::AllocatorWritten)?;
    file.write_all(&encoded.bytes[allocator_end..])?;
    inject(fault, FaultPoint::RecordsWritten)
}

fn receipt(
    encoded: &super::codec::EncodedTransaction,
    generation: u64,
    counters: IoCounters,
) -> CommitReceipt {
    CommitReceipt {
        generation,
        transaction_address: encoded.transaction_address,
        index_root_address: encoded.index_root_address,
        committed_len: encoded.committed_len,
        counters,
    }
}

fn inject(configured: Option<FaultPoint>, point: FaultPoint) -> PactResult<()> {
    if configured == Some(point) {
        return Err(PactError::Io(std::io::Error::other(format!(
            "injected candidate Pact fault after {point:?}"
        ))));
    }
    Ok(())
}

pub(crate) fn select_committed_slot_streamed(
    file: &mut File,
    io_chunk_bytes: usize,
) -> PactResult<(RootSlotSelection, IoCounters)> {
    let file_len = file.metadata()?.len();
    let header = read_range(file, 0, HEADER_BYTES as u64)?;
    let mut bytes_read = header.len() as u64;
    let mut buffer = vec![0_u8; io_chunk_bytes.max(4096)];
    let mut valid = Vec::new();
    for (slot_index, slot) in root_slot_candidates(&header)? {
        if slot.file_length_watermark > file_len
            || slot.committed_transaction_address >= slot.file_length_watermark
        {
            continue;
        }
        let marker_len = slot.file_length_watermark - slot.committed_transaction_address;
        if marker_len != super::codec::COMMIT_RECORD_BYTES {
            continue;
        }
        let marker_region = read_range(file, slot.committed_transaction_address, marker_len)?;
        bytes_read = bytes_read.saturating_add(marker_region.len() as u64);
        let Ok(marker) = decode_commit_marker_region(
            &marker_region,
            slot.committed_transaction_address,
            slot.generation,
        ) else {
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
            || marker.transaction_address >= slot.committed_transaction_address
        {
            continue;
        }
        let head = read_range(file, marker.transaction_address, RECORD_HEADER_BYTES as u64)?;
        bytes_read = bytes_read.saturating_add(head.len() as u64);
        if !record_header_is_transaction(&head, slot.generation) {
            continue;
        }
        let mut hasher = blake3::Hasher::new();
        let mut remaining = slot.committed_transaction_address - marker.transaction_address;
        file.seek(SeekFrom::Start(marker.transaction_address))?;
        while remaining > 0 {
            let take = usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| {
                PactError::corrupt("Pact transaction region exceeds platform limits")
            })?;
            file.read_exact(&mut buffer[..take])?;
            hasher.update(&buffer[..take]);
            remaining -= take as u64;
            bytes_read = bytes_read.saturating_add(take as u64);
        }
        if hasher.finalize().as_bytes() != &marker.root_digest {
            continue;
        }
        valid.push(RootSlotSelection {
            slot_index,
            slot,
            marker,
        });
    }
    let selected = valid
        .into_iter()
        .max_by_key(|selection| selection.slot.generation)
        .ok_or_else(|| PactError::corrupt("Pact file envelope has no valid committed root slot"))?;
    Ok((
        selected,
        IoCounters {
            bytes_read,
            ..IoCounters::default()
        },
    ))
}

fn record_header_is_transaction(head: &[u8], generation: u64) -> bool {
    if head.len() != RECORD_HEADER_BYTES || &head[..4] != super::codec::RECORD_MAGIC {
        return false;
    }
    let kind = head[4];
    let is_transaction = kind == super::codec::RecordKind::Transaction as u8
        || kind == super::codec::RecordKind::CompactTransaction as u8;
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&head[8..16]);
    is_transaction
        && head[5] == super::codec::RECORD_VERSION
        && u64::from_le_bytes(raw) == generation
}

fn select_bounded(
    file: &mut File,
) -> PactResult<(RootSlotSelection, DecodedTransaction, IoCounters)> {
    let file_len = file.metadata()?.len();
    let content_source = FileContentBodyReader {
        file: RefCell::new(file.try_clone()?),
    };
    let header = read_range(file, 0, HEADER_BYTES as u64)?;
    let mut bytes_read = header.len() as u64;
    let mut valid = Vec::new();
    for (slot_index, slot) in root_slot_candidates(&header)? {
        if slot.file_length_watermark > file_len
            || slot.committed_transaction_address >= slot.file_length_watermark
        {
            continue;
        }
        let marker_len = slot.file_length_watermark - slot.committed_transaction_address;
        if marker_len != super::codec::COMMIT_RECORD_BYTES {
            continue;
        }
        let marker_region = read_range(file, slot.committed_transaction_address, marker_len)?;
        bytes_read = bytes_read.saturating_add(marker_region.len() as u64);
        let Ok(marker) = decode_commit_marker_region(
            &marker_region,
            slot.committed_transaction_address,
            slot.generation,
        ) else {
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
            || marker.transaction_address >= marker.file_length_watermark
        {
            continue;
        }
        let transaction_len = marker.file_length_watermark - marker.transaction_address;
        let transaction_region = read_range(file, marker.transaction_address, transaction_len)?;
        bytes_read = bytes_read.saturating_add(transaction_region.len() as u64);
        let Ok(decoded) = decode_transaction_region(
            &transaction_region,
            marker.transaction_address,
            slot.generation,
            file_len,
            &content_source,
        ) else {
            continue;
        };
        if decoded.marker_address != slot.committed_transaction_address || decoded.marker != marker
        {
            continue;
        }
        valid.push((
            RootSlotSelection {
                slot_index,
                slot,
                marker,
            },
            decoded,
        ));
    }
    let (selected, decoded) = valid
        .into_iter()
        .max_by_key(|(selection, _)| selection.slot.generation)
        .ok_or_else(|| PactError::corrupt("Pact file envelope has no valid committed root slot"))?;
    Ok((
        selected,
        decoded,
        IoCounters {
            bytes_read,
            records_decoded: 1,
            ..IoCounters::default()
        },
    ))
}

pub(crate) fn read_range(file: &mut File, address: u64, len: u64) -> PactResult<Vec<u8>> {
    let len = usize::try_from(len)
        .map_err(|_| PactError::corrupt("Pact record range exceeds platform limits"))?;
    let mut bytes = vec![0_u8; len];
    file.seek(SeekFrom::Start(address))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

struct FilePageReader {
    file: RefCell<File>,
    batches: RefCell<BTreeMap<u64, Vec<[u8; super::btree::PAGE_BYTES]>>>,
    bytes_read: Cell<u64>,
    pages_read: Cell<u64>,
}

struct SlicePageReader<'a> {
    file: &'a [u8],
}

impl super::btree::PageReader for SlicePageReader<'_> {
    fn read_page(&self, address: u64) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
        super::codec::decode_index_page_record(self.file, address)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> PactResult<Vec<u8>> {
        super::codec::decode_component_payload_batch_record(self.file, address, ordinal)
    }
}

struct FileContentBodyReader {
    file: RefCell<File>,
}

impl ContentBodySource for FileContentBodyReader {
    fn read_content_body(&self, address: u64) -> PactResult<Vec<u8>> {
        let header = read_range(
            &mut self.file.borrow_mut(),
            address,
            RECORD_HEADER_BYTES as u64,
        )?;
        let record_len = content_body_record_len(&header)?;
        let region = read_range(&mut self.file.borrow_mut(), address, record_len)?;
        decode_content_body_record_region(&region)
    }
}

impl FilePageReader {
    fn open(path: &Path) -> PactResult<Self> {
        Ok(Self {
            file: RefCell::new(File::open(path)?),
            batches: RefCell::new(BTreeMap::new()),
            bytes_read: Cell::new(0),
            pages_read: Cell::new(0),
        })
    }
}

impl super::btree::PageReader for FilePageReader {
    fn read_page(&self, address: u64) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
        let batch_address = super::codec::index_page_batch_address(address);
        let ordinal = super::codec::index_page_ordinal(address);
        if let Some(page) = self
            .batches
            .borrow()
            .get(&batch_address)
            .and_then(|pages| pages.get(ordinal))
        {
            return Ok(*page);
        }
        let header = read_range(
            &mut self.file.borrow_mut(),
            batch_address,
            RECORD_HEADER_BYTES as u64,
        )?;
        let len = index_page_record_len(&header)?;
        let region = read_range(&mut self.file.borrow_mut(), batch_address, len)?;
        let pages = super::codec::decode_index_page_batch_record_region(&region)?;
        let page = pages
            .get(ordinal)
            .copied()
            .ok_or_else(|| PactError::corrupt("Pact index page ordinal is out of range"))?;
        self.batches.borrow_mut().insert(batch_address, pages);
        self.bytes_read
            .set(self.bytes_read.get().saturating_add(len));
        self.pages_read.set(self.pages_read.get().saturating_add(1));
        Ok(page)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> PactResult<Vec<u8>> {
        let header = read_range(
            &mut self.file.borrow_mut(),
            address,
            RECORD_HEADER_BYTES as u64,
        )?;
        let len = component_payload_batch_record_len(&header)?;
        let region = read_range(&mut self.file.borrow_mut(), address, len)?;
        super::codec::decode_component_payload_batch_record_region(&region, ordinal)
    }
}
