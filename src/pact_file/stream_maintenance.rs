use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::btree::{CowBTree, PAGE_BYTES, PageReader, page_references};
use super::codec::{
    COMMIT_RECORD_BYTES, CommitMarker, ContentManifest, RECORD_HEADER_BYTES, RecordKind,
    decode_u64_value, encode_commit_marker, push_record_with_flags, read_record,
};
use super::envelope::RootSlot;
use super::index::{
    IndexKey, MessageOrderRepairPlan, component_type, message_order_key,
    read_index_value_from_pages, stage_rebuilt_index,
};
use super::layout::HEADER_BYTES;
use super::log::write_slot;
use super::scratch::{ScratchPages, ScratchRange};
use super::snapshot::Snapshot;
use super::stream_decode::{PttrStream, StreamOpKind, WELL_KNOWN_SYMBOLS, transaction_trace_bytes};
use crate::error::{PactError, PactResult};
use crate::memory::PactMemoryOptions;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub(crate) struct MaintenancePolicy {
    pub(crate) max_derived_amplification_permille: u64,
    pub(crate) max_hot_segments: u64,
    pub(crate) max_dead_page_permille: u64,
}

impl Default for MaintenancePolicy {
    fn default() -> Self {
        Self {
            max_derived_amplification_permille: 1_250,
            max_hot_segments: 16,
            max_dead_page_permille: 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MaintenanceAssessment {
    pub(crate) generation: u64,
    pub(crate) file_bytes: u64,
    pub(crate) canonical_bytes: u64,
    pub(crate) facet_bytes: u64,
    pub(crate) derived_amplification_permille: u64,
    pub(crate) index_pages: u64,
    pub(crate) reachable_index_pages: u64,
    pub(crate) dead_page_permille: u64,
    pub(crate) hot_segments: u64,
    pub(crate) needed: bool,
    pub(crate) reasons: Vec<&'static str>,
}

const SCAN_PAGE_ENTRIES: usize = 256;

pub(crate) fn assess(
    snapshot: &Snapshot,
    options: &PactMemoryOptions,
    policy: &MaintenancePolicy,
) -> PactResult<MaintenanceAssessment> {
    let mapping = snapshot.mapping();
    let metrics = super::codec::measure_record_layout(mapping)?;
    let committed = usize::try_from(snapshot.committed_len())
        .map_err(|_| PactError::corrupt("Pact committed length exceeds platform limits"))?;
    let mut index_pages = 0_u64;
    let mut hot_segments = std::collections::BTreeSet::new();

    let mut live_roots = vec![snapshot.index_root()];
    let mut region_is_baseline = false;
    let mut position = HEADER_BYTES;
    while position < committed {
        let record = read_record(&mapping[..committed], &mut position)?;
        match record.kind {
            RecordKind::Transaction => {
                region_is_baseline = transaction_record_is_baseline(record.payload);
            }
            RecordKind::CompactTransaction => region_is_baseline = false,
            RecordKind::IndexPage => {
                index_pages += super::codec::index_page_batch_page_count(record.payload)? as u64;
                hot_segments.insert(record.generation);
            }
            RecordKind::IndexDelta => {
                if region_is_baseline && record.payload.len() == 8 {
                    let mut raw = [0_u8; 8];
                    raw.copy_from_slice(record.payload);
                    let root = u64::from_le_bytes(raw);
                    if root != snapshot.index_root() {
                        live_roots.push(root);
                    }
                }
            }
            _ => {}
        }
    }
    let reachable_index_pages = count_reachable_pages(snapshot, &live_roots, options)?;
    let dead_pages = index_pages.saturating_sub(reachable_index_pages);
    let dead_page_permille = if index_pages == 0 {
        0
    } else {
        dead_pages.saturating_mul(1000) / index_pages
    };
    let derived_amplification_permille = (metrics.derived_amplification * 1000.0) as u64;
    let mut reasons = Vec::new();

    if hot_segments.len() as u64 > policy.max_hot_segments {
        reasons.push("hot segments");
    }

    if derived_amplification_permille > policy.max_derived_amplification_permille
        && dead_page_permille > 0
    {
        reasons.push("derived amplification");
    }
    if dead_page_permille > policy.max_dead_page_permille {
        reasons.push("dead pages");
    }
    Ok(MaintenanceAssessment {
        generation: snapshot.generation(),
        file_bytes: metrics.file_bytes,
        canonical_bytes: metrics.canonical_bytes,
        facet_bytes: metrics.facet_bytes,
        derived_amplification_permille,
        index_pages,
        reachable_index_pages,
        dead_page_permille,
        hot_segments: hot_segments.len() as u64,
        needed: !reasons.is_empty(),
        reasons,
    })
}

fn count_reachable_pages(
    snapshot: &Snapshot,
    roots: &[u64],
    options: &PactMemoryOptions,
) -> PactResult<u64> {
    let mut scratch =
        ScratchPages::create(None, options.io_chunk_bytes, options.scratch_cache_bytes)?;
    let mut visited = CowBTree::from_root(None);
    let mut stack = roots.to_vec();
    let mut count = 0_u64;
    while let Some(address) = stack.pop() {
        let key = address.to_be_bytes().to_vec();
        if visited.get(&scratch, &key)?.is_some() {
            continue;
        }
        (visited, _) = visited.insert(&mut scratch, key, Vec::new())?;
        count += 1;
        let page = snapshot.read_page(address)?;
        for reference in page_references(&page)? {
            if visited.get(&scratch, &reference.to_be_bytes())?.is_none() {
                stack.push(reference);
            }
        }
    }
    Ok(count)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepackStage {
    RecordsCopied,

    CandidateComplete,

    BeforeReplace,

    Replaced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepackReport {
    pub(crate) generation: u64,
    pub(crate) bytes_before: u64,
    pub(crate) bytes_after: u64,
    pub(crate) transactions: u64,
    pub(crate) records_copied: u64,
    pub(crate) index_entries: u64,
    pub(crate) index_pages: u64,
}

const CANDIDATE_INFIX: &str = ".repack-";
const CANDIDATE_SUFFIX: &str = ".tmp";

fn candidate_path(canonical: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut name = canonical
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(format!(
        "{CANDIDATE_INFIX}{}-{nanos:x}{CANDIDATE_SUFFIX}",
        std::process::id()
    ));
    canonical.with_file_name(name)
}

pub(crate) fn remove_stale_candidates(canonical: &Path) -> PactResult<u64> {
    let Some(parent) = canonical.parent() else {
        return Ok(0);
    };
    let Some(base) = canonical.file_name().and_then(|name| name.to_str()) else {
        return Ok(0);
    };
    let prefix = format!("{base}{CANDIDATE_INFIX}");
    let mut removed = 0;
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(CANDIDATE_SUFFIX) {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

struct RegionWriter<'a> {
    inner: &'a mut File,
    hasher: blake3::Hasher,
    position: u64,
}

impl Write for RegionWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.position += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct SpoolSink<'a> {
    spool: &'a mut super::scratch::ScratchFile,
}

impl Write for SpoolSink<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.spool
            .append(buf)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct AddressMap {
    scratch: ScratchPages,
    tree: CowBTree,
}

impl AddressMap {
    fn new(options: &PactMemoryOptions) -> PactResult<Self> {
        Ok(Self {
            scratch: ScratchPages::create(
                None,
                options.io_chunk_bytes,
                options.scratch_cache_bytes,
            )?,
            tree: CowBTree::from_root(None),
        })
    }

    fn insert(&mut self, old: u64, new: u64) -> PactResult<()> {
        let (tree, _) = std::mem::replace(&mut self.tree, CowBTree::from_root(None)).insert(
            &mut self.scratch,
            old.to_be_bytes().to_vec(),
            new.to_le_bytes().to_vec(),
        )?;
        self.tree = tree;
        Ok(())
    }

    fn get(&self, old: u64) -> PactResult<u64> {
        let value = self
            .tree
            .get(&self.scratch, &old.to_be_bytes())?
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "Pact repack found no copied record at address {old}"
                ))
            })?;
        let raw: [u8; 8] = value
            .as_slice()
            .try_into()
            .map_err(|_| PactError::corrupt("Pact repack address map value has invalid width"))?;
        Ok(u64::from_le_bytes(raw))
    }
}

fn write_terminators(
    region: &mut RegionWriter<'_>,
    generation: u64,
    transaction_address: u64,
    index_root_address: u64,
) -> PactResult<(u64, RootSlot)> {
    let mut delta = Vec::new();
    push_record_with_flags(
        &mut delta,
        RecordKind::IndexDelta,
        generation,
        0,
        &index_root_address.to_le_bytes(),
    )?;
    region.write_all(&delta)?;

    let segment_directory_address = region.position;
    let mut directory = Vec::new();
    let mut directory_payload = Vec::with_capacity(24);
    directory_payload.extend_from_slice(&generation.to_le_bytes());
    directory_payload.extend_from_slice(&transaction_address.to_le_bytes());
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
    region.write_all(&directory)?;

    let allocator_address = region.position;
    let allocator_record_len = (RECORD_HEADER_BYTES + 16) as u64;
    let committed_len = allocator_address
        .checked_add(allocator_record_len)
        .and_then(|address| address.checked_add(COMMIT_RECORD_BYTES))
        .ok_or_else(|| PactError::invalid("Pact committed length overflow"))?;
    let mut allocator = Vec::new();
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
    let mut allocator_digest = [0_u8; 16];
    allocator_digest.copy_from_slice(&blake3::hash(&allocator).as_bytes()[..16]);
    region.write_all(&allocator)?;

    let commit_marker_address = region.position;
    let root_digest = *region.hasher.finalize().as_bytes();
    let marker = CommitMarker {
        generation,
        transaction_address,
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
    let mut commit = Vec::new();
    push_record_with_flags(
        &mut commit,
        RecordKind::Commit,
        generation,
        0,
        &encode_commit_marker(&marker),
    )?;
    region.inner.write_all(&commit)?;
    region.position += commit.len() as u64;
    if region.position != committed_len {
        return Err(PactError::internal(
            "Pact repack committed length prediction mismatch",
        ));
    }
    Ok((
        committed_len,
        RootSlot {
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
        },
    ))
}

const TERMINATOR_BYTES: u64 = (RECORD_HEADER_BYTES as u64 + 8)
    + (RECORD_HEADER_BYTES as u64 + 24)
    + (RECORD_HEADER_BYTES as u64 + 16)
    + COMMIT_RECORD_BYTES;

struct RegionWalk {
    records: u64,

    bytes: u64,

    old_index_root: Option<u64>,

    baseline: bool,
}

fn transaction_record_is_baseline(payload: &[u8]) -> bool {
    let mut cursor = 0;
    for _ in 0..2 {
        let Some(len) = payload
            .get(cursor..cursor + 4)
            .map(|raw| u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize)
        else {
            return false;
        };
        cursor += 4 + len;
    }
    payload.get(cursor + 1).copied() == Some(3)
}

fn walk_region(
    mapping: &[u8],
    region_start: u64,
    generation: u64,
    new_start: u64,
    map: &mut AddressMap,
    mut out: Option<&mut RegionWriter<'_>>,
) -> PactResult<RegionWalk> {
    let mut position = usize::try_from(region_start)
        .map_err(|_| PactError::corrupt("Pact transaction address exceeds platform limits"))?;
    let mut walk = RegionWalk {
        records: 0,
        bytes: 0,
        old_index_root: None,
        baseline: false,
    };
    let mut cursor = new_start;
    loop {
        let record_start = position;
        let record = read_record(mapping, &mut position)?;
        if record.generation != generation {
            return Err(PactError::corrupt(format!(
                "Pact repack found generation {} inside generation {generation}",
                record.generation
            )));
        }
        let encoded: std::borrow::Cow<'_, [u8]> = match record.kind {
            RecordKind::Commit => {
                walk.bytes = cursor - new_start;
                return Ok(walk);
            }
            RecordKind::IndexDelta => {
                if record.payload.len() == 8 {
                    let mut raw = [0_u8; 8];
                    raw.copy_from_slice(record.payload);
                    walk.old_index_root = Some(u64::from_le_bytes(raw));
                }
                continue;
            }
            RecordKind::IndexPage | RecordKind::SegmentDirectory | RecordKind::Allocator => {
                continue;
            }
            RecordKind::Transaction => {
                walk.baseline |= transaction_record_is_baseline(record.payload);
                std::borrow::Cow::Borrowed(&mapping[record_start..position])
            }
            RecordKind::Content => {
                let mut manifest: ContentManifest = serde_json::from_slice(record.payload)
                    .map_err(|error| {
                        PactError::corrupt(format!("invalid Pact content manifest: {error}"))
                    })?;
                for address in &mut manifest.body_addresses {
                    *address = map.get(*address)?;
                }
                let payload = serde_json::to_vec(&manifest).map_err(|error| {
                    PactError::codec(format!("encode Pact content manifest: {error}"))
                })?;
                let mut encoded = Vec::with_capacity(RECORD_HEADER_BYTES + payload.len());
                push_record_with_flags(
                    &mut encoded,
                    RecordKind::Content,
                    generation,
                    record.flags,
                    &payload,
                )?;
                std::borrow::Cow::Owned(encoded)
            }
            _ => std::borrow::Cow::Borrowed(&mapping[record_start..position]),
        };
        if out.is_none() {
            map.insert(record_start as u64, cursor)?;
        }
        if let Some(writer) = out.as_deref_mut() {
            if writer.position != cursor {
                return Err(PactError::internal("Pact repack lost its region position"));
            }
            writer.write_all(&encoded)?;
        }
        cursor = cursor
            .checked_add(encoded.len() as u64)
            .ok_or_else(|| PactError::invalid("Pact repack address overflow"))?;
        walk.records += 1;
    }
}

fn rebuild_index_to_spool(
    snapshot: &Snapshot,
    old_root: u64,
    index_start_address: u64,
    generation: u64,
    map: &AddressMap,
    options: &PactMemoryOptions,
) -> PactResult<(super::scratch::ScratchFile, u64, u64, u64, u64)> {
    let old_index = CowBTree::from_root(Some(old_root));
    let mut walker = ScratchRange::new(&old_index, &[], &[0xff], SCAN_PAGE_ENTRIES);
    let mut index_entries = 0_u64;
    let mut pending: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut failure: Option<PactError> = None;
    let entries = std::iter::from_fn(|| {
        if failure.is_some() {
            return None;
        }
        if pending.is_empty() {
            match walker.next_page(snapshot) {
                Ok(page) => {
                    pending = page;
                    pending.reverse();
                }
                Err(error) => {
                    failure = Some(error);
                    return None;
                }
            }
        }
        let (key, value) = pending.pop()?;
        index_entries += 1;
        Some(remap_index_value(&key, value, map).map(|value| (key, value)))
    });
    let staged = stage_rebuilt_index(
        entries,
        index_start_address,
        generation,
        options.in_flight_bytes,
        options.io_chunk_bytes,
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    let pages = staged.staged_page_count() as u64;
    let mut spool = super::scratch::ScratchFile::create(None, options.io_chunk_bytes)?;
    let (bytes, root) = {
        let mut sink = SpoolSink { spool: &mut spool };
        staged.finish_into(&mut sink)?
    };
    Ok((spool, bytes, root, index_entries, pages))
}

fn copy_spool(
    spool: &super::scratch::ScratchFile,
    bytes: u64,
    region: &mut RegionWriter<'_>,
    io_chunk_bytes: usize,
) -> PactResult<()> {
    let mut copied = 0_u64;
    let chunk_bytes = io_chunk_bytes.max(4096) as u64;
    while copied < bytes {
        let take = (bytes - copied).min(chunk_bytes);
        let chunk = spool.read_span(super::scratch::SpoolSpan {
            offset: copied,
            len: take,
        })?;
        region.write_all(&chunk)?;
        copied += take;
    }
    Ok(())
}

fn transaction_addresses(snapshot: &Snapshot) -> PactResult<Vec<(u64, u64)>> {
    let mut out = Vec::new();
    let mut next_generation = 1_u64;
    loop {
        let entries = snapshot.index_range(
            &IndexKey::Generation {
                generation: next_generation,
            },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            SCAN_PAGE_ENTRIES,
            false,
        )?;
        if entries.is_empty() {
            break;
        }
        let mut last_seen = next_generation;
        for (key, value) in &entries {
            let IndexKey::Generation { generation } = IndexKey::decode(key)? else {
                return Err(PactError::corrupt(
                    "Pact generation range returned another namespace",
                ));
            };
            let raw: [u8; 8] = value
                .as_slice()
                .try_into()
                .map_err(|_| PactError::corrupt("Pact transaction address has invalid width"))?;
            out.push((generation, u64::from_le_bytes(raw)));
            last_seen = generation;
        }
        if entries.len() < SCAN_PAGE_ENTRIES {
            break;
        }
        next_generation = last_seen
            .checked_add(1)
            .ok_or_else(|| PactError::corrupt("Pact generation overflow"))?;
    }
    Ok(out)
}

fn remap_index_value(key: &[u8], value: Vec<u8>, map: &AddressMap) -> PactResult<Vec<u8>> {
    let remap_u64 = |bytes: &[u8]| -> PactResult<Vec<u8>> {
        let raw: [u8; 8] = bytes
            .try_into()
            .map_err(|_| PactError::corrupt("Pact index address value has invalid width"))?;
        Ok(map.get(u64::from_le_bytes(raw))?.to_le_bytes().to_vec())
    };
    Ok(match IndexKey::decode(key)? {
        IndexKey::Revision { .. }
        | IndexKey::Generation { .. }
        | IndexKey::ContentHash { .. }
        | IndexKey::ComponentBatch { .. } => remap_u64(&value)?,
        IndexKey::ContentId { .. } if value.len() == 40 => {
            let mut remapped = value[..32].to_vec();
            remapped.extend_from_slice(&remap_u64(&value[32..])?);
            remapped
        }
        _ => value,
    })
}

pub(crate) fn repack(
    canonical: &Path,
    snapshot: &Snapshot,
    options: &PactMemoryOptions,
    observer: &mut dyn FnMut(RepackStage) -> PactResult<()>,
) -> PactResult<RepackReport> {
    let candidate = candidate_path(canonical);
    let result = repack_into(canonical, &candidate, snapshot, options, observer);
    if result.is_err() {
        let _ = std::fs::remove_file(&candidate);
    }
    result
}

fn repack_into(
    canonical: &Path,
    candidate: &Path,
    snapshot: &Snapshot,
    options: &PactMemoryOptions,
    observer: &mut dyn FnMut(RepackStage) -> PactResult<()>,
) -> PactResult<RepackReport> {
    options.validate()?;
    let mapping = snapshot.mapping();
    let committed = usize::try_from(snapshot.committed_len())
        .map_err(|_| PactError::corrupt("Pact committed length exceeds platform limits"))?;
    let mapping = &mapping[..committed];
    let header = &mapping[..HEADER_BYTES];
    let identity = super::envelope::file_identity(header)?;
    let source_metadata = std::fs::metadata(canonical)?;
    let source_id = super::append_commit::platform_file_id(&source_metadata);
    let generation = snapshot.generation();

    let transactions_in_order = transaction_addresses(snapshot)?;
    let &(last_generation, _) = transactions_in_order
        .last()
        .ok_or_else(|| PactError::corrupt("Pact repack found no committed transaction"))?;
    if last_generation != generation {
        return Err(PactError::corrupt(
            "Pact repack found no transaction for the committed generation",
        ));
    }
    let mut map = AddressMap::new(options)?;
    struct Planned {
        generation: u64,
        old_address: u64,
        new_start: u64,
        baseline_index: Option<(super::scratch::ScratchFile, u64, u64)>,
    }
    let mut planned: Vec<Planned> = Vec::new();
    let mut cursor = HEADER_BYTES as u64;
    let mut records_copied = 0_u64;
    let mut index_entries = 0_u64;
    let mut index_pages = 0_u64;
    for &(record_generation, old_address) in &transactions_in_order {
        let walk = walk_region(
            mapping,
            old_address,
            record_generation,
            cursor,
            &mut map,
            None,
        )?;
        records_copied += walk.records;
        let new_start = cursor;
        cursor = cursor
            .checked_add(walk.bytes)
            .ok_or_else(|| PactError::invalid("Pact repack address overflow"))?;
        let mut baseline_index = None;
        if record_generation != generation {
            if walk.baseline {
                let old_root = walk.old_index_root.ok_or_else(|| {
                    PactError::corrupt("Pact baseline transaction has no index delta")
                })?;
                let (spool, bytes, root, entries, pages) = rebuild_index_to_spool(
                    snapshot,
                    old_root,
                    cursor,
                    record_generation,
                    &map,
                    options,
                )?;
                index_entries += entries;
                index_pages += pages;
                cursor = cursor
                    .checked_add(bytes)
                    .ok_or_else(|| PactError::invalid("Pact repack address overflow"))?;
                baseline_index = Some((spool, bytes, root));
            }
            cursor = cursor
                .checked_add(TERMINATOR_BYTES)
                .ok_or_else(|| PactError::invalid("Pact repack address overflow"))?;
        }
        planned.push(Planned {
            generation: record_generation,
            old_address,
            new_start,
            baseline_index,
        });
    }
    let index_start_address = cursor;
    let transactions = planned.len() as u64;

    let (index_spool, index_bytes, index_root_address, entries, pages) = rebuild_index_to_spool(
        snapshot,
        snapshot.index_root(),
        index_start_address,
        generation,
        &map,
        options,
    )?;
    index_entries += entries;
    index_pages += pages;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(candidate)?;
    file.write_all(&super::root_slot::encode_empty_header(identity))?;
    let mut final_slot = None;
    let mut committed_len = 0;
    for plan in &planned {
        file.seek(SeekFrom::Start(plan.new_start))?;
        let mut region = RegionWriter {
            inner: &mut file,
            hasher: blake3::Hasher::new(),
            position: plan.new_start,
        };
        walk_region(
            mapping,
            plan.old_address,
            plan.generation,
            plan.new_start,
            &mut map,
            Some(&mut region),
        )?;
        if plan.generation == generation {
            if region.position != index_start_address {
                return Err(PactError::internal("Pact repack index placement mismatch"));
            }
            copy_spool(
                &index_spool,
                index_bytes,
                &mut region,
                options.io_chunk_bytes,
            )?;
            let (len, slot) =
                write_terminators(&mut region, generation, plan.new_start, index_root_address)?;
            committed_len = len;
            final_slot = Some(slot);
        } else if let Some((spool, bytes, root)) = &plan.baseline_index {
            copy_spool(spool, *bytes, &mut region, options.io_chunk_bytes)?;
            write_terminators(&mut region, plan.generation, plan.new_start, *root)?;
        } else {
            write_terminators(
                &mut region,
                plan.generation,
                plan.new_start,
                index_root_address,
            )?;
        }
    }
    let slot =
        final_slot.ok_or_else(|| PactError::internal("Pact repack wrote no final region"))?;
    observer(RepackStage::RecordsCopied)?;
    file.sync_data()?;
    write_slot(&mut file, 0, &slot)?;
    file.sync_all()?;
    observer(RepackStage::CandidateComplete)?;

    let check = Snapshot::open_with_memory_options(
        candidate,
        std::sync::Arc::new(super::backend::IoCounterSet::default()),
        options,
    )?;
    if check.generation() != generation
        || check.committed_len() != committed_len
        || check.metadata()? != snapshot.metadata()?
    {
        return Err(PactError::corrupt(
            "Pact repack candidate does not describe the source's committed head",
        ));
    }
    drop(check);

    let current = std::fs::metadata(canonical)?;
    if super::append_commit::platform_file_id(&current) != source_id
        || current.len() < snapshot.committed_len()
    {
        return Err(PactError::invalid(
            "Pact file advanced under maintenance; the candidate was discarded",
        ));
    }
    let mut source = File::open(canonical)?;
    let (selection, _) =
        super::log::select_committed_slot_streamed(&mut source, options.io_chunk_bytes)?;
    if selection.slot.generation != generation
        || selection.slot.file_length_watermark != snapshot.committed_len()
    {
        return Err(PactError::invalid(
            "Pact file advanced under maintenance; the candidate was discarded",
        ));
    }
    drop(source);
    observer(RepackStage::BeforeReplace)?;
    std::fs::rename(candidate, canonical)?;
    if let Some(parent) = canonical.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
    observer(RepackStage::Replaced)?;
    Ok(RepackReport {
        generation,
        bytes_before: snapshot.committed_len(),
        bytes_after: committed_len,
        transactions,
        records_copied,
        index_entries,
        index_pages,
    })
}

#[allow(dead_code)]
pub(crate) fn reachable_page_count(pages: &dyn PageReader, root: u64) -> PactResult<u64> {
    let mut seen = BTreeMap::new();
    let mut stack = vec![root];
    while let Some(address) = stack.pop() {
        if seen.insert(address, ()).is_some() {
            continue;
        }
        let page = pages.read_page(address)?;
        let _ = PAGE_BYTES;
        stack.extend(page_references(&page)?);
    }
    Ok(seen.len() as u64)
}

pub(crate) const MESSAGE_ORDER_REPAIR_MARKER: &str = "pact.maintenance.message_order_repair.v1";

pub(crate) fn plan_message_order_repair(
    snapshot: &Snapshot,
    branch: &str,
    options: &PactMemoryOptions,
) -> PactResult<MessageOrderRepairPlan> {
    let mut plan = MessageOrderRepairPlan::new(options)?;
    let mapping = snapshot.mapping();
    let committed = usize::try_from(snapshot.committed_len())
        .map_err(|_| PactError::corrupt("Pact committed length exceeds the address space"))?
        .min(mapping.len());
    let revision_key = |revision: u64| IndexKey::Revision {
        branch: branch.to_string(),
        revision,
    };
    let mut next_revision = 0_u64;
    loop {
        let entries = snapshot.index_range(
            &revision_key(next_revision),
            &revision_key(u64::MAX),
            SCAN_PAGE_ENTRIES,
            false,
        )?;
        let Some((last_key, _)) = entries.last() else {
            break;
        };
        let IndexKey::Revision {
            revision: last_revision,
            ..
        } = IndexKey::decode(last_key)?
        else {
            return Err(PactError::corrupt(
                "Pact revision range returned another namespace",
            ));
        };
        for (key, value) in &entries {
            let IndexKey::Revision { revision, .. } = IndexKey::decode(key)? else {
                return Err(PactError::corrupt(
                    "Pact revision range returned another namespace",
                ));
            };
            let address = decode_u64_value(value, "Pact transaction address")?;
            let mut position = usize::try_from(address)
                .ok()
                .filter(|position| *position < committed)
                .ok_or_else(|| {
                    PactError::corrupt(format!(
                        "Pact revision {revision} names address {address} past the committed length"
                    ))
                })?;
            let record = read_record(&mapping[..committed], &mut position)?;
            if record.kind == RecordKind::Transaction
                && transaction_record_is_baseline(record.payload)
            {
                continue;
            }
            replay_message_rows(mapping, address, revision, options, &mut plan)?;
        }
        if entries.len() < SCAN_PAGE_ENTRIES {
            break;
        }
        next_revision = last_revision
            .checked_add(1)
            .ok_or_else(|| PactError::corrupt("Pact revision overflow"))?;
    }

    let root = snapshot.index_root();
    let order_key = |order: u64| IndexKey::MessageOrder {
        branch: branch.to_string(),
        order,
    };
    let mut start = 0_u64;
    loop {
        let entries = snapshot.index_range(
            &order_key(start),
            &order_key(u64::MAX),
            SCAN_PAGE_ENTRIES,
            false,
        )?;
        let Some((last_key, _)) = entries.last() else {
            break;
        };
        let IndexKey::MessageOrder { order: last, .. } = IndexKey::decode(last_key)? else {
            return Err(PactError::corrupt(
                "Pact message order range returned another namespace",
            ));
        };
        for (key, value) in &entries {
            let IndexKey::MessageOrder { order, .. } = IndexKey::decode(key)? else {
                return Err(PactError::corrupt(
                    "Pact message order range returned another namespace",
                ));
            };
            if plan.get(value)?.is_some_and(|wanted| wanted != order) {
                plan.stale_keys = plan.stale_keys.saturating_add(1);
            }
        }
        if entries.len() < SCAN_PAGE_ENTRIES {
            break;
        }
        start = last
            .checked_add(1)
            .ok_or_else(|| PactError::corrupt("Pact message order overflow"))?;
    }
    let mut start = Vec::new();
    loop {
        let page = plan.page(&start)?;
        let Some((last, _)) = page.last() else {
            break;
        };
        let mut next = last.clone();
        next.push(0);
        for (id, order) in &page {
            let order = decode_u64_value(order, "Pact message order plan value")?;
            let held = read_index_value_from_pages(snapshot, root, &order_key(order))?
                .is_some_and(|value| value == *id);
            if held {
                continue;
            }
            let component_id = std::str::from_utf8(id).map_err(|_| {
                PactError::corrupt("Pact message order plan holds a non-UTF-8 component id")
            })?;
            let facet = read_index_value_from_pages(
                snapshot,
                root,
                &IndexKey::Component {
                    branch: branch.to_string(),
                    component: component_id.to_string(),
                },
            )?;
            if facet.is_some() {
                plan.rows_to_reindex = plan.rows_to_reindex.saturating_add(1);
            }
        }
        if page.len() < REPAIR_SCAN_ENTRIES {
            break;
        }
        start = next;
    }
    Ok(plan)
}

const REPAIR_SCAN_ENTRIES: usize = 256;

fn replay_message_rows(
    mapping: &[u8],
    address: u64,
    revision: u64,
    options: &PactMemoryOptions,
    plan: &mut MessageOrderRepairPlan,
) -> PactResult<()> {
    let bytes = transaction_trace_bytes(mapping, address)?;
    let mut pages =
        ScratchPages::create(None, options.io_chunk_bytes, options.scratch_cache_bytes)?;
    let mut rank = 0_u64;
    let mut stream = match PttrStream::open(&bytes, &mut pages) {
        Ok(stream) => stream,
        Err(_) => {
            let trace = crate::transaction_trace::decode_transaction_trace(&bytes)?;
            for op in &trace.ops {
                let crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } =
                    &op.kind
                else {
                    continue;
                };
                if component_type(component) == Some("message_turn") {
                    plan.insert(&op.component_id, message_order_key(revision, rank)?)?;
                    rank += 1;
                }
            }
            return Ok(());
        }
    };
    let mut type_keys = BTreeSet::new();
    let mut turn_values = BTreeSet::new();
    let mut classify = |ordinal: u64, text: &str| match text {
        "componentType" | "type" | "node_type" => {
            type_keys.insert(ordinal);
        }
        "\"message_turn\"" | "message_turn" => {
            turn_values.insert(ordinal);
        }
        _ => {}
    };
    for (ordinal, tag) in WELL_KNOWN_SYMBOLS.iter().enumerate() {
        classify(ordinal as u64, tag);
    }
    for ordinal in WELL_KNOWN_SYMBOLS.len() as u64..stream.symbol_count() {
        let text = stream.symbol_text(&pages, ordinal)?;
        classify(ordinal, &text);
    }
    while let Some(op) = stream.next_op()? {
        let StreamOpKind::Insert { fields, .. } = &op.kind else {
            continue;
        };
        let is_message = fields
            .iter()
            .any(|(key, value)| type_keys.contains(key) && turn_values.contains(value));
        if !is_message {
            continue;
        }
        let id = stream.symbol_text(&pages, op.component)?;
        plan.insert(&id, message_order_key(revision, rank)?)?;
        rank += 1;
    }
    Ok(())
}
