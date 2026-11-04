use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::transaction::PactTransaction;
use crate::error::{PactError, PactResult};
use crate::store::{ContentRecord, PactTransactionTrace};
use crate::trace::BoundaryKind;
use crate::transaction_trace::{decode_transaction_trace_with, encode_transaction_trace};

#[cfg(test)]
mod tests;

pub(crate) const RECORD_MAGIC: &[u8; 4] = b"PRC2";
pub(crate) const RECORD_VERSION: u8 = 1;
pub(crate) const RECORD_HEADER_BYTES: usize = 32;
pub(crate) const MAX_RECORD_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LEDGER_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const RECORD_FLAG_ZSTD: u16 = 1;
const MAX_SEMANTIC_EXPANSION_RATIO: usize = 128;
const SEMANTIC_DECODE_SLACK_BYTES: usize = 64 * 1024;
pub(crate) const COMMIT_PAYLOAD_BYTES: usize = 128;
pub(crate) const COMMIT_RECORD_BYTES: u64 = (RECORD_HEADER_BYTES + COMMIT_PAYLOAD_BYTES) as u64;
const INDEX_BATCH_MAGIC: &[u8; 4] = b"PZB2";
const INDEX_BATCH_HEADER_BYTES: usize = 12;
const COMPONENT_BATCH_MAGIC: &[u8; 4] = b"PCB2";
const COMPONENT_BATCH_HEADER_BYTES: usize = 12;
const CONTENT_BATCH_MAGIC: &[u8; 4] = b"PCT2";
const CONTENT_BATCH_HEADER_BYTES: usize = 12;
const COLD_COMPRESSION_LEVEL: i32 = 6;
const COLD_CONTENT_COMPRESSION_LEVEL: i32 = 19;
pub(crate) const INDEX_PAGE_ORDINAL_BITS: u32 = 15;

pub(crate) const MAX_INDEX_BATCH_PAGES: usize = 1 << INDEX_PAGE_ORDINAL_BITS;

pub(crate) const INDEX_BATCH_TARGET_PAGES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum RecordKind {
    Transaction = 1,
    Trace = 2,
    PublicTrace = 3,
    Content = 4,
    IndexPage = 5,
    IndexDelta = 6,
    Commit = 7,
    ContentBody = 8,
    Checkpoint = 9,
    LedgerDelta = 10,
    CompactTransaction = 11,
    ComponentPayloadBatch = 12,
    SegmentDirectory = 13,
    Allocator = 14,
    ContentBatch = 15,
}

impl RecordKind {
    fn parse(raw: u8) -> PactResult<Self> {
        match raw {
            1 => Ok(Self::Transaction),
            2 => Ok(Self::Trace),
            3 => Ok(Self::PublicTrace),
            4 => Ok(Self::Content),
            5 => Ok(Self::IndexPage),
            6 => Ok(Self::IndexDelta),
            7 => Ok(Self::Commit),
            8 => Ok(Self::ContentBody),
            9 => Ok(Self::Checkpoint),
            10 => Ok(Self::LedgerDelta),
            11 => Ok(Self::CompactTransaction),
            12 => Ok(Self::ComponentPayloadBatch),
            13 => Ok(Self::SegmentDirectory),
            14 => Ok(Self::Allocator),
            15 => Ok(Self::ContentBatch),
            _ => Err(PactError::corrupt(format!(
                "unknown Pact transaction record kind: {raw}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitMarker {
    pub(crate) generation: u64,
    pub(crate) transaction_address: u64,
    pub(crate) index_root_address: u64,
    pub(crate) file_length_watermark: u64,
    pub(crate) segment_directory_address: u64,
    pub(crate) segment_directory_len: u64,
    pub(crate) allocator_address: u64,
    pub(crate) allocator_len: u64,
    pub(crate) segment_directory_digest: [u8; 16],
    pub(crate) allocator_digest: [u8; 16],
    pub(crate) root_digest: [u8; 32],
}

pub(crate) struct EncodedTransaction {
    pub(crate) bytes: Vec<u8>,
    pub(crate) transaction_address: u64,
    pub(crate) index_root_address: u64,
    pub(crate) commit_marker_address: u64,
    pub(crate) committed_len: u64,
    pub(crate) segment_directory_address: u64,
    pub(crate) segment_directory_len: u64,
    pub(crate) allocator_address: u64,
    pub(crate) allocator_len: u64,
    pub(crate) segment_directory_digest: [u8; 16],
    pub(crate) allocator_digest: [u8; 16],
}

#[derive(Debug)]
pub(crate) struct DecodedTransaction {
    pub(crate) transaction: PactTransaction,
    pub(crate) checkpoint: Option<super::checkpoint::SemanticCheckpoint>,
    pub(crate) marker: CommitMarker,
    pub(crate) marker_address: u64,
}

pub(crate) trait ContentBodySource {
    fn read_content_body(&self, address: u64) -> PactResult<Vec<u8>>;
}

struct SliceContentBodySource<'a>(&'a [u8]);

impl ContentBodySource for SliceContentBodySource<'_> {
    fn read_content_body(&self, address: u64) -> PactResult<Vec<u8>> {
        decode_content_body_record(self.0, address)
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ContentManifest {
    pub(crate) record: ContentRecord,
    pub(crate) body_addresses: Vec<u64>,
    #[serde(default)]
    pub(crate) body_offsets: Vec<u64>,
    pub(crate) body_is_bytes: bool,
}

pub(crate) struct ContentRangeDescriptor {
    pub(crate) record: ContentRecord,
    pub(crate) body_addresses: Vec<u64>,
    pub(crate) body_offsets: Vec<u64>,
    pub(crate) body_is_bytes: bool,
}

pub(crate) fn encode_transaction(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
) -> PactResult<EncodedTransaction> {
    encode_transaction_checkpointed(
        transaction,
        generation,
        transaction_address,
        None,
        None,
        None,
    )
}

pub(crate) fn encode_transaction_with_index(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
    existing_pages: Option<&dyn super::btree::PageReader>,
    existing_index_root: Option<u64>,
) -> PactResult<EncodedTransaction> {
    encode_transaction_checkpointed(
        transaction,
        generation,
        transaction_address,
        existing_pages,
        existing_index_root,
        None,
    )
}

pub(crate) fn encode_transaction_checkpointed(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
    existing_pages: Option<&dyn super::btree::PageReader>,
    existing_index_root: Option<u64>,
    checkpoint: Option<&super::checkpoint::SemanticCheckpoint>,
) -> PactResult<EncodedTransaction> {
    encode_transaction_checkpointed_inner(
        transaction,
        generation,
        transaction_address,
        existing_pages,
        existing_index_root,
        checkpoint,
        false,
    )
}

pub(crate) fn encode_transaction_checkpointed_compact(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
    existing_pages: Option<&dyn super::btree::PageReader>,
    existing_index_root: Option<u64>,
    checkpoint: Option<&super::checkpoint::SemanticCheckpoint>,
) -> PactResult<EncodedTransaction> {
    encode_transaction_checkpointed_inner_mode(
        transaction,
        generation,
        transaction_address,
        existing_pages,
        existing_index_root,
        checkpoint,
        false,
        true,
        None,
        None,
        None,
    )
}

pub(crate) fn encode_transaction_checkpointed_cold(
    mut transaction: PactTransaction,
    generation: u64,
    transaction_address: u64,
) -> PactResult<EncodedTransaction> {
    let baseline_ops = super::index::baseline_component_ops(
        transaction
            .head_checkpoint
            .as_ref()
            .ok_or_else(|| PactError::invalid("cold Pact transaction requires a baseline"))?,
    )?;
    let ledger = transaction
        .head_checkpoint
        .take()
        .ok_or_else(|| PactError::invalid("cold Pact transaction requires a baseline"))?;
    let decoded_ledger: crate::store::ContextTreeLedger = serde_json::from_value(ledger.clone())
        .map_err(|error| PactError::invalid(format!("invalid cold Pact ledger: {error}")))?;
    let text_changes = super::change_feed::indexed_text_changes_from_ledger(&decoded_ledger)?;
    let search_changes =
        super::search_change_feed::indexed_search_changes_from_ledger(&decoded_ledger)?;
    transaction.head_checkpoint = Some(super::head_checkpoint::strip_derived_head_owned(ledger)?);
    let baseline_branches = [super::index::BaselineBranchOps {
        branch_id: transaction.branch_id.clone(),
        revision: transaction.trace.to_rev,
        ops: baseline_ops,
    }];
    encode_transaction_checkpointed_cold_prepared(
        transaction,
        generation,
        transaction_address,
        &baseline_branches,
        &text_changes,
        &search_changes,
    )
}

pub(crate) fn encode_transaction_checkpointed_cold_prepared(
    transaction: PactTransaction,
    generation: u64,
    transaction_address: u64,
    baseline_branches: &[super::index::BaselineBranchOps],
    text_changes: &[super::change_feed::IndexedTextChange],
    search_changes: &[super::search_change_feed::IndexedSearchChange],
) -> PactResult<EncodedTransaction> {
    if !transaction
        .head_checkpoint
        .as_ref()
        .is_some_and(super::head_checkpoint::is_compact_ledger)
    {
        return Err(PactError::invalid(
            "prepared cold Pact transaction requires a compact baseline",
        ));
    }
    encode_transaction_checkpointed_inner_mode(
        &transaction,
        generation,
        transaction_address,
        None,
        None,
        None,
        false,
        false,
        Some(baseline_branches),
        Some(text_changes),
        Some(search_changes),
    )
}

#[allow(clippy::too_many_arguments)]
fn encode_transaction_checkpointed_inner(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
    existing_pages: Option<&dyn super::btree::PageReader>,
    existing_index_root: Option<u64>,
    checkpoint: Option<&super::checkpoint::SemanticCheckpoint>,
    legacy_content_index: bool,
) -> PactResult<EncodedTransaction> {
    encode_transaction_checkpointed_inner_mode(
        transaction,
        generation,
        transaction_address,
        existing_pages,
        existing_index_root,
        checkpoint,
        legacy_content_index,
        true,
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn encode_transaction_checkpointed_inner_mode(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
    existing_pages: Option<&dyn super::btree::PageReader>,
    existing_index_root: Option<u64>,
    checkpoint: Option<&super::checkpoint::SemanticCheckpoint>,
    legacy_content_index: bool,
    compact_core: bool,
    cold_baseline_branches: Option<&[super::index::BaselineBranchOps]>,
    cold_text_changes: Option<&[super::change_feed::IndexedTextChange]>,
    cold_search_changes: Option<&[super::search_change_feed::IndexedSearchChange]>,
) -> PactResult<EncodedTransaction> {
    transaction.validate()?;
    let cold_baseline = cold_baseline_branches.is_some();
    if generation == 0 {
        return Err(PactError::invalid(
            "Pact transaction generation must be positive",
        ));
    }

    let mut bytes = Vec::new();
    if compact_core {
        let core = super::compact_codec::encode_standalone_core(transaction)?;
        push_record(
            &mut bytes,
            RecordKind::CompactTransaction,
            generation,
            &core,
        )?;
    } else {
        let mut header = Vec::new();
        write_string(&mut header, &transaction.transaction_id)?;
        write_string(&mut header, &transaction.branch_id)?;
        header.push(encode_boundary(transaction.boundary));
        match &transaction.head_checkpoint {
            Some(ledger) => {
                if !super::head_checkpoint::is_compact_ledger(ledger) {
                    return Err(PactError::invalid(
                        "cold Pact transaction requires a compact head checkpoint",
                    ));
                }
                header.push(3);
                let ledger = serde_json::to_vec(&ledger).map_err(|error| {
                    PactError::codec(format!("encode Pact head checkpoint: {error}"))
                })?;
                if ledger.len() > MAX_LEDGER_SNAPSHOT_BYTES {
                    return Err(PactError::invalid(format!(
                        "Pact head checkpoint exceeds {MAX_LEDGER_SNAPSHOT_BYTES} decoded bytes"
                    )));
                }
                let compressed =
                    zstd::bulk::compress(&ledger, COLD_COMPRESSION_LEVEL).map_err(|error| {
                        PactError::codec(format!("compress Pact head checkpoint: {error}"))
                    })?;
                write_bytes(&mut header, &(ledger.len() as u64).to_le_bytes())?;
                write_bytes(&mut header, &compressed)?;
            }
            None => header.push(0),
        }
        push_record(&mut bytes, RecordKind::Transaction, generation, &header)?;

        let trace = compressed_payload(&encode_transaction_trace(&transaction.trace))?;
        push_record(&mut bytes, RecordKind::Trace, generation, &trace)?;

        let public_trace = serde_json::to_vec(&transaction.public_trace)
            .map_err(|error| PactError::codec(format!("encode public Pact trace: {error}")))?;
        let public_trace = compressed_payload(&public_trace)?;
        push_record(
            &mut bytes,
            RecordKind::PublicTrace,
            generation,
            &public_trace,
        )?;

        if let Some(delta) = &transaction.ledger_delta {
            let payload = serde_json::to_vec(delta)
                .map_err(|error| PactError::codec(format!("encode Pact ledger delta: {error}")))?;
            let payload = compressed_payload(&payload)?;
            push_record(&mut bytes, RecordKind::LedgerDelta, generation, &payload)?;
        }
    }

    let mut content_bodies = BTreeMap::new();
    let mut content_manifests = BTreeMap::new();
    let mut batched_content = Vec::new();
    let mut addressed_content = Vec::new();
    for content in &transaction.new_content {
        let (payload, _) = captured_content_payload(content)?;
        if cold_baseline && payload.len() <= 256 * 1024 {
            batched_content.push(content.clone());
        } else {
            addressed_content.push(content);
        }
    }
    if !batched_content.is_empty() {
        let batch_address = transaction_address
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| PactError::invalid("Pact content batch address overflow"))?;
        bytes.extend_from_slice(&encode_content_batch_record(generation, &batched_content)?);
        content_manifests.extend(
            batched_content
                .iter()
                .map(|content| (content.id.clone(), batch_address)),
        );
    }
    for content in addressed_content {
        let (payload, body_is_bytes) = captured_content_payload(content)?;
        let mut body_addresses = Vec::new();
        let mut body_offsets = Vec::new();
        let mut body_offset = 0_u64;
        for chunk in content_chunks(payload, body_is_bytes) {
            body_offsets.push(body_offset);
            body_offset = body_offset.saturating_add(chunk.len() as u64);
            let chunk_hash = *blake3::hash(chunk).as_bytes();
            let existing_address = match (existing_pages, existing_index_root) {
                (Some(pages), Some(root)) => super::index::read_index_value_from_pages(
                    pages,
                    root,
                    &super::index::IndexKey::ContentHash { hash: chunk_hash },
                )?
                .map(|value| decode_u64_value(&value, "Pact content body address"))
                .transpose()?,
                _ => None,
            };
            let body_address = if let Some(address) = content_bodies
                .get(&chunk_hash)
                .copied()
                .or(existing_address)
            {
                address
            } else {
                let address = transaction_address
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| PactError::invalid("Pact content body address overflow"))?;
                let (compressed, stored_chunk) = encoded_content_body_payload(chunk)?;
                push_record_with_flags(
                    &mut bytes,
                    RecordKind::ContentBody,
                    generation,
                    if compressed { RECORD_FLAG_ZSTD } else { 0 },
                    &stored_chunk,
                )?;
                address
            };
            content_bodies.insert(chunk_hash, body_address);
            body_addresses.push(body_address);
        }
        let mut record = content.clone();
        record.storage.inline = None;
        record.storage.inline_bytes = None;
        let manifest_address = transaction_address
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| PactError::invalid("Pact content manifest address overflow"))?;
        let manifest = serde_json::to_vec(&ContentManifest {
            record,
            body_addresses,
            body_offsets,
            body_is_bytes,
        })
        .map_err(|error| PactError::codec(format!("encode Pact content manifest: {error}")))?;
        push_record(&mut bytes, RecordKind::Content, generation, &manifest)?;
        content_manifests.insert(content.id.clone(), manifest_address);
    }

    if let Some(checkpoint) = checkpoint {
        if checkpoint.branch_id != transaction.branch_id
            || checkpoint.revision != transaction.trace.to_rev
        {
            return Err(PactError::invalid(
                "Pact checkpoint identity does not match its transaction",
            ));
        }
        let payload = super::checkpoint::encode_checkpoint(checkpoint)?;
        push_record(&mut bytes, RecordKind::Checkpoint, generation, &payload)?;
    }

    let component_batch_address = if cold_baseline {
        let payloads = if let Some(branches) = cold_baseline_branches {
            super::index::baseline_component_payloads_from_branches(branches, generation)?
        } else {
            super::index::baseline_component_payloads(transaction, generation)?
        };
        if payloads.is_empty() {
            None
        } else {
            let address = transaction_address
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| PactError::invalid("Pact component batch address overflow"))?;
            bytes.extend_from_slice(&encode_component_payload_batch_record(
                generation, &payloads,
            )?);
            Some(address)
        }
    } else {
        None
    };

    let index_start_address = transaction_address
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| PactError::invalid("Pact transaction address overflow"))?;
    let index = super::index::build_transaction_index(
        transaction,
        generation,
        transaction_address,
        existing_pages,
        existing_index_root,
        index_start_address,
        super::index::ContentIndexInputs {
            body_addresses: &content_bodies,
            manifest_addresses: &content_manifests,
            legacy_content_id_values: legacy_content_index,
            component_batch_address,
            baseline_branches_override: cold_baseline_branches,
            text_changes_override: cold_text_changes,
            search_changes_override: cold_search_changes,
        },
    )?;
    let index_root_address = index.root_address;
    bytes.extend_from_slice(&index.bytes);
    push_record(
        &mut bytes,
        RecordKind::IndexDelta,
        generation,
        &index_root_address.to_le_bytes(),
    )?;

    let segment_directory_address = transaction_address
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| PactError::invalid("Pact segment directory address overflow"))?;
    let mut directory_payload = Vec::with_capacity(24);
    directory_payload.extend_from_slice(&generation.to_le_bytes());
    directory_payload.extend_from_slice(&transaction_address.to_le_bytes());
    directory_payload.extend_from_slice(&index_root_address.to_le_bytes());
    let directory_start = bytes.len();
    push_record(
        &mut bytes,
        RecordKind::SegmentDirectory,
        generation,
        &directory_payload,
    )?;
    let segment_directory_len = u64::try_from(bytes.len() - directory_start)
        .map_err(|_| PactError::invalid("Pact segment directory length overflow"))?;
    let mut segment_directory_digest = [0_u8; 16];
    segment_directory_digest
        .copy_from_slice(&blake3::hash(&bytes[directory_start..]).as_bytes()[..16]);

    let allocator_address = transaction_address
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| PactError::invalid("Pact allocator address overflow"))?;
    let allocator_record_len = (RECORD_HEADER_BYTES + 16) as u64;
    let committed_len = allocator_address
        .checked_add(allocator_record_len)
        .and_then(|address| address.checked_add(COMMIT_RECORD_BYTES))
        .ok_or_else(|| PactError::invalid("Pact committed length overflow"))?;
    let mut allocator_payload = Vec::with_capacity(16);
    allocator_payload.extend_from_slice(&generation.to_le_bytes());
    allocator_payload.extend_from_slice(&committed_len.to_le_bytes());
    let allocator_start = bytes.len();
    push_record(
        &mut bytes,
        RecordKind::Allocator,
        generation,
        &allocator_payload,
    )?;
    let allocator_len = u64::try_from(bytes.len() - allocator_start)
        .map_err(|_| PactError::invalid("Pact allocator length overflow"))?;
    if allocator_len != allocator_record_len {
        return Err(PactError::internal(
            "Pact allocator record length changed unexpectedly",
        ));
    }
    let mut allocator_digest = [0_u8; 16];
    allocator_digest.copy_from_slice(&blake3::hash(&bytes[allocator_start..]).as_bytes()[..16]);

    let commit_marker_address = transaction_address
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| PactError::invalid("Pact commit marker address overflow"))?;
    if commit_marker_address.checked_add(COMMIT_RECORD_BYTES) != Some(committed_len) {
        return Err(PactError::internal(
            "Pact commit marker length prediction mismatch",
        ));
    }
    let root_digest = *blake3::hash(&bytes).as_bytes();
    let marker = CommitMarker {
        generation,
        transaction_address,
        index_root_address,
        file_length_watermark: committed_len,
        segment_directory_address,
        segment_directory_len,
        allocator_address,
        allocator_len,
        segment_directory_digest,
        allocator_digest,
        root_digest,
    };
    push_record(
        &mut bytes,
        RecordKind::Commit,
        generation,
        &encode_commit_marker(&marker),
    )?;

    Ok(EncodedTransaction {
        bytes,
        transaction_address,
        index_root_address,
        commit_marker_address,
        committed_len,
        segment_directory_address,
        segment_directory_len,
        allocator_address,
        allocator_len,
        segment_directory_digest,
        allocator_digest,
    })
}

pub(crate) fn decode_transaction(
    file: &[u8],
    transaction_address: u64,
    expected_generation: u64,
) -> PactResult<DecodedTransaction> {
    decode_transaction_with(file, transaction_address, expected_generation, None)
}

pub(crate) fn decode_transaction_with(
    file: &[u8],
    transaction_address: u64,
    expected_generation: u64,
    derived_branches: Option<&std::collections::BTreeSet<String>>,
) -> PactResult<DecodedTransaction> {
    let start = usize::try_from(transaction_address)
        .map_err(|_| PactError::corrupt("Pact transaction address exceeds platform limits"))?;
    if start >= file.len() {
        return Err(PactError::corrupt(
            "Pact transaction address exceeds file length",
        ));
    }

    let content_source = SliceContentBodySource(file);
    decode_transaction_inner(
        &file[start..],
        transaction_address,
        expected_generation,
        file.len() as u64,
        &content_source,
        derived_branches,
    )
}

fn decode_transaction_inner(
    region: &[u8],
    transaction_address: u64,
    expected_generation: u64,
    file_length: u64,
    content_source: &dyn ContentBodySource,
    derived_branches: Option<&std::collections::BTreeSet<String>>,
) -> PactResult<DecodedTransaction> {
    #[cfg(test)]
    crate::tests::instrumentation::PACT_TRANSACTION_DECODE_COUNT
        .with(|count| count.set(count.get() + 1));
    let mut position = 0;
    let mut transaction_id = None;
    let mut branch_id = None;
    let mut boundary = None;
    let mut head_checkpoint = None;
    let mut trace = None;
    let mut public_trace = None;
    let mut content = Vec::new();
    let mut checkpoint = None;
    let mut ledger_delta = None;
    let mut compact_core = None;
    let mut index_root_address = None;
    let mut segment_directory = None;
    let mut allocator = None;

    loop {
        let record_offset = position;
        let record_address = transaction_address
            .checked_add(record_offset as u64)
            .ok_or_else(|| PactError::corrupt("Pact record address overflow"))?;
        let record = read_record(region, &mut position)?;
        if record.generation != expected_generation {
            return Err(PactError::corrupt(format!(
                "Pact record generation {} does not match root generation {expected_generation}",
                record.generation
            )));
        }
        match record.kind {
            RecordKind::CompactTransaction => {
                if compact_core.is_some() {
                    return Err(PactError::corrupt(
                        "duplicate compact Pact transaction core",
                    ));
                }
                compact_core = Some(super::compact_codec::decode_standalone_core_with(
                    record.payload,
                    derived_branches,
                )?);
            }
            RecordKind::Transaction => {
                if transaction_id.is_some() {
                    return Err(PactError::corrupt("duplicate Pact transaction header"));
                }
                let mut cursor = 0;
                transaction_id = Some(read_string(record.payload, &mut cursor)?);
                branch_id = Some(read_string(record.payload, &mut cursor)?);
                let boundary_code = *record
                    .payload
                    .get(cursor)
                    .ok_or_else(|| PactError::corrupt("truncated Pact transaction boundary"))?;
                cursor += 1;
                let genesis_code = *record
                    .payload
                    .get(cursor)
                    .ok_or_else(|| PactError::corrupt("truncated Pact genesis marker"))?;
                cursor += 1;
                head_checkpoint = match genesis_code {
                    0 => None,
                    3 => {
                        let raw_len = read_bytes(record.payload, &mut cursor)?;
                        let raw_len: [u8; 8] = raw_len.try_into().map_err(|_| {
                            PactError::corrupt("Pact head checkpoint length has invalid width")
                        })?;
                        let raw_len =
                            usize::try_from(u64::from_le_bytes(raw_len)).map_err(|_| {
                                PactError::corrupt("Pact head checkpoint exceeds platform limits")
                            })?;
                        if raw_len > MAX_LEDGER_SNAPSHOT_BYTES {
                            return Err(PactError::corrupt(
                                "decoded Pact head checkpoint exceeds its fixed limit",
                            ));
                        }
                        let compressed = read_bytes(record.payload, &mut cursor)?;
                        let ledger =
                            zstd::bulk::decompress(compressed, raw_len).map_err(|error| {
                                PactError::corrupt(format!(
                                    "invalid compressed Pact head checkpoint: {error}"
                                ))
                            })?;
                        if ledger.len() != raw_len {
                            return Err(PactError::corrupt(
                                "decoded Pact head checkpoint length does not match its header",
                            ));
                        }
                        Some(serde_json::from_slice(&ledger).map_err(|error| {
                            PactError::corrupt(format!("invalid Pact head checkpoint: {error}"))
                        })?)
                    }
                    _ => return Err(PactError::corrupt("invalid Pact genesis marker")),
                };
                if cursor != record.payload.len() {
                    return Err(PactError::corrupt(
                        "Pact transaction header has trailing bytes",
                    ));
                }
                boundary = Some(decode_boundary(boundary_code)?);
            }
            RecordKind::Trace => {
                if trace.is_some() {
                    return Err(PactError::corrupt("duplicate canonical Pact trace"));
                }
                let payload = decompressed_payload(record.payload)?;
                let skip_derived_depth = branch_id
                    .as_ref()
                    .is_some_and(|branch| derived_branches.is_some_and(|set| set.contains(branch)));
                trace = Some(decode_transaction_trace_with(&payload, skip_derived_depth)?);
            }
            RecordKind::PublicTrace => {
                if public_trace.is_some() {
                    return Err(PactError::corrupt("duplicate public Pact trace"));
                }
                public_trace = Some(
                    serde_json::from_slice::<PactTransactionTrace>(&decompressed_payload(
                        record.payload,
                    )?)
                    .map_err(|error| {
                        PactError::corrupt(format!("invalid public Pact trace: {error}"))
                    })?,
                );
            }
            RecordKind::Content => {
                let mut manifest = serde_json::from_slice::<ContentManifest>(record.payload)
                    .map_err(|error| {
                        PactError::corrupt(format!("invalid Pact content manifest: {error}"))
                    })?;
                let body = manifest
                    .body_addresses
                    .into_iter()
                    .map(|address| content_source.read_content_body(address))
                    .collect::<PactResult<Vec<_>>>()?
                    .concat();
                if manifest.body_is_bytes {
                    manifest.record.storage.inline_bytes = Some(body);
                } else {
                    manifest.record.storage.inline =
                        Some(String::from_utf8(body).map_err(|_| {
                            PactError::corrupt("Pact text content body is not UTF-8")
                        })?);
                }
                content.push(manifest.record);
            }
            RecordKind::ContentBatch => {
                content.extend(decode_content_batch_payload(record.payload)?);
            }
            RecordKind::ContentBody => {}
            RecordKind::ComponentPayloadBatch => {
                component_batch_metadata(record.payload)?;
            }
            RecordKind::SegmentDirectory => {
                if segment_directory.is_some()
                    || record.payload.len() != 24
                    || read_u64(record.payload, 0)? != expected_generation
                    || read_u64(record.payload, 8)? != transaction_address
                    || Some(read_u64(record.payload, 16)?) != index_root_address
                {
                    return Err(PactError::corrupt("invalid Pact segment directory record"));
                }
                let mut digest = [0_u8; 16];
                digest.copy_from_slice(
                    &blake3::hash(&region[record_offset..position]).as_bytes()[..16],
                );
                segment_directory = Some((
                    record_address,
                    u64::try_from(position - record_offset)
                        .map_err(|_| PactError::corrupt("Pact directory length overflow"))?,
                    digest,
                ));
            }
            RecordKind::Allocator => {
                if allocator.is_some()
                    || record.payload.len() != 16
                    || read_u64(record.payload, 0)? != expected_generation
                {
                    return Err(PactError::corrupt("invalid Pact allocator record"));
                }
                let mut digest = [0_u8; 16];
                digest.copy_from_slice(
                    &blake3::hash(&region[record_offset..position]).as_bytes()[..16],
                );
                allocator = Some((
                    record_address,
                    u64::try_from(position - record_offset)
                        .map_err(|_| PactError::corrupt("Pact allocator length overflow"))?,
                    digest,
                    read_u64(record.payload, 8)?,
                ));
            }
            RecordKind::Checkpoint => {
                if checkpoint.is_some() {
                    return Err(PactError::corrupt("duplicate Pact semantic checkpoint"));
                }
                checkpoint = Some(super::checkpoint::decode_checkpoint(record.payload)?);
            }
            RecordKind::LedgerDelta => {
                if ledger_delta.is_some() {
                    return Err(PactError::corrupt("duplicate Pact ledger delta"));
                }
                let payload = decompressed_payload(record.payload)?;
                ledger_delta = Some(serde_json::from_slice(&payload).map_err(|error| {
                    PactError::corrupt(format!("invalid Pact ledger delta: {error}"))
                })?);
            }
            RecordKind::IndexPage => {
                index_batch_metadata(record.payload)?;
            }
            RecordKind::IndexDelta => {
                if index_root_address.is_some() {
                    return Err(PactError::corrupt("duplicate Pact index delta"));
                }
                if record.payload.len() != 8 {
                    return Err(PactError::corrupt(
                        "Pact index delta must contain one root address",
                    ));
                }
                index_root_address = Some(read_u64(record.payload, 0)?);
            }
            RecordKind::Commit => {
                let marker = decode_commit_marker(record.payload)?;
                let (directory_address, directory_len, directory_digest) = segment_directory
                    .ok_or_else(|| PactError::corrupt("Pact transaction missing directory"))?;
                let (allocator_address, allocator_len, allocator_digest, allocator_file_end) =
                    allocator
                        .ok_or_else(|| PactError::corrupt("Pact transaction missing allocator"))?;
                if marker.generation != expected_generation
                    || marker.transaction_address != transaction_address
                    || marker.index_root_address
                        != index_root_address.ok_or_else(|| {
                            PactError::corrupt("Pact transaction missing index delta")
                        })?
                    || marker.file_length_watermark
                        != transaction_address.saturating_add(position as u64)
                    || marker.file_length_watermark > file_length
                    || marker.segment_directory_address != directory_address
                    || marker.segment_directory_len != directory_len
                    || marker.allocator_address != allocator_address
                    || marker.allocator_len != allocator_len
                    || marker.segment_directory_digest != directory_digest
                    || marker.allocator_digest != allocator_digest
                    || allocator_file_end != marker.file_length_watermark
                    || marker.root_digest != *blake3::hash(&region[..record_offset]).as_bytes()
                {
                    return Err(PactError::corrupt("Pact commit marker mismatch"));
                }
                let transaction = if let Some(mut transaction) = compact_core.take() {
                    if transaction_id.is_some()
                        || branch_id.is_some()
                        || boundary.is_some()
                        || head_checkpoint.is_some()
                        || trace.is_some()
                        || public_trace.is_some()
                        || ledger_delta.is_some()
                    {
                        return Err(PactError::corrupt(
                            "Pact transaction mixes compact and pre-v0.1 core records",
                        ));
                    }
                    if !transaction.new_content.is_empty() {
                        return Err(PactError::corrupt(
                            "compact Pact core duplicates content manifests",
                        ));
                    }
                    transaction.new_content = content;
                    transaction
                } else {
                    PactTransaction {
                        transaction_id: transaction_id
                            .ok_or_else(|| PactError::corrupt("missing Pact transaction header"))?,
                        branch_id: branch_id
                            .ok_or_else(|| PactError::corrupt("missing Pact transaction branch"))?,
                        boundary: boundary.ok_or_else(|| {
                            PactError::corrupt("missing Pact transaction boundary")
                        })?,
                        trace: trace
                            .ok_or_else(|| PactError::corrupt("missing canonical Pact trace"))?,
                        public_trace: public_trace
                            .ok_or_else(|| PactError::corrupt("missing public Pact trace"))?,
                        new_content: content,
                        head_checkpoint,
                        ledger_delta,
                        head_metadata: None,
                    }
                };
                transaction.validate().map_err(|error| {
                    PactError::corrupt(format!("invalid committed Pact transaction: {error}"))
                })?;
                if let Some(checkpoint) = &checkpoint
                    && (checkpoint.branch_id != transaction.branch_id
                        || checkpoint.revision != transaction.trace.to_rev)
                {
                    return Err(PactError::corrupt(
                        "Pact checkpoint identity does not match its transaction",
                    ));
                }
                return Ok(DecodedTransaction {
                    transaction,
                    checkpoint,
                    marker,
                    marker_address: record_address,
                });
            }
        }
    }
}

pub(crate) fn decode_transaction_region(
    region: &[u8],
    transaction_address: u64,
    expected_generation: u64,
    file_length: u64,
    content_source: &dyn ContentBodySource,
) -> PactResult<DecodedTransaction> {
    decode_transaction_inner(
        region,
        transaction_address,
        expected_generation,
        file_length,
        content_source,
        None,
    )
}

pub(crate) struct Record<'a> {
    pub(crate) kind: RecordKind,
    pub(crate) generation: u64,
    pub(crate) flags: u16,
    pub(crate) payload: &'a [u8],
}

fn push_record(
    out: &mut Vec<u8>,
    kind: RecordKind,
    generation: u64,
    payload: &[u8],
) -> PactResult<()> {
    push_record_with_flags(out, kind, generation, 0, payload)
}

pub(crate) fn push_record_with_flags(
    out: &mut Vec<u8>,
    kind: RecordKind,
    generation: u64,
    flags: u16,
    payload: &[u8],
) -> PactResult<()> {
    let payload_len = u64::try_from(payload.len())
        .map_err(|_| PactError::codec("Pact record payload length overflow"))?;
    if payload_len > MAX_RECORD_PAYLOAD_BYTES {
        return Err(PactError::invalid(format!(
            "Pact record payload exceeds {MAX_RECORD_PAYLOAD_BYTES} bytes"
        )));
    }
    out.extend_from_slice(RECORD_MAGIC);
    out.push(kind as u8);
    out.push(RECORD_VERSION);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&blake3::hash(payload).as_bytes()[..8]);
    out.extend_from_slice(payload);
    Ok(())
}

pub(crate) fn captured_content_payload(content: &ContentRecord) -> PactResult<(&[u8], bool)> {
    if let Some(bytes) = &content.storage.inline_bytes {
        return Ok((bytes, true));
    }
    if let Some(text) = &content.storage.inline {
        return Ok((text.as_bytes(), false));
    }
    Err(PactError::unsupported(format!(
        "candidate content {} has no captured payload bytes",
        content.id
    )))
}

pub(crate) fn content_chunks(payload: &[u8], body_is_bytes: bool) -> Vec<&[u8]> {
    const BYTE_CHUNK: usize = 4096;
    const LINES_PER_CHUNK: usize = 32;
    if !body_is_bytes
        && let Ok(text) = std::str::from_utf8(payload)
        && text.contains('\n')
    {
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut lines = 0;
        for (index, byte) in payload.iter().enumerate() {
            if *byte == b'\n' {
                lines += 1;
                if lines == LINES_PER_CHUNK {
                    chunks.push(&payload[start..=index]);
                    start = index + 1;
                    lines = 0;
                }
            }
        }
        if start < payload.len() {
            chunks.push(&payload[start..]);
        }
        if !chunks.is_empty() {
            return chunks;
        }
    }
    payload.chunks(BYTE_CHUNK).collect()
}

fn compressed_payload(payload: &[u8]) -> PactResult<Vec<u8>> {
    let compressed = zstd::bulk::compress(payload, 3)
        .map_err(|error| PactError::codec(format!("compress Pact semantic record: {error}")))?;
    if compressed.len().saturating_add(4) >= payload.len() {
        return Ok(payload.to_vec());
    }
    let mut stored = Vec::with_capacity(4 + compressed.len());
    stored.extend_from_slice(b"PZS2");
    stored.extend_from_slice(&compressed);
    Ok(stored)
}

pub(crate) fn decompressed_payload(payload: &[u8]) -> PactResult<Vec<u8>> {
    let Some(compressed) = payload.strip_prefix(b"PZS2") else {
        return Ok(payload.to_vec());
    };
    let decode_limit = compressed
        .len()
        .saturating_mul(MAX_SEMANTIC_EXPANSION_RATIO)
        .saturating_add(SEMANTIC_DECODE_SLACK_BYTES)
        .min(MAX_RECORD_PAYLOAD_BYTES as usize);
    zstd::bulk::decompress(compressed, decode_limit)
        .map_err(|error| PactError::corrupt(format!("invalid Pact semantic record: {error}")))
}

pub(crate) fn encoded_content_body_payload(payload: &[u8]) -> PactResult<(bool, Vec<u8>)> {
    let compressed = zstd::bulk::compress(payload, 3)
        .map_err(|error| PactError::codec(format!("compress Pact content body: {error}")))?;
    if compressed.len().saturating_add(8) >= payload.len() {
        return Ok((false, payload.to_vec()));
    }
    let mut stored = Vec::with_capacity(8 + compressed.len());
    stored.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    stored.extend_from_slice(&compressed);
    Ok((true, stored))
}

fn decoded_content_body_payload(payload: &[u8], compressed: bool) -> PactResult<Vec<u8>> {
    if !compressed {
        return Ok(payload.to_vec());
    }
    let expected_len = payload
        .get(..8)
        .ok_or_else(|| PactError::corrupt("compressed Pact content body is truncated"))?;
    let expected_len = u64::from_le_bytes(
        expected_len
            .try_into()
            .map_err(|_| PactError::corrupt("compressed Pact content body length is invalid"))?,
    );
    if expected_len > MAX_RECORD_PAYLOAD_BYTES {
        return Err(PactError::corrupt(
            "decoded Pact content body exceeds record limit",
        ));
    }
    let decoded = zstd::bulk::decompress(
        &payload[8..],
        usize::try_from(expected_len)
            .map_err(|_| PactError::corrupt("Pact content body exceeds platform limits"))?,
    )
    .map_err(|error| {
        PactError::corrupt(format!("invalid compressed Pact content body: {error}"))
    })?;
    if decoded.len() as u64 != expected_len {
        return Err(PactError::corrupt(
            "decoded Pact content body length does not match its header",
        ));
    }
    Ok(decoded)
}

pub(crate) fn decode_u64_value(bytes: &[u8], label: &str) -> PactResult<u64> {
    let raw: [u8; 8] = bytes
        .try_into()
        .map_err(|_| PactError::corrupt(format!("{label} has invalid width")))?;
    Ok(u64::from_le_bytes(raw))
}

pub(crate) fn encode_index_page_batch_record(
    generation: u64,
    pages: &[[u8; super::btree::PAGE_BYTES]],
) -> PactResult<Vec<u8>> {
    if pages.is_empty() || pages.len() > MAX_INDEX_BATCH_PAGES {
        return Err(PactError::invalid(format!(
            "Pact index batch must contain 1..={MAX_INDEX_BATCH_PAGES} pages"
        )));
    }
    let raw_len = pages
        .len()
        .checked_mul(super::btree::PAGE_BYTES)
        .ok_or_else(|| PactError::invalid("Pact index batch length overflow"))?;
    let mut raw = Vec::with_capacity(raw_len);
    for page in pages {
        raw.extend_from_slice(page);
    }
    let compression_level = if raw.len() >= 64 * 1024 {
        COLD_COMPRESSION_LEVEL
    } else {
        7
    };
    let compressed = zstd::bulk::compress(&raw, compression_level)
        .map_err(|error| PactError::codec(format!("compress Pact B+tree batch: {error}")))?;
    let mut payload = Vec::with_capacity(INDEX_BATCH_HEADER_BYTES + compressed.len());
    payload.extend_from_slice(INDEX_BATCH_MAGIC);
    payload.extend_from_slice(
        &u16::try_from(pages.len())
            .map_err(|_| PactError::invalid("Pact index page count overflow"))?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&0_u16.to_le_bytes());
    payload.extend_from_slice(
        &u32::try_from(raw_len)
            .map_err(|_| PactError::invalid("Pact index batch length overflow"))?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&compressed);
    let mut record = Vec::with_capacity(RECORD_HEADER_BYTES + payload.len());
    push_record(&mut record, RecordKind::IndexPage, generation, &payload)?;
    Ok(record)
}

pub(crate) fn encode_component_payload_batch_record(
    generation: u64,
    entries: &[(u32, Vec<u8>)],
) -> PactResult<Vec<u8>> {
    if entries.is_empty() {
        return Err(PactError::invalid(
            "Pact component payload batch cannot be empty",
        ));
    }
    let mut raw = Vec::new();
    for (ordinal, payload) in entries {
        raw.extend_from_slice(&ordinal.to_le_bytes());
        raw.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| PactError::invalid("Pact component payload exceeds u32"))?
                .to_le_bytes(),
        );
        raw.extend_from_slice(payload);
    }
    if raw.len() > MAX_RECORD_PAYLOAD_BYTES as usize {
        return Err(PactError::invalid(
            "Pact component payload batch exceeds record limit",
        ));
    }
    let compressed = zstd::bulk::compress(&raw, COLD_COMPRESSION_LEVEL)
        .map_err(|error| PactError::codec(format!("compress Pact component batch: {error}")))?;
    let mut payload = Vec::with_capacity(COMPONENT_BATCH_HEADER_BYTES + compressed.len());
    payload.extend_from_slice(COMPONENT_BATCH_MAGIC);
    payload.extend_from_slice(
        &u32::try_from(entries.len())
            .map_err(|_| PactError::invalid("Pact component count exceeds u32"))?
            .to_le_bytes(),
    );
    payload.extend_from_slice(
        &u32::try_from(raw.len())
            .map_err(|_| PactError::invalid("Pact component batch length exceeds u32"))?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&compressed);
    let mut record = Vec::with_capacity(RECORD_HEADER_BYTES + payload.len());
    push_record(
        &mut record,
        RecordKind::ComponentPayloadBatch,
        generation,
        &payload,
    )?;
    Ok(record)
}

pub(crate) fn encode_content_batch_record(
    generation: u64,
    records: &[ContentRecord],
) -> PactResult<Vec<u8>> {
    if records.is_empty() {
        return Err(PactError::invalid("Pact content batch cannot be empty"));
    }
    let raw = serde_json::to_vec(records)
        .map_err(|error| PactError::codec(format!("encode Pact content batch: {error}")))?;
    if raw.len() > MAX_RECORD_PAYLOAD_BYTES as usize {
        return Err(PactError::invalid(
            "Pact content batch exceeds record limit",
        ));
    }
    let compressed = zstd::bulk::compress(&raw, COLD_CONTENT_COMPRESSION_LEVEL)
        .map_err(|error| PactError::codec(format!("compress Pact content batch: {error}")))?;
    let mut payload = Vec::with_capacity(CONTENT_BATCH_HEADER_BYTES + compressed.len());
    payload.extend_from_slice(CONTENT_BATCH_MAGIC);
    payload.extend_from_slice(
        &u32::try_from(records.len())
            .map_err(|_| PactError::invalid("Pact content count exceeds u32"))?
            .to_le_bytes(),
    );
    payload.extend_from_slice(
        &u32::try_from(raw.len())
            .map_err(|_| PactError::invalid("Pact content batch length exceeds u32"))?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&compressed);
    let mut record = Vec::with_capacity(RECORD_HEADER_BYTES + payload.len());
    push_record(&mut record, RecordKind::ContentBatch, generation, &payload)?;
    Ok(record)
}

pub(crate) fn decode_content_batch_record_at(
    file: &[u8],
    address: u64,
    content_id: &str,
) -> PactResult<ContentRecord> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact content batch address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::ContentBatch {
        return Err(PactError::corrupt(
            "Pact content batch address references another record kind",
        ));
    }
    decode_content_batch_payload(record.payload)?
        .into_iter()
        .find(|record| record.id == content_id)
        .ok_or_else(|| {
            PactError::corrupt(format!(
                "Pact content batch does not contain content id {content_id}"
            ))
        })
}

fn decode_content_batch_payload(payload: &[u8]) -> PactResult<Vec<ContentRecord>> {
    if payload.len() <= CONTENT_BATCH_HEADER_BYTES || payload.get(..4) != Some(CONTENT_BATCH_MAGIC)
    {
        return Err(PactError::corrupt("invalid Pact content batch header"));
    }
    let count = usize::try_from(read_u32(payload, 4)?)
        .map_err(|_| PactError::corrupt("Pact content count exceeds platform limits"))?;
    let raw_len = usize::try_from(read_u32(payload, 8)?)
        .map_err(|_| PactError::corrupt("Pact content batch exceeds platform limits"))?;
    if count == 0 || raw_len > MAX_RECORD_PAYLOAD_BYTES as usize {
        return Err(PactError::corrupt("invalid Pact content batch limits"));
    }
    let raw = zstd::bulk::decompress(&payload[CONTENT_BATCH_HEADER_BYTES..], raw_len)
        .map_err(|error| PactError::corrupt(format!("invalid Pact content batch: {error}")))?;
    if raw.len() != raw_len {
        return Err(PactError::corrupt(
            "decoded Pact content batch length does not match its header",
        ));
    }
    let records: Vec<ContentRecord> = serde_json::from_slice(&raw)
        .map_err(|error| PactError::corrupt(format!("invalid Pact content records: {error}")))?;
    if records.len() != count {
        return Err(PactError::corrupt(
            "Pact content batch count does not match its header",
        ));
    }
    Ok(records)
}

pub(crate) fn decode_component_payload_batch_record(
    file: &[u8],
    address: u64,
    ordinal: u32,
) -> PactResult<Vec<u8>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact component batch address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::ComponentPayloadBatch {
        return Err(PactError::corrupt(
            "Pact component batch address references another record kind",
        ));
    }
    decode_component_payloads(record.payload)?
        .remove(&ordinal)
        .ok_or_else(|| {
            PactError::corrupt(format!(
                "Pact component payload batch has no ordinal {ordinal}"
            ))
        })
}

pub(crate) fn decode_component_payload_batch_record_all(
    file: &[u8],
    address: u64,
) -> PactResult<BTreeMap<u32, Vec<u8>>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact component batch address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::ComponentPayloadBatch {
        return Err(PactError::corrupt(
            "Pact component batch address references another record kind",
        ));
    }
    decode_component_payloads(record.payload)
}

pub(crate) fn decode_component_payload_batch_record_region(
    region: &[u8],
    ordinal: u32,
) -> PactResult<Vec<u8>> {
    decode_component_payload_batch_record(region, 0, ordinal)
}

fn decode_component_payload(payload: &[u8], ordinal: u32) -> PactResult<Vec<u8>> {
    decode_component_payloads(payload)?
        .remove(&ordinal)
        .ok_or_else(|| {
            PactError::corrupt(format!(
                "Pact component payload batch has no ordinal {ordinal}"
            ))
        })
}

fn decode_component_payloads(payload: &[u8]) -> PactResult<BTreeMap<u32, Vec<u8>>> {
    let (count, raw_len, compressed) = component_batch_metadata(payload)?;
    let raw = zstd::bulk::decompress(compressed, raw_len).map_err(|error| {
        PactError::corrupt(format!("invalid compressed Pact component batch: {error}"))
    })?;
    if raw.len() != raw_len {
        return Err(PactError::corrupt(
            "decoded Pact component batch length does not match its header",
        ));
    }
    let mut cursor = 0;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let stored_ordinal = read_u32(&raw, cursor)?;
        cursor = cursor
            .checked_add(4)
            .ok_or_else(|| PactError::corrupt("Pact component batch cursor overflow"))?;
        let len = usize::try_from(read_u32(&raw, cursor)?)
            .map_err(|_| PactError::corrupt("Pact component payload exceeds platform limits"))?;
        cursor = cursor
            .checked_add(4)
            .ok_or_else(|| PactError::corrupt("Pact component batch cursor overflow"))?;
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= raw.len())
            .ok_or_else(|| PactError::corrupt("Pact component payload overruns its batch"))?;
        if entries
            .insert(stored_ordinal, raw[cursor..end].to_vec())
            .is_some()
        {
            return Err(PactError::corrupt(
                "Pact component payload batch repeats an ordinal",
            ));
        }
        cursor = end;
    }
    if cursor != raw.len() {
        return Err(PactError::corrupt(
            "Pact component payload batch has trailing bytes",
        ));
    }
    Ok(entries)
}

fn component_batch_metadata(payload: &[u8]) -> PactResult<(usize, usize, &[u8])> {
    if payload.len() <= COMPONENT_BATCH_HEADER_BYTES
        || payload.get(..4) != Some(COMPONENT_BATCH_MAGIC)
    {
        return Err(PactError::corrupt("invalid Pact component batch header"));
    }
    let count = usize::try_from(read_u32(payload, 4)?)
        .map_err(|_| PactError::corrupt("Pact component count exceeds platform limits"))?;
    let raw_len = usize::try_from(read_u32(payload, 8)?)
        .map_err(|_| PactError::corrupt("Pact component batch exceeds platform limits"))?;
    if count == 0 || raw_len > MAX_RECORD_PAYLOAD_BYTES as usize {
        return Err(PactError::corrupt("invalid Pact component batch limits"));
    }
    Ok((count, raw_len, &payload[COMPONENT_BATCH_HEADER_BYTES..]))
}

pub(crate) fn component_payload_batch_record_len(header: &[u8]) -> PactResult<u64> {
    if header.len() != RECORD_HEADER_BYTES
        || &header[..4] != RECORD_MAGIC
        || header[4] != RecordKind::ComponentPayloadBatch as u8
    {
        return Err(PactError::corrupt(
            "invalid Pact component payload batch record header",
        ));
    }
    let payload_len = read_u64(header, 16)?;
    if payload_len <= COMPONENT_BATCH_HEADER_BYTES as u64 || payload_len > MAX_RECORD_PAYLOAD_BYTES
    {
        return Err(PactError::corrupt(
            "invalid Pact component payload batch length",
        ));
    }
    (RECORD_HEADER_BYTES as u64)
        .checked_add(payload_len)
        .ok_or_else(|| PactError::corrupt("Pact component batch record length overflow"))
}

pub(crate) fn pack_index_page_address(batch_address: u64, ordinal: usize) -> PactResult<u64> {
    if batch_address >= (1_u64 << (64 - INDEX_PAGE_ORDINAL_BITS)) {
        return Err(PactError::invalid(
            "Pact index batch address exceeds 56 bits",
        ));
    }
    if ordinal >= MAX_INDEX_BATCH_PAGES {
        return Err(PactError::invalid(format!(
            "Pact index page ordinal exceeds {INDEX_PAGE_ORDINAL_BITS} bits"
        )));
    }
    Ok((batch_address << INDEX_PAGE_ORDINAL_BITS) | ordinal as u64)
}

pub(crate) fn index_page_batch_address(address: u64) -> u64 {
    address >> INDEX_PAGE_ORDINAL_BITS
}

pub(crate) fn index_page_ordinal(address: u64) -> usize {
    address as usize & (MAX_INDEX_BATCH_PAGES - 1)
}

pub(crate) fn decode_index_page_record(
    file: &[u8],
    address: u64,
) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
    let batch_address = index_page_batch_address(address);
    let pages = decode_index_page_batch_record(file, batch_address)?;
    pages
        .get(index_page_ordinal(address))
        .copied()
        .ok_or_else(|| PactError::corrupt("Pact index page ordinal is out of range"))
}

pub(crate) fn decode_index_page_batch_record(
    file: &[u8],
    batch_address: u64,
) -> PactResult<Vec<[u8; super::btree::PAGE_BYTES]>> {
    let mut position = usize::try_from(batch_address)
        .map_err(|_| PactError::corrupt("Pact index page address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::IndexPage {
        return Err(PactError::corrupt(
            "Pact B+tree address does not reference an index batch",
        ));
    }
    decode_index_batch_pages(record.payload)
}

pub(crate) fn decode_index_page_record_region(
    region: &[u8],
    address: u64,
) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
    let pages = decode_index_page_batch_record_region(region)?;
    pages
        .get(index_page_ordinal(address))
        .copied()
        .ok_or_else(|| PactError::corrupt("Pact index page ordinal is out of range"))
}

pub(crate) fn decode_index_page_batch_record_region(
    region: &[u8],
) -> PactResult<Vec<[u8; super::btree::PAGE_BYTES]>> {
    let mut position = 0;
    let record = read_record(region, &mut position)?;
    if record.kind != RecordKind::IndexPage {
        return Err(PactError::corrupt(
            "Pact B+tree address does not reference an index batch",
        ));
    }
    if position != region.len() {
        return Err(PactError::corrupt(
            "Pact index batch region has trailing bytes",
        ));
    }
    decode_index_batch_pages(record.payload)
}

pub(crate) fn content_body_record_len(header: &[u8]) -> PactResult<u64> {
    if header.len() != RECORD_HEADER_BYTES || &header[..4] != RECORD_MAGIC {
        return Err(PactError::corrupt(
            "invalid Pact content body record header",
        ));
    }
    let payload_len = read_u64(header, 16)?;
    if payload_len > MAX_RECORD_PAYLOAD_BYTES {
        return Err(PactError::corrupt("Pact content body exceeds record limit"));
    }
    (RECORD_HEADER_BYTES as u64)
        .checked_add(payload_len)
        .ok_or_else(|| PactError::corrupt("Pact content body record length overflow"))
}

pub(crate) fn index_page_record_len(header: &[u8]) -> PactResult<u64> {
    if header.len() != RECORD_HEADER_BYTES
        || &header[..4] != RECORD_MAGIC
        || header[4] != RecordKind::IndexPage as u8
    {
        return Err(PactError::corrupt("invalid Pact index page record header"));
    }
    let payload_len = read_u64(header, 16)?;
    if payload_len < INDEX_BATCH_HEADER_BYTES as u64 || payload_len > MAX_RECORD_PAYLOAD_BYTES {
        return Err(PactError::corrupt(
            "invalid Pact index batch payload length",
        ));
    }
    (RECORD_HEADER_BYTES as u64)
        .checked_add(payload_len)
        .ok_or_else(|| PactError::corrupt("Pact index page record length overflow"))
}

pub(crate) fn index_page_batch_page_count(payload: &[u8]) -> PactResult<usize> {
    index_batch_metadata(payload).map(|(count, _, _)| count)
}

fn index_batch_metadata(payload: &[u8]) -> PactResult<(usize, usize, &[u8])> {
    if payload.len() <= INDEX_BATCH_HEADER_BYTES || &payload[..4] != INDEX_BATCH_MAGIC {
        return Err(PactError::corrupt("invalid Pact index batch header"));
    }
    let count = usize::from(u16::from_le_bytes([payload[4], payload[5]]));
    if count == 0 || count > MAX_INDEX_BATCH_PAGES || payload[6..8] != [0, 0] {
        return Err(PactError::corrupt("invalid Pact index batch page count"));
    }
    let raw_len = usize::try_from(u32::from_le_bytes([
        payload[8],
        payload[9],
        payload[10],
        payload[11],
    ]))
    .map_err(|_| PactError::corrupt("Pact index batch length exceeds platform limits"))?;
    if raw_len != count.saturating_mul(super::btree::PAGE_BYTES) {
        return Err(PactError::corrupt("invalid Pact index batch raw length"));
    }
    Ok((count, raw_len, &payload[INDEX_BATCH_HEADER_BYTES..]))
}

fn decode_index_batch_pages(payload: &[u8]) -> PactResult<Vec<[u8; super::btree::PAGE_BYTES]>> {
    let (_, raw_len, compressed) = index_batch_metadata(payload)?;
    let raw = zstd::bulk::decompress(compressed, raw_len)
        .map_err(|error| PactError::corrupt(format!("invalid Pact index batch: {error}")))?;
    if raw.len() != raw_len {
        return Err(PactError::corrupt(
            "invalid decompressed Pact index batch length",
        ));
    }
    raw.chunks_exact(super::btree::PAGE_BYTES)
        .map(|page| {
            page.try_into()
                .map_err(|_| PactError::corrupt("invalid Pact index page width"))
        })
        .collect()
}

pub(crate) fn decode_content_body_record(file: &[u8], address: u64) -> PactResult<Vec<u8>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact content body address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::ContentBody {
        return Err(PactError::corrupt(
            "Pact content-body address does not reference a body record",
        ));
    }
    if record.flags & !RECORD_FLAG_ZSTD != 0 {
        return Err(PactError::corrupt(
            "Pact content-body record has unsupported flags",
        ));
    }
    decoded_content_body_payload(record.payload, record.flags & RECORD_FLAG_ZSTD != 0)
}

pub(crate) fn content_body_decoded_len(file: &[u8], address: u64) -> PactResult<usize> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact content body address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::ContentBody {
        return Err(PactError::corrupt(
            "Pact content-body address does not reference a body record",
        ));
    }
    if record.flags & !RECORD_FLAG_ZSTD != 0 {
        return Err(PactError::corrupt(
            "Pact content-body record has unsupported flags",
        ));
    }
    if record.flags & RECORD_FLAG_ZSTD == 0 {
        return Ok(record.payload.len());
    }
    let raw = record
        .payload
        .get(..8)
        .ok_or_else(|| PactError::corrupt("compressed Pact content body is truncated"))?;
    usize::try_from(u64::from_le_bytes(raw.try_into().map_err(|_| {
        PactError::corrupt("compressed Pact content body length is invalid")
    })?))
    .map_err(|_| PactError::corrupt("Pact content body exceeds platform limits"))
}

pub(crate) fn decode_content_range_descriptor_at(
    file: &[u8],
    address: u64,
    content_id: &str,
) -> PactResult<Option<ContentRangeDescriptor>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact content locator exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::Content {
        return Ok(None);
    }
    let manifest = serde_json::from_slice::<ContentManifest>(record.payload)
        .map_err(|error| PactError::corrupt(format!("invalid Pact content manifest: {error}")))?;
    if manifest.record.id != content_id {
        return Err(PactError::corrupt(
            "Pact content locator references the wrong content id",
        ));
    }
    Ok(Some(ContentRangeDescriptor {
        record: manifest.record,
        body_addresses: manifest.body_addresses,
        body_offsets: manifest.body_offsets,
        body_is_bytes: manifest.body_is_bytes,
    }))
}

pub(crate) fn decode_content_batch_records_at(
    file: &[u8],
    address: u64,
) -> PactResult<Option<Vec<ContentRecord>>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact content locator exceeds platform limits"))?;
    let first = read_record(file, &mut position)?;
    if first.kind != RecordKind::ContentBatch {
        return Ok(None);
    }
    decode_content_batch_payload(first.payload).map(Some)
}

pub(crate) fn decode_content_record_at(
    file: &[u8],
    address: u64,
    content_id: &str,
    expected_hash: &[u8; 32],
) -> PactResult<ContentRecord> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact content locator exceeds platform limits"))?;
    let first = read_record(file, &mut position)?;
    match first.kind {
        RecordKind::Content => {
            decode_content_manifest(first.payload, file, content_id, expected_hash)
        }
        RecordKind::ContentBatch => {
            let record = decode_content_batch_payload(first.payload)?
                .into_iter()
                .find(|record| record.id == content_id)
                .ok_or_else(|| {
                    PactError::corrupt("Pact content batch does not contain its indexed id")
                })?;
            let (payload, _) = captured_content_payload(&record)?;
            if blake3::hash(payload).as_bytes() != expected_hash {
                return Err(PactError::corrupt(
                    "Pact content id index disagrees with its batched payload",
                ));
            }
            Ok(record)
        }
        RecordKind::Transaction | RecordKind::CompactTransaction => {
            let mut cursor = position;
            loop {
                let record = read_record(file, &mut cursor)?;
                match record.kind {
                    RecordKind::Content => {
                        let manifest = serde_json::from_slice::<ContentManifest>(record.payload)
                            .map_err(|error| {
                                PactError::corrupt(format!(
                                    "invalid Pact content manifest: {error}"
                                ))
                            })?;
                        if manifest.record.id == content_id {
                            return hydrate_content_manifest(manifest, file, expected_hash);
                        }
                    }
                    RecordKind::Commit => {
                        return Err(PactError::corrupt(
                            "Pact content locator transaction does not contain its content id",
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => Err(PactError::corrupt(
            "Pact content locator does not reference a manifest or transaction record",
        )),
    }
}

fn decode_content_manifest(
    payload: &[u8],
    file: &[u8],
    content_id: &str,
    expected_hash: &[u8; 32],
) -> PactResult<ContentRecord> {
    let manifest = serde_json::from_slice::<ContentManifest>(payload)
        .map_err(|error| PactError::corrupt(format!("invalid Pact content manifest: {error}")))?;
    if manifest.record.id != content_id {
        return Err(PactError::corrupt(
            "Pact content locator references the wrong content id",
        ));
    }
    hydrate_content_manifest(manifest, file, expected_hash)
}

fn hydrate_content_manifest(
    mut manifest: ContentManifest,
    file: &[u8],
    expected_hash: &[u8; 32],
) -> PactResult<ContentRecord> {
    let body = manifest
        .body_addresses
        .iter()
        .map(|address| decode_content_body_record(file, *address))
        .collect::<PactResult<Vec<_>>>()?
        .concat();
    if blake3::hash(&body).as_bytes() != expected_hash {
        return Err(PactError::corrupt(
            "Pact content id index disagrees with its payload",
        ));
    }
    if manifest.body_is_bytes {
        manifest.record.storage.inline_bytes = Some(body);
    } else {
        manifest.record.storage.inline = Some(
            String::from_utf8(body)
                .map_err(|_| PactError::corrupt("Pact text content body is not UTF-8"))?,
        );
    }
    Ok(manifest.record)
}

pub(crate) fn decode_content_body_record_region(region: &[u8]) -> PactResult<Vec<u8>> {
    decode_content_body_record(region, 0)
}

pub(crate) fn decode_record_generation_at(file: &[u8], address: u64) -> PactResult<u64> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact record address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if !matches!(
        record.kind,
        RecordKind::Transaction | RecordKind::CompactTransaction
    ) {
        return Err(PactError::corrupt(
            "Pact revision address does not reference a transaction",
        ));
    }
    Ok(record.generation)
}

pub(crate) fn decode_any_record_generation_at(file: &[u8], address: u64) -> PactResult<u64> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact record address exceeds platform limits"))?;
    Ok(read_record(file, &mut position)?.generation)
}

pub(crate) fn measure_record_layout(
    file: &[u8],
) -> PactResult<super::layout_metrics::PhysicalLayoutMetrics> {
    let selected = super::root_slot::select_root_slot(file)?;
    let committed_len = usize::try_from(selected.slot.file_length_watermark)
        .map_err(|_| PactError::corrupt("Pact committed length exceeds platform limits"))?;
    if committed_len > file.len() {
        return Err(PactError::corrupt(
            "Pact committed length exceeds the available file length",
        ));
    }

    let mut metrics = super::layout_metrics::PhysicalLayoutMetrics {
        file_bytes: selected.slot.file_length_watermark,
        envelope_bytes: super::layout::HEADER_BYTES as u64,
        ..Default::default()
    };
    let committed = &file[..committed_len];
    let mut position = super::layout::HEADER_BYTES;
    while position < committed.len() {
        let record_start = position;
        let record = read_record(committed, &mut position)?;
        let record_bytes = u64::try_from(position - record_start)
            .map_err(|_| PactError::corrupt("Pact record length exceeds u64"))?;
        match record.kind {
            RecordKind::Transaction
            | RecordKind::CompactTransaction
            | RecordKind::Trace
            | RecordKind::PublicTrace
            | RecordKind::LedgerDelta
            | RecordKind::ComponentPayloadBatch => metrics.canonical_bytes += record_bytes,
            RecordKind::Checkpoint => metrics.checkpoint_bytes += record_bytes,
            RecordKind::Content | RecordKind::ContentBody | RecordKind::ContentBatch => {
                metrics.content_bytes += record_bytes
            }
            RecordKind::IndexPage | RecordKind::IndexDelta => metrics.facet_bytes += record_bytes,
            RecordKind::SegmentDirectory => metrics.directory_bytes += record_bytes,
            RecordKind::Allocator => metrics.allocator_bytes += record_bytes,
            RecordKind::Commit => metrics.commit_bytes += record_bytes,
        }
    }
    if position != committed.len() {
        return Err(PactError::corrupt(
            "Pact physical records do not cover the committed prefix",
        ));
    }
    let derived_bytes = metrics
        .checkpoint_bytes
        .saturating_add(metrics.facet_bytes)
        .saturating_add(metrics.directory_bytes)
        .saturating_add(metrics.allocator_bytes);
    metrics.derived_amplification = if metrics.canonical_bytes == 0 {
        0.0
    } else {
        derived_bytes as f64 / metrics.canonical_bytes as f64
    };
    if metrics.accounted_bytes() != metrics.file_bytes {
        return Err(PactError::corrupt(
            "Pact physical layout does not account for every committed byte",
        ));
    }
    Ok(metrics)
}

pub(crate) fn read_record<'a>(file: &'a [u8], position: &mut usize) -> PactResult<Record<'a>> {
    let header_end = position
        .checked_add(RECORD_HEADER_BYTES)
        .filter(|end| *end <= file.len())
        .ok_or_else(|| PactError::corrupt("truncated Pact record header"))?;
    let header = &file[*position..header_end];
    if &header[..4] != RECORD_MAGIC || header[5] != RECORD_VERSION {
        return Err(PactError::corrupt("invalid Pact record header"));
    }
    let kind = RecordKind::parse(header[4])?;
    let flags = u16::from_le_bytes([header[6], header[7]]);
    let generation = read_u64(header, 8)?;
    let payload_len = read_u64(header, 16)?;
    if payload_len > MAX_RECORD_PAYLOAD_BYTES {
        return Err(PactError::corrupt(format!(
            "Pact record payload exceeds {MAX_RECORD_PAYLOAD_BYTES} bytes"
        )));
    }
    let payload_len = usize::try_from(payload_len)
        .map_err(|_| PactError::corrupt("Pact record length exceeds platform limits"))?;
    let payload_end = header_end
        .checked_add(payload_len)
        .filter(|end| *end <= file.len())
        .ok_or_else(|| PactError::corrupt("Pact record payload overruns file"))?;
    let payload = &file[header_end..payload_end];
    if blake3::hash(payload).as_bytes()[..8] != header[24..32] {
        return Err(PactError::corrupt("Pact record payload checksum mismatch"));
    }
    *position = payload_end;
    Ok(Record {
        kind,
        generation,
        flags,
        payload,
    })
}

pub(crate) fn encode_commit_marker(marker: &CommitMarker) -> [u8; COMMIT_PAYLOAD_BYTES] {
    let mut bytes = [0_u8; COMMIT_PAYLOAD_BYTES];
    bytes[..8].copy_from_slice(&marker.generation.to_le_bytes());
    bytes[8..16].copy_from_slice(&marker.transaction_address.to_le_bytes());
    bytes[16..24].copy_from_slice(&marker.index_root_address.to_le_bytes());
    bytes[24..32].copy_from_slice(&marker.file_length_watermark.to_le_bytes());
    bytes[32..40].copy_from_slice(&marker.segment_directory_address.to_le_bytes());
    bytes[40..48].copy_from_slice(&marker.segment_directory_len.to_le_bytes());
    bytes[48..56].copy_from_slice(&marker.allocator_address.to_le_bytes());
    bytes[56..64].copy_from_slice(&marker.allocator_len.to_le_bytes());
    bytes[64..80].copy_from_slice(&marker.segment_directory_digest);
    bytes[80..96].copy_from_slice(&marker.allocator_digest);
    bytes[96..128].copy_from_slice(&marker.root_digest);
    bytes
}

pub(crate) fn decode_commit_marker(payload: &[u8]) -> PactResult<CommitMarker> {
    if payload.len() != COMMIT_PAYLOAD_BYTES {
        return Err(PactError::corrupt("invalid Pact commit marker length"));
    }
    let mut root_digest = [0_u8; 32];
    root_digest.copy_from_slice(&payload[96..128]);
    Ok(CommitMarker {
        generation: read_u64(payload, 0)?,
        transaction_address: read_u64(payload, 8)?,
        index_root_address: read_u64(payload, 16)?,
        file_length_watermark: read_u64(payload, 24)?,
        segment_directory_address: read_u64(payload, 32)?,
        segment_directory_len: read_u64(payload, 40)?,
        allocator_address: read_u64(payload, 48)?,
        allocator_len: read_u64(payload, 56)?,
        segment_directory_digest: payload[64..80]
            .try_into()
            .map_err(|_| PactError::corrupt("invalid Pact directory digest width"))?,
        allocator_digest: payload[80..96]
            .try_into()
            .map_err(|_| PactError::corrupt("invalid Pact allocator digest width"))?,
        root_digest,
    })
}

pub(crate) fn decode_commit_marker_at(
    file: &[u8],
    marker_address: u64,
    expected_generation: u64,
) -> PactResult<CommitMarker> {
    let mut position = usize::try_from(marker_address)
        .map_err(|_| PactError::corrupt("Pact commit marker address exceeds platform limits"))?;
    let record = read_record(file, &mut position)?;
    if record.kind != RecordKind::Commit || record.generation != expected_generation {
        return Err(PactError::corrupt(
            "Pact root slot does not reference its generation commit marker",
        ));
    }
    let marker = decode_commit_marker(record.payload)?;
    if marker.generation != expected_generation || marker.file_length_watermark != position as u64 {
        return Err(PactError::corrupt("Pact commit marker generation mismatch"));
    }
    Ok(marker)
}

pub(crate) fn decode_commit_marker_region(
    region: &[u8],
    marker_address: u64,
    expected_generation: u64,
) -> PactResult<CommitMarker> {
    let mut position = 0;
    let record = read_record(region, &mut position)?;
    if record.kind != RecordKind::Commit || record.generation != expected_generation {
        return Err(PactError::corrupt(
            "Pact root slot does not reference its generation commit marker",
        ));
    }
    let marker = decode_commit_marker(record.payload)?;
    if marker.generation != expected_generation
        || marker.file_length_watermark != marker_address.saturating_add(position as u64)
    {
        return Err(PactError::corrupt("Pact commit marker generation mismatch"));
    }
    Ok(marker)
}

fn write_string(out: &mut Vec<u8>, value: &str) -> PactResult<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| PactError::invalid("Pact transaction string exceeds u32 length"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_bytes(out: &mut Vec<u8>, value: &[u8]) -> PactResult<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| PactError::invalid("Pact transaction bytes exceed u32 length"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn read_bytes<'a>(bytes: &'a [u8], cursor: &mut usize) -> PactResult<&'a [u8]> {
    let len = usize::try_from(read_u32(bytes, *cursor)?)
        .map_err(|_| PactError::corrupt("Pact byte length exceeds platform limits"))?;
    *cursor += 4;
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| PactError::corrupt("Pact bytes overrun record"))?;
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}

pub(crate) fn read_string(bytes: &[u8], cursor: &mut usize) -> PactResult<String> {
    let len = usize::try_from(read_u32(bytes, *cursor)?)
        .map_err(|_| PactError::corrupt("Pact string length exceeds platform limits"))?;
    *cursor += 4;
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| PactError::corrupt("Pact string overruns record"))?;
    let value = std::str::from_utf8(&bytes[*cursor..end])
        .map_err(|_| PactError::corrupt("Pact transaction string is not UTF-8"))?
        .to_string();
    *cursor = end;
    Ok(value)
}

fn read_u32(bytes: &[u8], offset: usize) -> PactResult<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| PactError::corrupt("truncated Pact u32 field"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> PactResult<u64> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| PactError::corrupt("truncated Pact u64 field"))?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn encode_boundary(boundary: BoundaryKind) -> u8 {
    match boundary {
        BoundaryKind::Authored => 0,
        BoundaryKind::PreRequest => 1,
        BoundaryKind::PostRequestAlteration => 2,
        BoundaryKind::Request => 3,
        BoundaryKind::Response => 4,
        BoundaryKind::PostResponseAlteration => 5,
        BoundaryKind::PostResponse => 6,
        BoundaryKind::Adhoc => 7,
    }
}

fn decode_boundary(raw: u8) -> PactResult<BoundaryKind> {
    match raw {
        0 => Ok(BoundaryKind::Authored),
        1 => Ok(BoundaryKind::PreRequest),
        2 => Ok(BoundaryKind::PostRequestAlteration),
        3 => Ok(BoundaryKind::Request),
        4 => Ok(BoundaryKind::Response),
        5 => Ok(BoundaryKind::PostResponseAlteration),
        6 => Ok(BoundaryKind::PostResponse),
        7 => Ok(BoundaryKind::Adhoc),
        _ => Err(PactError::corrupt(format!(
            "unknown Pact transaction boundary: {raw}"
        ))),
    }
}
