use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::atomic_output::{PublishMode, publish_staged};
use crate::error::{PactError, PactResult};

const FACET_MAGIC: &[u8; 8] = b"PACTSEM1";
const FACET_VERSION: u16 = 1;
const FACET_HEADER_BYTES: usize = 4096;
const FACET_IDENTITY_START: usize = 16;
const FACET_IDENTITY_BYTES: usize = 32;
const FACET_ROOT_SLOT_START: usize = 64;
const FACET_ROOT_SLOT_COUNT: usize = 2;
const FACET_ROOT_SLOT_BYTES: usize = 128;
const FACET_ROOT_SLOT_PAYLOAD_BYTES: usize = 96;
const FACET_ROOT_SLOT_DIGEST_START: usize = 96;
const FACET_ROOT_SLOT_DIGEST_BYTES: usize = 32;
const FACET_MAX_FILE_LENGTH_WATERMARK: u64 = 1_u64 << 56;
const FACET_SEGMENT_DIGEST_BYTES: usize = 32;
const FACET_COPY_BUFFER_BYTES: usize = 64 * 1024;

fn record_facet_compaction_chunk_bytes(bytes: usize) {
    #[cfg(test)]
    tests::record_facet_compaction_chunk_bytes(bytes);
    #[cfg(not(test))]
    let _ = bytes;
}

pub fn facet_path_for(source: &Path) -> PathBuf {
    let mut name = source.as_os_str().to_os_string();
    if source.extension().and_then(|extension| extension.to_str()) == Some("pact") {
        name.push(".sem");
    } else {
        name.push(".pact.sem");
    }
    PathBuf::from(name)
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum FacetSegmentType {
    Code(u16),

    Name(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacetCompactionPolicy {
    pub max_dead_live_ratio: f64,
}

impl FacetCompactionPolicy {
    pub fn new(max_dead_live_ratio: f64) -> Self {
        Self {
            max_dead_live_ratio,
        }
    }

    pub fn disabled() -> Self {
        Self {
            max_dead_live_ratio: f64::INFINITY,
        }
    }

    fn validate(&self) -> PactResult<()> {
        if self.max_dead_live_ratio.is_nan() || self.max_dead_live_ratio < 0.0 {
            return Err(PactError::invalid(
                "facet compaction policy requires a non-negative dead/live ratio",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetSegmentInfo {
    pub id: u64,

    pub segment_type: FacetSegmentType,

    pub len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetTypeStats {
    pub segment_type: FacetSegmentType,

    pub segment_count: u64,

    pub live_bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FacetStats {
    pub generation: u64,

    pub file_length_watermark: u64,

    pub live_bytes: u64,

    pub dead_bytes: u64,

    pub by_type: Vec<FacetTypeStats>,
}

#[derive(Clone, Debug, Default)]
pub struct FacetWriteBatch {
    appends: Vec<(FacetSegmentType, Vec<u8>)>,
    drops: Vec<u64>,
}

impl FacetWriteBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&mut self, segment_type: FacetSegmentType, bytes: Vec<u8>) -> &mut Self {
        self.appends.push((segment_type, bytes));
        self
    }

    pub fn drop_segment(&mut self, id: u64) -> &mut Self {
        self.drops.push(id);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.appends.is_empty() && self.drops.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FacetFaultPoint {
    SegmentsWritten,
    DataSynced,
    ManifestWritten,
    ManifestSynced,
    RootSlotWritten,
    RootSynced,
    CompactionStaged,
    CompactionPublished,
}

fn inject(configured: Option<FacetFaultPoint>, point: FacetFaultPoint) -> PactResult<()> {
    if configured == Some(point) {
        return Err(PactError::Io(std::io::Error::other(format!(
            "injected candidate facet fault after {point:?}"
        ))));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FacetRootSlot {
    generation: u64,
    manifest_address: u64,
    manifest_len: u64,
    file_length_watermark: u64,
    manifest_digest: [u8; FACET_SEGMENT_DIGEST_BYTES],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManifestEntry {
    id: u64,
    segment_type: FacetSegmentType,
    address: u64,
    len: u64,
    digest: [u8; FACET_SEGMENT_DIGEST_BYTES],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Manifest {
    entries: Vec<ManifestEntry>,
    next_segment_id: u64,
}

#[derive(Clone, Debug)]
pub struct FacetStore {
    source_path: PathBuf,
    facet_path: PathBuf,
    policy: FacetCompactionPolicy,
}

impl FacetStore {
    pub fn create(source: &Path, policy: FacetCompactionPolicy) -> PactResult<Self> {
        policy.validate()?;

        std::fs::metadata(source)?;
        let store = Self {
            source_path: source.to_path_buf(),
            facet_path: facet_path_for(source),
            policy,
        };
        let manifest = Manifest {
            entries: Vec::new(),
            next_segment_id: 1,
        };
        let manifest_bytes = encode_manifest(&manifest)?;
        let slot = FacetRootSlot {
            generation: 1,
            manifest_address: FACET_HEADER_BYTES as u64,
            manifest_len: manifest_bytes.len() as u64,
            file_length_watermark: FACET_HEADER_BYTES as u64 + manifest_bytes.len() as u64,
            manifest_digest: *blake3::hash(&manifest_bytes).as_bytes(),
        };
        let header = encode_initial_header(new_facet_identity()?, &slot)?;
        let mut bytes = Vec::with_capacity(FACET_HEADER_BYTES + manifest_bytes.len());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&manifest_bytes);
        let staged = store.stage_with_source_mode(&bytes)?;
        publish_staged(staged, &store.facet_path, PublishMode::CreateNew)?;
        Ok(store)
    }

    pub fn open(source: &Path, policy: FacetCompactionPolicy) -> PactResult<Self> {
        policy.validate()?;
        let store = Self {
            source_path: source.to_path_buf(),
            facet_path: facet_path_for(source),
            policy,
        };

        FacetReader::open(&store.facet_path)?;
        Ok(store)
    }

    pub fn open_or_create(source: &Path, policy: FacetCompactionPolicy) -> PactResult<Self> {
        if facet_path_for(source).exists() {
            Self::open(source, policy)
        } else {
            Self::create(source, policy)
        }
    }

    pub fn path(&self) -> &Path {
        &self.facet_path
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub fn reader(&self) -> PactResult<FacetReader> {
        FacetReader::open(&self.facet_path)
    }

    pub fn stats(&self) -> PactResult<FacetStats> {
        Ok(self.reader()?.stats())
    }

    pub fn commit(&mut self, batch: &FacetWriteBatch) -> PactResult<FacetStats> {
        self.commit_with_fault(batch, None)
    }

    pub(crate) fn commit_with_fault(
        &mut self,
        batch: &FacetWriteBatch,
        fault: Option<FacetFaultPoint>,
    ) -> PactResult<FacetStats> {
        for (segment_type, _) in &batch.appends {
            validate_segment_type(segment_type)?;
        }
        let mut file = open_locked_current(&self.facet_path)?;
        let header = read_header(&mut file)?;
        validate_envelope(&header)?;
        let (current_index, current_slot, mut manifest) = newest_readable_root(&mut file, &header)?;

        let mut drops = batch.drops.clone();
        drops.sort_unstable();
        drops.dedup();
        for id in &drops {
            if manifest
                .entries
                .binary_search_by_key(id, |entry| entry.id)
                .is_err()
            {
                return Err(PactError::not_found(format!(
                    "facet segment {id} does not exist"
                )));
            }
        }
        manifest
            .entries
            .retain(|entry| drops.binary_search(&entry.id).is_err());

        let mut position = current_slot.file_length_watermark;
        file.seek(SeekFrom::Start(position))?;
        for (segment_type, bytes) in &batch.appends {
            let id = manifest.next_segment_id;
            manifest.next_segment_id = manifest
                .next_segment_id
                .checked_add(1)
                .ok_or_else(|| PactError::internal("facet segment id space exhausted"))?;
            file.write_all(bytes)?;
            manifest.entries.push(ManifestEntry {
                id,
                segment_type: segment_type.clone(),
                address: position,
                len: bytes.len() as u64,
                digest: *blake3::hash(bytes).as_bytes(),
            });
            position = advance_watermark(position, bytes.len() as u64)?;
        }
        inject(fault, FacetFaultPoint::SegmentsWritten)?;
        file.sync_data()?;
        inject(fault, FacetFaultPoint::DataSynced)?;

        let manifest_bytes = encode_manifest(&manifest)?;
        let manifest_address = position;
        file.write_all(&manifest_bytes)?;
        inject(fault, FacetFaultPoint::ManifestWritten)?;
        file.sync_data()?;
        inject(fault, FacetFaultPoint::ManifestSynced)?;

        let new_slot = FacetRootSlot {
            generation: current_slot
                .generation
                .checked_add(1)
                .ok_or_else(|| PactError::internal("facet generation space exhausted"))?,
            manifest_address,
            manifest_len: manifest_bytes.len() as u64,
            file_length_watermark: advance_watermark(
                manifest_address,
                manifest_bytes.len() as u64,
            )?,
            manifest_digest: *blake3::hash(&manifest_bytes).as_bytes(),
        };

        file.set_len(new_slot.file_length_watermark)?;
        file.sync_data()?;
        let target_index = (current_index + 1) % FACET_ROOT_SLOT_COUNT;
        let slot_bytes = encode_root_slot(&new_slot);
        file.seek(SeekFrom::Start(
            (FACET_ROOT_SLOT_START + target_index * FACET_ROOT_SLOT_BYTES) as u64,
        ))?;
        file.write_all(&slot_bytes)?;
        inject(fault, FacetFaultPoint::RootSlotWritten)?;
        file.sync_all()?;
        inject(fault, FacetFaultPoint::RootSynced)?;

        self.clamp_permissions_to_source()?;
        let stats = stats_for(&new_slot, &manifest);
        if self.should_compact(&stats) {
            return self.compact_with_fault(fault);
        }
        Ok(stats)
    }

    fn should_compact(&self, stats: &FacetStats) -> bool {
        let ratio = self.policy.max_dead_live_ratio;
        if !ratio.is_finite() {
            return false;
        }

        #[allow(clippy::cast_precision_loss)]
        {
            stats.dead_bytes as f64 > ratio * stats.live_bytes as f64
        }
    }

    fn compact_with_fault(&self, fault: Option<FacetFaultPoint>) -> PactResult<FacetStats> {
        let reader = FacetReader::open(&self.facet_path)?;
        let mut address = FACET_HEADER_BYTES as u64;
        let mut entries = Vec::with_capacity(reader.manifest.entries.len());
        for entry in &reader.manifest.entries {
            entries.push(ManifestEntry {
                id: entry.id,
                segment_type: entry.segment_type.clone(),
                address,
                len: entry.len,
                digest: entry.digest,
            });
            address = advance_watermark(address, entry.len)?;
        }
        let manifest = Manifest {
            entries,
            next_segment_id: reader.manifest.next_segment_id,
        };
        let manifest_bytes = encode_manifest(&manifest)?;
        let slot = FacetRootSlot {
            generation: reader
                .root
                .generation
                .checked_add(1)
                .ok_or_else(|| PactError::internal("facet generation space exhausted"))?,
            manifest_address: address,
            manifest_len: manifest_bytes.len() as u64,
            file_length_watermark: advance_watermark(address, manifest_bytes.len() as u64)?,
            manifest_digest: *blake3::hash(&manifest_bytes).as_bytes(),
        };
        let header = encode_initial_header(reader.identity, &slot)?;
        let staged =
            self.stage_streaming_compaction(&reader, &manifest, &slot, &header, &manifest_bytes)?;
        inject(fault, FacetFaultPoint::CompactionStaged)?;
        publish_staged(staged, &self.facet_path, PublishMode::ReplaceExisting)?;
        inject(fault, FacetFaultPoint::CompactionPublished)?;
        Ok(stats_for(&slot, &manifest))
    }

    fn stage_streaming_compaction(
        &self,
        reader: &FacetReader,
        manifest: &Manifest,
        slot: &FacetRootSlot,
        header: &[u8; FACET_HEADER_BYTES],
        manifest_bytes: &[u8],
    ) -> PactResult<tempfile::NamedTempFile> {
        let mut staged = self.new_staged_with_source_mode()?;
        staged.write_all(header)?;
        for entry in &reader.manifest.entries {
            stream_verified_segment(&reader.file, entry, |chunk| {
                record_facet_compaction_chunk_bytes(chunk.len());
                staged.write_all(chunk)?;
                Ok(())
            })?;
        }
        staged.write_all(manifest_bytes)?;
        staged.as_file().sync_all()?;
        if staged.as_file().metadata()?.len() != slot.file_length_watermark {
            return Err(PactError::storage(
                "streamed facet candidate length did not match its root watermark",
            ));
        }

        let verified = FacetReader::open(staged.path())?;
        if verified.identity != reader.identity
            || verified.root != *slot
            || verified.manifest != *manifest
        {
            return Err(PactError::storage(
                "streamed facet candidate did not match its requested root",
            ));
        }
        for entry in &verified.manifest.entries {
            stream_verified_segment(&verified.file, entry, |_| Ok(()))?;
        }
        Ok(staged)
    }

    fn stage_with_source_mode(&self, bytes: &[u8]) -> PactResult<tempfile::NamedTempFile> {
        let mut staged = self.new_staged_with_source_mode()?;
        staged.write_all(bytes)?;
        staged.as_file().sync_all()?;
        if std::fs::read(staged.path())? != bytes {
            return Err(PactError::storage(
                "facet publication candidate did not match requested bytes",
            ));
        }
        Ok(staged)
    }

    fn new_staged_with_source_mode(&self) -> PactResult<tempfile::NamedTempFile> {
        let parent = self
            .facet_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let staged = tempfile::NamedTempFile::new_in(&parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = self.facet_mode_for_source()?;
            std::fs::set_permissions(staged.path(), std::fs::Permissions::from_mode(mode))?;
        }
        Ok(staged)
    }

    #[cfg(unix)]
    fn facet_mode_for_source(&self) -> PactResult<u32> {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&self.source_path)?.permissions().mode();

        Ok(mode & 0o666)
    }

    fn clamp_permissions_to_source(&self) -> PactResult<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let allowed = self.facet_mode_for_source()?;
            let current = std::fs::metadata(&self.facet_path)?.permissions().mode() & 0o7777;
            let clamped = current & allowed;
            if clamped != current {
                std::fs::set_permissions(
                    &self.facet_path,
                    std::fs::Permissions::from_mode(clamped),
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct FacetReader {
    file: File,
    identity: [u8; FACET_IDENTITY_BYTES],
    root: FacetRootSlot,
    manifest: Manifest,
    infos: Vec<FacetSegmentInfo>,
}

impl FacetReader {
    fn open(path: &Path) -> PactResult<Self> {
        let mut file = File::open(path)?;
        let header = read_header(&mut file)?;
        validate_envelope(&header)?;
        let mut identity = [0_u8; FACET_IDENTITY_BYTES];
        identity.copy_from_slice(
            &header[FACET_IDENTITY_START..FACET_IDENTITY_START + FACET_IDENTITY_BYTES],
        );
        let (_, root, manifest) = newest_readable_root(&mut file, &header)?;
        let infos = manifest
            .entries
            .iter()
            .map(|entry| FacetSegmentInfo {
                id: entry.id,
                segment_type: entry.segment_type.clone(),
                len: entry.len,
            })
            .collect();
        Ok(Self {
            file,
            identity,
            root,
            manifest,
            infos,
        })
    }

    pub fn generation(&self) -> u64 {
        self.root.generation
    }

    pub fn segments(&self) -> &[FacetSegmentInfo] {
        &self.infos
    }

    pub fn read_segment(&self, id: u64) -> PactResult<Vec<u8>> {
        let index = self
            .manifest
            .entries
            .binary_search_by_key(&id, |entry| entry.id)
            .map_err(|_| PactError::not_found(format!("facet segment {id} does not exist")))?;
        let entry = &self.manifest.entries[index];
        let len = usize::try_from(entry.len)
            .map_err(|_| PactError::corrupt("facet segment length exceeds addressable memory"))?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(entry.address))?;
        let mut bytes = vec![0_u8; len];
        file.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                PactError::corrupt(format!("facet segment {id} is truncated"))
            } else {
                PactError::Io(error)
            }
        })?;
        if blake3::hash(&bytes).as_bytes() != &entry.digest {
            return Err(PactError::corrupt(format!(
                "facet segment {id} failed checksum validation"
            )));
        }
        Ok(bytes)
    }

    pub fn stats(&self) -> FacetStats {
        stats_for(&self.root, &self.manifest)
    }
}

fn validate_segment_type(segment_type: &FacetSegmentType) -> PactResult<()> {
    match segment_type {
        FacetSegmentType::Code(_) => Ok(()),
        FacetSegmentType::Name(name) => {
            if name.is_empty() {
                return Err(PactError::invalid(
                    "facet segment type name cannot be empty",
                ));
            }
            if u16::try_from(name.len()).is_err() {
                return Err(PactError::invalid(
                    "facet segment type name exceeds 65535 bytes",
                ));
            }
            Ok(())
        }
    }
}

fn advance_watermark(position: u64, len: u64) -> PactResult<u64> {
    let next = position
        .checked_add(len)
        .filter(|next| *next <= FACET_MAX_FILE_LENGTH_WATERMARK)
        .ok_or_else(|| PactError::storage("facet file exceeds maximum length"))?;
    Ok(next)
}

fn stream_verified_segment(
    file: &File,
    entry: &ManifestEntry,
    mut visit: impl FnMut(&[u8]) -> PactResult<()>,
) -> PactResult<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(entry.address))?;
    let mut remaining = entry.len;
    let mut buffer = vec![0_u8; FACET_COPY_BUFFER_BYTES];
    let mut digest = blake3::Hasher::new();
    while remaining > 0 {
        let chunk_len = usize::try_from(remaining.min(FACET_COPY_BUFFER_BYTES as u64))
            .map_err(|_| PactError::corrupt("facet segment chunk exceeds addressable memory"))?;
        let chunk = &mut buffer[..chunk_len];
        file.read_exact(chunk).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                PactError::corrupt(format!("facet segment {} is truncated", entry.id))
            } else {
                PactError::Io(error)
            }
        })?;
        digest.update(chunk);
        visit(chunk)?;
        remaining -= chunk_len as u64;
    }
    if digest.finalize().as_bytes() != &entry.digest {
        return Err(PactError::corrupt(format!(
            "facet segment {} failed checksum validation",
            entry.id
        )));
    }
    Ok(())
}

fn new_facet_identity() -> PactResult<[u8; FACET_IDENTITY_BYTES]> {
    let mut identity = [0_u8; FACET_IDENTITY_BYTES];
    getrandom::fill(&mut identity).map_err(|error| {
        PactError::internal(format!("facet identity generation failed: {error}"))
    })?;
    if identity.iter().all(|byte| *byte == 0) {
        return Err(PactError::internal(
            "generated facet identity was unexpectedly empty",
        ));
    }
    Ok(identity)
}

fn encode_initial_header(
    identity: [u8; FACET_IDENTITY_BYTES],
    slot: &FacetRootSlot,
) -> PactResult<[u8; FACET_HEADER_BYTES]> {
    let mut header = [0_u8; FACET_HEADER_BYTES];
    header[..FACET_MAGIC.len()].copy_from_slice(FACET_MAGIC);
    header[8..10].copy_from_slice(&FACET_VERSION.to_le_bytes());
    header[10..12].copy_from_slice(&(FACET_HEADER_BYTES as u16).to_le_bytes());
    header[FACET_IDENTITY_START..FACET_IDENTITY_START + FACET_IDENTITY_BYTES]
        .copy_from_slice(&identity);
    let slot_bytes = encode_root_slot(slot);
    header[FACET_ROOT_SLOT_START..FACET_ROOT_SLOT_START + FACET_ROOT_SLOT_BYTES]
        .copy_from_slice(&slot_bytes);
    if decode_root_slot(&header, 0).is_none() {
        return Err(PactError::internal("encoded facet root slot is invalid"));
    }
    Ok(header)
}

fn encode_root_slot(slot: &FacetRootSlot) -> [u8; FACET_ROOT_SLOT_BYTES] {
    let mut bytes = [0_u8; FACET_ROOT_SLOT_BYTES];
    bytes[..8].copy_from_slice(&slot.generation.to_le_bytes());
    bytes[8..16].copy_from_slice(&slot.manifest_address.to_le_bytes());
    bytes[16..24].copy_from_slice(&slot.manifest_len.to_le_bytes());
    bytes[24..32].copy_from_slice(&slot.file_length_watermark.to_le_bytes());
    bytes[32..32 + FACET_SEGMENT_DIGEST_BYTES].copy_from_slice(&slot.manifest_digest);
    let digest = blake3::hash(&bytes[..FACET_ROOT_SLOT_PAYLOAD_BYTES]);
    bytes
        [FACET_ROOT_SLOT_DIGEST_START..FACET_ROOT_SLOT_DIGEST_START + FACET_ROOT_SLOT_DIGEST_BYTES]
        .copy_from_slice(digest.as_bytes());
    bytes
}

fn decode_root_slot(header: &[u8], slot: usize) -> Option<FacetRootSlot> {
    let start = FACET_ROOT_SLOT_START + slot * FACET_ROOT_SLOT_BYTES;
    let payload = header.get(start..start + FACET_ROOT_SLOT_PAYLOAD_BYTES)?;
    if payload.iter().all(|byte| *byte == 0) {
        return None;
    }
    let stored_digest = header.get(
        start + FACET_ROOT_SLOT_DIGEST_START
            ..start + FACET_ROOT_SLOT_DIGEST_START + FACET_ROOT_SLOT_DIGEST_BYTES,
    )?;
    if blake3::hash(payload).as_bytes() != stored_digest {
        return None;
    }
    let mut manifest_digest = [0_u8; FACET_SEGMENT_DIGEST_BYTES];
    manifest_digest.copy_from_slice(&payload[32..32 + FACET_SEGMENT_DIGEST_BYTES]);
    let decoded = FacetRootSlot {
        generation: read_u64_at(payload, 0),
        manifest_address: read_u64_at(payload, 8),
        manifest_len: read_u64_at(payload, 16),
        file_length_watermark: read_u64_at(payload, 24),
        manifest_digest,
    };
    let valid = decoded.generation > 0
        && decoded.manifest_len > 0
        && decoded.file_length_watermark >= FACET_HEADER_BYTES as u64
        && decoded.file_length_watermark <= FACET_MAX_FILE_LENGTH_WATERMARK
        && decoded.manifest_address >= FACET_HEADER_BYTES as u64
        && decoded
            .manifest_address
            .checked_add(decoded.manifest_len)
            .is_some_and(|end| end <= decoded.file_length_watermark);
    valid.then_some(decoded)
}

fn newest_readable_root(
    file: &mut File,
    header: &[u8],
) -> PactResult<(usize, FacetRootSlot, Manifest)> {
    let mut slots = (0..FACET_ROOT_SLOT_COUNT)
        .filter_map(|index| decode_root_slot(header, index).map(|root| (index, root)))
        .collect::<Vec<_>>();
    slots.sort_by_key(|(_, root)| std::cmp::Reverse(root.generation));
    if slots.is_empty() {
        return Err(PactError::corrupt("facet envelope has no valid root slot"));
    }
    let mut last_error = None;
    for (index, root) in slots {
        match read_manifest(file, &root) {
            Ok(manifest) => return Ok((index, root, manifest)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| PactError::corrupt("facet envelope has no readable root")))
}

fn open_locked_current(path: &Path) -> PactResult<File> {
    loop {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        file.lock()?;
        if path_still_points_to_file(path, &file)? {
            return Ok(file);
        }
    }
}

#[cfg(unix)]
fn path_still_points_to_file(path: &Path, file: &File) -> PactResult<bool> {
    use std::os::unix::fs::MetadataExt;

    let locked = file.metadata()?;
    let current = std::fs::metadata(path)?;
    Ok(locked.dev() == current.dev() && locked.ino() == current.ino())
}

#[cfg(not(unix))]
fn path_still_points_to_file(path: &Path, file: &File) -> PactResult<bool> {
    let mut locked = file.try_clone()?;
    let mut current = File::open(path)?;
    let locked_header = read_header(&mut locked)?;
    let current_header = read_header(&mut current)?;
    Ok(
        locked_header[FACET_IDENTITY_START..FACET_IDENTITY_START + FACET_IDENTITY_BYTES]
            == current_header[FACET_IDENTITY_START..FACET_IDENTITY_START + FACET_IDENTITY_BYTES]
            && (0..FACET_ROOT_SLOT_COUNT).all(|index| {
                decode_root_slot(&locked_header, index) == decode_root_slot(&current_header, index)
            }),
    )
}

fn read_header(file: &mut File) -> PactResult<[u8; FACET_HEADER_BYTES]> {
    let mut header = [0_u8; FACET_HEADER_BYTES];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            PactError::corrupt("truncated facet envelope")
        } else {
            PactError::Io(error)
        }
    })?;
    Ok(header)
}

fn validate_envelope(header: &[u8]) -> PactResult<()> {
    let magic = header
        .get(..FACET_MAGIC.len())
        .ok_or_else(|| PactError::corrupt("truncated facet envelope"))?;
    if magic != FACET_MAGIC {
        return Err(PactError::corrupt("unknown facet file magic"));
    }
    let version = read_u16(header, 8)?;
    if version != FACET_VERSION {
        return Err(PactError::unsupported(format!(
            "unsupported facet file version: {version}"
        )));
    }
    let header_bytes = usize::from(read_u16(header, 10)?);
    if header_bytes != FACET_HEADER_BYTES {
        return Err(PactError::corrupt(format!(
            "invalid facet header length: {header_bytes}"
        )));
    }
    Ok(())
}

fn read_manifest(file: &mut File, slot: &FacetRootSlot) -> PactResult<Manifest> {
    let len = usize::try_from(slot.manifest_len)
        .map_err(|_| PactError::corrupt("facet manifest length exceeds addressable memory"))?;
    file.seek(SeekFrom::Start(slot.manifest_address))?;
    let mut bytes = vec![0_u8; len];
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            PactError::corrupt("truncated facet manifest")
        } else {
            PactError::Io(error)
        }
    })?;
    if blake3::hash(&bytes).as_bytes() != &slot.manifest_digest {
        return Err(PactError::corrupt(
            "facet manifest failed checksum validation",
        ));
    }
    decode_manifest(&bytes)
}

fn encode_manifest(manifest: &Manifest) -> PactResult<Vec<u8>> {
    let count = u32::try_from(manifest.entries.len())
        .map_err(|_| PactError::invalid("facet manifest has too many segments"))?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&count.to_le_bytes());
    for entry in &manifest.entries {
        bytes.extend_from_slice(&entry.id.to_le_bytes());
        match &entry.segment_type {
            FacetSegmentType::Code(code) => {
                bytes.push(0);
                bytes.extend_from_slice(&code.to_le_bytes());
            }
            FacetSegmentType::Name(name) => {
                validate_segment_type(&entry.segment_type)?;
                let len = u16::try_from(name.len()).map_err(|_| {
                    PactError::invalid("facet segment type name exceeds 65535 bytes")
                })?;
                bytes.push(1);
                bytes.extend_from_slice(&len.to_le_bytes());
                bytes.extend_from_slice(name.as_bytes());
            }
        }
        bytes.extend_from_slice(&entry.address.to_le_bytes());
        bytes.extend_from_slice(&entry.len.to_le_bytes());
        bytes.extend_from_slice(&entry.digest);
    }
    bytes.extend_from_slice(&manifest.next_segment_id.to_le_bytes());
    Ok(bytes)
}

fn decode_manifest(bytes: &[u8]) -> PactResult<Manifest> {
    let mut cursor = ByteCursor { bytes, offset: 0 };
    let count = cursor.read_u32()?;
    let mut entries: Vec<ManifestEntry> = Vec::new();
    for _ in 0..count {
        let id = cursor.read_u64()?;
        if entries.last().is_some_and(|previous| previous.id >= id) {
            return Err(PactError::corrupt(
                "facet manifest segment ids are not strictly increasing",
            ));
        }
        let segment_type = match cursor.read_u8()? {
            0 => FacetSegmentType::Code(cursor.read_u16()?),
            1 => {
                let name_len = usize::from(cursor.read_u16()?);
                if name_len == 0 {
                    return Err(PactError::corrupt(
                        "facet segment type name cannot be empty",
                    ));
                }
                let raw = cursor.read_slice(name_len)?;
                FacetSegmentType::Name(String::from_utf8(raw.to_vec()).map_err(|_| {
                    PactError::corrupt("facet segment type name is not valid UTF-8")
                })?)
            }
            tag => {
                return Err(PactError::corrupt(format!(
                    "unknown facet segment type tag: {tag}"
                )));
            }
        };
        let address = cursor.read_u64()?;
        let len = cursor.read_u64()?;
        let mut digest = [0_u8; FACET_SEGMENT_DIGEST_BYTES];
        digest.copy_from_slice(cursor.read_slice(FACET_SEGMENT_DIGEST_BYTES)?);
        entries.push(ManifestEntry {
            id,
            segment_type,
            address,
            len,
            digest,
        });
    }
    let next_segment_id = cursor.read_u64()?;
    if cursor.offset != bytes.len() {
        return Err(PactError::corrupt("facet manifest has trailing bytes"));
    }
    if next_segment_id == 0
        || entries
            .last()
            .is_some_and(|entry| entry.id >= next_segment_id)
    {
        return Err(PactError::corrupt(
            "facet manifest next segment id is not past every entry",
        ));
    }
    Ok(Manifest {
        entries,
        next_segment_id,
    })
}

fn stats_for(slot: &FacetRootSlot, manifest: &Manifest) -> FacetStats {
    let live_bytes: u64 = manifest.entries.iter().map(|entry| entry.len).sum();
    let dead_bytes = slot
        .file_length_watermark
        .saturating_sub(FACET_HEADER_BYTES as u64)
        .saturating_sub(live_bytes)
        .saturating_sub(slot.manifest_len);
    let mut by_type: BTreeMap<&FacetSegmentType, (u64, u64)> = BTreeMap::new();
    for entry in &manifest.entries {
        let row = by_type.entry(&entry.segment_type).or_insert((0, 0));
        row.0 += 1;
        row.1 += entry.len;
    }
    FacetStats {
        generation: slot.generation,
        file_length_watermark: slot.file_length_watermark,
        live_bytes,
        dead_bytes,
        by_type: by_type
            .into_iter()
            .map(|(segment_type, (segment_count, live))| FacetTypeStats {
                segment_type: segment_type.clone(),
                segment_count,
                live_bytes: live,
            })
            .collect(),
    }
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl ByteCursor<'_> {
    fn read_slice(&mut self, len: usize) -> PactResult<&[u8]> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| PactError::corrupt("truncated facet manifest"))?;
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> PactResult<u8> {
        Ok(self.read_slice(1)?[0])
    }

    fn read_u16(&mut self) -> PactResult<u16> {
        let raw = self.read_slice(2)?;
        Ok(u16::from_le_bytes([raw[0], raw[1]]))
    }

    fn read_u32(&mut self) -> PactResult<u32> {
        let raw = self.read_slice(4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn read_u64(&mut self) -> PactResult<u64> {
        let raw = self.read_slice(8)?;
        let mut buffer = [0_u8; 8];
        buffer.copy_from_slice(raw);
        Ok(u64::from_le_bytes(buffer))
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> PactResult<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| PactError::corrupt("truncated facet envelope"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}

#[cfg(test)]
mod tests;
