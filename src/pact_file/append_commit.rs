use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::append_prepare::PreparedTransaction;
use super::append_state::AppendState;
use super::backend::{IoCounterSet, IoCounters};
use super::codec::{
    COMMIT_RECORD_BYTES, CommitMarker, ContentManifest, RECORD_FLAG_ZSTD, RECORD_HEADER_BYTES,
    RecordKind, captured_content_payload, content_chunks, decode_u64_value, encode_commit_marker,
    encoded_content_body_payload, push_record_with_flags,
};
use super::envelope::RootSlot;
use super::index::{AppendIndexInputs, IndexKey, stage_append_index};
use super::log::{FaultPoint, read_range, select_committed_slot_streamed, write_slot};
use super::root_slot::slot_index_for_next;
use super::snapshot::Snapshot;
use super::stream_encode::{RecordSpool, spool_standalone_core};
use super::transaction::PactTransaction;
use crate::error::{PactError, PactResult};
use crate::memory::PactMemoryOptions;
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;

#[cfg(test)]
mod tests;

pub(crate) struct WriterLock {
    _file: File,
    path: PathBuf,
}

impl WriterLock {
    pub(crate) fn lock_path(canonical: &Path) -> PathBuf {
        let mut name = canonical
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(".lock");
        canonical.with_file_name(name)
    }

    pub(crate) fn acquire(canonical: &Path) -> PactResult<Self> {
        let path = Self::lock_path(canonical);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(std::fs::TryLockError::WouldBlock) => Err(PactError::storage(format!(
                "another Pact writer holds {}",
                path.display()
            ))),
            Err(std::fs::TryLockError::Error(error)) => Err(PactError::Io(error)),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) fn platform_file_id(metadata: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        (0, 0)
    }
}

pub(crate) struct PinnedFile {
    pub(crate) snapshot: Snapshot,
    pub(crate) platform_id: (u64, u64),

    pub(crate) unexplained_tail: u64,
}

pub(crate) fn pin(
    canonical: &Path,
    counters: std::sync::Arc<IoCounterSet>,
    options: &PactMemoryOptions,
) -> PactResult<PinnedFile> {
    let snapshot = Snapshot::open_with_memory_options(canonical, counters, options)?;
    let metadata = std::fs::metadata(canonical)?;
    let platform_id = platform_file_id(&metadata);
    let unexplained_tail = metadata.len().saturating_sub(snapshot.committed_len());
    Ok(PinnedFile {
        snapshot,
        platform_id,
        unexplained_tail,
    })
}

pub(crate) fn tail_is_unpublished_candidate(
    file: &mut File,
    watermark: u64,
    generation: u64,
) -> PactResult<bool> {
    let len = file.metadata()?.len();
    if len <= watermark {
        return Ok(true);
    }
    let head_len = (len - watermark).min(RECORD_HEADER_BYTES as u64);
    if head_len < RECORD_HEADER_BYTES as u64 {
        return Ok(true);
    }
    let head = read_range(file, watermark, head_len)?;
    if &head[..4] != super::codec::RECORD_MAGIC {
        return Ok(false);
    }
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&head[8..16]);
    let record_generation = u64::from_le_bytes(raw);
    let kind = head[4];
    let opens_transaction =
        kind == RecordKind::Transaction as u8 || kind == RecordKind::CompactTransaction as u8;
    Ok(opens_transaction && record_generation == generation + 1)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublishReceipt {
    pub(crate) generation: u64,
    pub(crate) transaction_address: u64,
    pub(crate) index_root_address: u64,
    pub(crate) committed_len: u64,
    pub(crate) bytes_written: u64,
    pub(crate) recovered_tail_bytes: u64,
}

#[derive(Debug)]
pub(crate) enum PublishError {
    Rejected(PactError),

    Unknown(std::io::Error),
}

impl From<PactError> for PublishError {
    fn from(error: PactError) -> Self {
        Self::Rejected(error)
    }
}

impl From<std::io::Error> for PublishError {
    fn from(error: std::io::Error) -> Self {
        Self::Rejected(PactError::Io(error))
    }
}

struct HashingWriter<'a> {
    inner: &'a mut File,
    hasher: blake3::Hasher,
    written: u64,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn inject(configured: Option<FaultPoint>, point: FaultPoint) -> std::io::Result<()> {
    if configured == Some(point) {
        return Err(std::io::Error::other(format!(
            "injected candidate Pact fault after {point:?}"
        )));
    }
    Ok(())
}

struct LaidOut {
    record: super::stream_encode::SpooledRecord,
}

pub(crate) fn publish(
    canonical: &Path,
    pinned: &PinnedFile,
    state: &mut AppendState,
    prepared: &PreparedTransaction,
    options: &PactMemoryOptions,
    counters: &IoCounterSet,
    fault: Option<FaultPoint>,
) -> Result<PublishReceipt, PublishError> {
    let snapshot = &pinned.snapshot;
    state.verify_pinned(snapshot)?;
    if prepared.from_revision != state.head_revision {
        return Err(PactError::invalid(format!(
            "prepared transaction follows revision {} but the pinned head is {}",
            prepared.from_revision, state.head_revision
        ))
        .into());
    }
    let generation = snapshot
        .generation()
        .checked_add(1)
        .ok_or_else(|| PactError::unsupported("Pact generation exceeds the format limit"))?;
    let watermark = snapshot.committed_len();

    let mut file = OpenOptions::new().read(true).write(true).open(canonical)?;
    let metadata = file.metadata()?;
    if platform_file_id(&metadata) != pinned.platform_id {
        return Err(PactError::invalid(
            "Pact file was replaced under the writer (platform identity changed)",
        )
        .into());
    }
    let (selection, selection_io) =
        select_committed_slot_streamed(&mut file, options.io_chunk_bytes)?;
    if selection.slot.generation != snapshot.generation()
        || selection.slot.file_length_watermark != watermark
        || selection.slot.index_root_address != snapshot.index_root()
    {
        return Err(PactError::invalid(format!(
            "Pact file advanced under the writer: pinned generation {} watermark {}, committed generation {} watermark {}",
            snapshot.generation(),
            watermark,
            selection.slot.generation,
            selection.slot.file_length_watermark
        ))
        .into());
    }
    let header = read_range(&mut file, 0, super::layout::HEADER_BYTES as u64)?;
    let identity = super::envelope::file_identity(&header)?;
    if hex(&identity) != snapshot.source_file_identity() {
        return Err(
            PactError::invalid("Pact file logical identity changed under the writer").into(),
        );
    }
    let recovered_tail_bytes = metadata.len().saturating_sub(watermark);
    if recovered_tail_bytes > 0 {
        if !tail_is_unpublished_candidate(&mut file, watermark, snapshot.generation())? {
            return Err(PactError::corrupt(format!(
                "Pact file carries {recovered_tail_bytes} unexplained bytes past the committed watermark; recovery is required before appending"
            ))
            .into());
        }
        file.set_len(watermark)?;
    }

    let mut records = RecordSpool::create(options.io_chunk_bytes)?;
    let shell = PactTransaction::delta(
        prepared.transaction_id.clone(),
        state.branch_id.clone(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: prepared.from_revision,
            to_rev: prepared.to_revision,
            ops: Vec::new(),
        },
        prepared.public_trace.clone(),
        Vec::new(),
        prepared.delta.clone(),
    );
    let mut laid_out = Vec::new();
    let mut cursor = watermark;
    let core = spool_standalone_core(
        &shell,
        &prepared.trace,
        &mut records,
        generation,
        options.io_chunk_bytes,
    )?;
    cursor = cursor
        .checked_add(core.encoded_len())
        .ok_or_else(|| PactError::invalid("Pact transaction address overflow"))?;
    laid_out.push(LaidOut { record: core });

    let mut body_addresses: BTreeMap<[u8; 32], u64> = BTreeMap::new();
    let mut manifest_addresses: BTreeMap<String, u64> = BTreeMap::new();
    for content in &prepared.new_content {
        let (payload, body_is_bytes) = captured_content_payload(content)?;
        let mut addresses = Vec::new();
        let mut offsets = Vec::new();
        let mut offset = 0_u64;
        for chunk in content_chunks(payload, body_is_bytes) {
            offsets.push(offset);
            offset = offset.saturating_add(chunk.len() as u64);
            let chunk_hash = *blake3::hash(chunk).as_bytes();
            let existing = match body_addresses.get(&chunk_hash) {
                Some(address) => Some(*address),
                None => super::index::read_index_value_from_pages(
                    snapshot,
                    snapshot.index_root(),
                    &IndexKey::ContentHash { hash: chunk_hash },
                )?
                .map(|value| decode_u64_value(&value, "Pact content body address"))
                .transpose()?,
            };
            let address = match existing {
                Some(address) => address,
                None => {
                    let (compressed, stored) = encoded_content_body_payload(chunk)?;
                    let record = records.push(
                        RecordKind::ContentBody,
                        generation,
                        if compressed { RECORD_FLAG_ZSTD } else { 0 },
                        &stored,
                    )?;
                    let address = cursor;
                    cursor = cursor
                        .checked_add(record.encoded_len())
                        .ok_or_else(|| PactError::invalid("Pact content body address overflow"))?;
                    laid_out.push(LaidOut { record });
                    address
                }
            };
            body_addresses.insert(chunk_hash, address);
            addresses.push(address);
        }
        let mut stripped = content.clone();
        stripped.storage.inline = None;
        stripped.storage.inline_bytes = None;
        let manifest = serde_json::to_vec(&ContentManifest {
            record: stripped,
            body_addresses: addresses,
            body_offsets: offsets,
            body_is_bytes,
        })
        .map_err(|error| PactError::codec(format!("encode Pact content manifest: {error}")))?;
        let record = records.push(RecordKind::Content, generation, 0, &manifest)?;
        manifest_addresses.insert(content.id.clone(), cursor);
        cursor = cursor
            .checked_add(record.encoded_len())
            .ok_or_else(|| PactError::invalid("Pact content manifest address overflow"))?;
        laid_out.push(LaidOut { record });
    }

    let extensions = crate::extension::indexed_records_from_transaction(&shell)?;
    let ops = prepared
        .index_ops
        .iter()
        .map(|(_, op)| op.clone())
        .collect::<Vec<_>>();
    let index_start_address = cursor;
    let staged = stage_append_index(
        snapshot,
        AppendIndexInputs {
            branch_id: &state.branch_id,
            revision: prepared.to_revision,
            generation,
            transaction_address: watermark,
            index_start_address,
            existing_root: snapshot.index_root(),
            metadata: &prepared.metadata,
            ops: &ops,
            extensions: &extensions,
            text_changes: &prepared.text_changes,
            search_changes: &prepared.search_changes,
            new_content: &prepared.new_content,
            body_addresses: &body_addresses,
            manifest_addresses: &manifest_addresses,
            in_flight_bytes: options.in_flight_bytes,
            io_chunk_bytes: options.io_chunk_bytes,
            dangling_relations: &state.dangling_relations,
            order_repair: prepared.order_repair.as_ref(),
        },
    )?;

    file.seek(SeekFrom::Start(watermark))?;
    let mut writer = HashingWriter {
        inner: &mut file,
        hasher: blake3::Hasher::new(),
        written: 0,
    };
    for entry in &laid_out {
        records.emit(&entry.record, &mut writer)?;
    }
    if writer.written != index_start_address - watermark {
        return Err(PactError::internal(
            "Pact candidate length prediction mismatch before the index",
        )
        .into());
    }
    let (index_bytes, index_root_address) = staged.finish_into(&mut writer)?;
    let mut cursor = index_start_address
        .checked_add(index_bytes)
        .ok_or_else(|| PactError::invalid("Pact index address overflow"))?;
    let index_delta = records.push(
        RecordKind::IndexDelta,
        generation,
        0,
        &index_root_address.to_le_bytes(),
    )?;
    records.emit(&index_delta, &mut writer)?;
    cursor = cursor
        .checked_add(index_delta.encoded_len())
        .ok_or_else(|| PactError::invalid("Pact index delta address overflow"))?;

    let segment_directory_address = cursor;
    let mut directory = Vec::with_capacity(RECORD_HEADER_BYTES + 24);
    let mut directory_payload = Vec::with_capacity(24);
    directory_payload.extend_from_slice(&generation.to_le_bytes());
    directory_payload.extend_from_slice(&watermark.to_le_bytes());
    directory_payload.extend_from_slice(&index_root_address.to_le_bytes());
    push_record_with_flags(
        &mut directory,
        RecordKind::SegmentDirectory,
        generation,
        0,
        &directory_payload,
    )?;
    let segment_directory_len = directory.len() as u64;
    let mut segment_directory_digest = [0_u8; 16];
    segment_directory_digest.copy_from_slice(&blake3::hash(&directory).as_bytes()[..16]);
    cursor = cursor
        .checked_add(segment_directory_len)
        .ok_or_else(|| PactError::invalid("Pact segment directory address overflow"))?;

    let allocator_address = cursor;
    let allocator_record_len = (RECORD_HEADER_BYTES + 16) as u64;
    let committed_len = allocator_address
        .checked_add(allocator_record_len)
        .and_then(|address| address.checked_add(COMMIT_RECORD_BYTES))
        .ok_or_else(|| PactError::invalid("Pact committed length overflow"))?;
    let mut allocator = Vec::with_capacity(RECORD_HEADER_BYTES + 16);
    let mut allocator_payload = Vec::with_capacity(16);
    allocator_payload.extend_from_slice(&generation.to_le_bytes());
    allocator_payload.extend_from_slice(&committed_len.to_le_bytes());
    push_record_with_flags(
        &mut allocator,
        RecordKind::Allocator,
        generation,
        0,
        &allocator_payload,
    )?;
    if allocator.len() as u64 != allocator_record_len {
        return Err(
            PactError::internal("Pact allocator record length changed unexpectedly").into(),
        );
    }
    let mut allocator_digest = [0_u8; 16];
    allocator_digest.copy_from_slice(&blake3::hash(&allocator).as_bytes()[..16]);
    let commit_marker_address = allocator_address + allocator_record_len;

    writer.write_all(&directory)?;
    inject(fault, FaultPoint::DirectoryWritten)?;
    writer.write_all(&allocator)?;
    inject(fault, FaultPoint::AllocatorWritten)?;
    if writer.written != commit_marker_address - watermark {
        return Err(PactError::internal(
            "Pact candidate length prediction mismatch before the commit marker",
        )
        .into());
    }
    let root_digest = *writer.hasher.finalize().as_bytes();
    let marker = CommitMarker {
        generation,
        transaction_address: watermark,
        index_root_address,
        file_length_watermark: committed_len,
        segment_directory_address,
        segment_directory_len,
        allocator_address,
        allocator_len: allocator_record_len,
        segment_directory_digest,
        allocator_digest,
        root_digest,
    };
    let mut commit = Vec::with_capacity(COMMIT_RECORD_BYTES as usize);
    push_record_with_flags(
        &mut commit,
        RecordKind::Commit,
        generation,
        0,
        &encode_commit_marker(&marker),
    )?;
    file.write_all(&commit)?;
    inject(fault, FaultPoint::RecordsWritten)?;
    file.sync_data()?;
    inject(fault, FaultPoint::DataSynced)?;

    let slot_index = slot_index_for_next(Some(&selection));
    let slot = RootSlot {
        generation,
        committed_transaction_address: commit_marker_address,
        index_root_address,
        file_length_watermark: committed_len,
        segment_directory_address,
        segment_directory_len,
        allocator_address,
        allocator_len: allocator_record_len,
        segment_directory_digest,
        allocator_digest,
    };
    write_slot(&mut file, slot_index, &slot).map_err(|error| match error {
        PactError::Io(error) => PublishError::Unknown(error),
        other => PublishError::Rejected(other),
    })?;
    inject(fault, FaultPoint::RootSlotWritten).map_err(PublishError::Unknown)?;
    file.sync_all().map_err(PublishError::Unknown)?;
    inject(fault, FaultPoint::RootSynced).map_err(PublishError::Unknown)?;

    let bytes_written = committed_len - watermark + super::layout::ROOT_SLOT_BYTES as u64;
    counters.record(&IoCounters {
        bytes_read: selection_io.bytes_read,
        bytes_written,
        pages_written: 1,
        ..IoCounters::default()
    });
    state.advance(
        prepared.to_revision,
        generation,
        committed_len,
        index_root_address,
    );
    Ok(PublishReceipt {
        generation,
        transaction_address: watermark,
        index_root_address,
        committed_len,
        bytes_written,
        recovered_tail_bytes,
    })
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
