use super::*;

use crate::error::{PactError, PactResult};

#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub manifest: PactFileManifest,
    pub records: Vec<ScannedRecord>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ScannedRecord {
    pub kind: String,
    pub payload: Value,
    pub payload_bytes: usize,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanMode {
    ManifestOnly,
    PageSummary,
    CollectRecords,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScanCacheKey {
    path: PathBuf,
    file_len: u64,
    modified_nanos: u128,
    collect_records: bool,
}

#[derive(Default)]
struct ScanCache {
    entries: Vec<(ScanCacheKey, Arc<ScannedFile>)>,
    hits: u64,
    misses: u64,
    invalidations: u64,
}

static SCAN_CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();

fn scan_cache() -> &'static Mutex<ScanCache> {
    SCAN_CACHE.get_or_init(|| Mutex::new(ScanCache::default()))
}

pub fn scan_file(bytes: &[u8], collect_records: bool) -> ScannedFile {
    scan_file_with_mode(
        bytes,
        if collect_records {
            ScanMode::CollectRecords
        } else {
            ScanMode::ManifestOnly
        },
    )
}

pub fn scan_file_with_mode(bytes: &[u8], mode: ScanMode) -> ScannedFile {
    let collect_records = mode == ScanMode::CollectRecords;
    let committed_len = crate::pact_file::committed_prefix(bytes)
        .map(|committed| committed.len())
        .unwrap_or_default();
    if !collect_records && let Ok(manifest) = indexed_scan_manifest(bytes, committed_len) {
        return ScannedFile {
            manifest,
            records: Vec::new(),
            errors: Vec::new(),
        };
    }
    match crate::pact_file::open_ledger_bytes(bytes) {
        Ok(ledger) => {
            let manifest = v2_scan_manifest(&ledger, bytes.len(), committed_len);
            scan_v2_ledger(ledger, manifest, Vec::new(), collect_records)
                .unwrap_or_else(|error| invalid_scan(bytes.len(), committed_len, error.to_string()))
        }
        Err(error) => invalid_scan(bytes.len(), committed_len, error.to_string()),
    }
}

pub fn scan_pact_file(path: &Path, collect_records: bool) -> PactResult<Arc<ScannedFile>> {
    scan_file_cached(path, collect_records)
}

pub(super) fn scan_file_page_summary_from_path(path: &Path) -> PactResult<ScannedFile> {
    scan_file_cached(path, false).map(|scanned| scanned.as_ref().clone())
}

pub(super) fn scan_file_cached(path: &Path, collect_records: bool) -> PactResult<Arc<ScannedFile>> {
    let metadata = fs::metadata(path)
        .map_err(|error| PactError::storage(format!("failed to stat .pact file: {error}")))?;
    let key = ScanCacheKey {
        path: path.to_path_buf(),
        file_len: metadata.len(),
        modified_nanos: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos()),
        collect_records,
    };
    {
        let mut cache = scan_cache()
            .lock()
            .map_err(|_| PactError::internal("failed to lock .pact scan cache"))?;
        if let Some((_, scanned)) = cache.entries.iter().find(|(entry, _)| entry == &key) {
            let scanned = Arc::clone(scanned);
            cache.hits += 1;
            return Ok(scanned);
        }
        cache.misses += 1;
    }
    let bytes = fs::read(path)
        .map_err(|error| PactError::storage(format!("failed to read .pact file: {error}")))?;
    let scanned = Arc::new(scan_file(&bytes, collect_records));
    let mut cache = scan_cache()
        .lock()
        .map_err(|_| PactError::internal("failed to lock .pact scan cache"))?;
    cache
        .entries
        .retain(|(entry, _)| entry.path != key.path || entry.collect_records != collect_records);
    cache.entries.push((key, Arc::clone(&scanned)));
    if cache.entries.len() > 16 {
        cache.entries.remove(0);
    }
    Ok(scanned)
}

pub(super) fn invalidate_scan_cache(path: &Path) {
    let Some(cache) = SCAN_CACHE.get() else {
        return;
    };
    if let Ok(mut cache) = cache.lock() {
        let before = cache.entries.len();
        cache.entries.retain(|(entry, _)| entry.path != path);
        if cache.entries.len() != before {
            cache.invalidations += 1;
        }
    }
}

pub(super) fn scan_cache_snapshot() -> (u64, u64, u64, usize) {
    let cache = scan_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (
        cache.hits,
        cache.misses,
        cache.invalidations,
        cache.entries.len(),
    )
}

fn invalid_scan(file_bytes: usize, committed_bytes: usize, error: String) -> ScannedFile {
    ScannedFile {
        manifest: PactFileManifest {
            schema: "pact.file.v2".to_string(),
            version: 2,
            file_bytes,
            last_valid_offset: committed_bytes,
            ..PactFileManifest::default()
        },
        records: Vec::new(),
        errors: vec![error],
    }
}

fn indexed_scan_manifest(bytes: &[u8], committed_bytes: usize) -> PactResult<PactFileManifest> {
    let selected = crate::pact_file::root_slot::select_root_slot(bytes)?;
    let root = selected.slot.index_root_address;
    let encoded = crate::pact_file::index::read_index_value(
        bytes,
        root,
        &crate::pact_file::index::IndexKey::Metadata,
    )?
    .ok_or_else(|| PactError::corrupt("Pact index has no head metadata"))?;
    let metadata = crate::pact_file::index::HeadMetadata::decode(&encoded)?;
    let current = metadata.current_branch_id.as_str();
    let node_count = metadata
        .node_counts
        .get(current)
        .copied()
        .unwrap_or_default();
    let head_revision = metadata.head_revisions.get(current).copied();
    let commit_count = head_revision.map_or(0, |revision| revision as usize + 1);
    Ok(PactFileManifest {
        schema: "pact.file.v2".to_string(),
        version: 2,
        file_kind: "branch".to_string(),
        capabilities: vec!["canonical-ledger".to_string(), "root-slots".to_string()],
        file_bytes: bytes.len(),
        record_count: node_count + metadata.trace_count + 3,
        branch_count: metadata.branch_count,
        node_count,
        trace_count: metadata.trace_count,
        component_count: node_count,
        commit_count,
        head_count: metadata.branch_count,
        cycle_count: commit_count,
        last_valid_offset: committed_bytes,
        ..PactFileManifest::default()
    })
}

fn v2_scan_manifest(
    ledger: &ContextTreeLedger,
    file_bytes: usize,
    committed_bytes: usize,
) -> PactFileManifest {
    let current = ledger.branches.get(&ledger.current_branch_id);
    let node_count = current.map_or(0, |branch| branch.current.tree.index.by_id.len());
    PactFileManifest {
        schema: "pact.file.v2".to_string(),
        version: 2,
        file_kind: "branch".to_string(),
        capabilities: vec!["canonical-ledger".to_string(), "root-slots".to_string()],
        file_bytes,
        record_count: node_count + ledger.trace_count() + 3,
        branch_count: ledger.branches.len(),
        node_count,
        trace_count: ledger.trace_count(),
        component_count: node_count,
        commit_count: current.map_or(0, |branch| branch.head_revision as usize + 1),
        head_count: ledger.branches.len(),
        cycle_count: current.map_or(0, |branch| branch.head_revision as usize + 1),
        last_valid_offset: committed_bytes,
        ..PactFileManifest::default()
    }
}

fn scan_v2_ledger(
    ledger: ContextTreeLedger,
    manifest: PactFileManifest,
    errors: Vec<String>,
    collect_records: bool,
) -> PactResult<ScannedFile> {
    let mut scanned = ScannedFile {
        manifest,
        records: Vec::new(),
        errors,
    };
    if !collect_records || !scanned.errors.is_empty() {
        return Ok(scanned);
    }
    let mut sequence = 1_u64;
    let mut push = |kind: &str, payload: Value| {
        let payload_bytes = serde_json::to_vec(&payload).map_or(0, |bytes| bytes.len());
        scanned.records.push(ScannedRecord {
            kind: kind.to_string(),
            payload,
            payload_bytes,
            sequence,
        });
        sequence += 1;
    };
    push(
        "meta",
        json!({
            "schema": "pact.file.v2",
            "version": 2,
            "root_id": ledger.root_id,
            "current_branch_id": ledger.current_branch_id,
            "branch_count": ledger.branches.len(),
            "capabilities": ["canonical-ledger", "root-slots"],
        }),
    );
    push(
        "ledger",
        serde_json::to_value(&ledger)
            .map_err(|error| PactError::codec(format!("encode ledger facade: {error}")))?,
    );
    let mut root = json!({
        "root_id": ledger.root_id,
        "current_branch_id": ledger.current_branch_id,
        "branches": ledger.branches.keys().cloned().collect::<Vec<_>>(),
    });
    if let Some(parent) = &ledger.parent {
        root["parent"] = serde_json::to_value(parent)
            .map_err(|error| PactError::codec(format!("encode parent provenance: {error}")))?;
    }
    if let Some(base) = &ledger.base {
        root["base"] = serde_json::to_value(base)
            .map_err(|error| PactError::codec(format!("encode base provenance: {error}")))?;
    }
    push("root", root);
    let mut content = ContentCollector::with_compression();
    for record in ledger.content.records.values() {
        content.put_content_record(record)?;
    }
    for payload in content.by_hash.into_values() {
        push(
            "content",
            serde_json::to_value(payload)
                .map_err(|error| PactError::codec(format!("encode content facade: {error}")))?,
        );
    }
    for chunk in content.chunks {
        push(
            "content_chunk",
            serde_json::to_value(chunk)
                .map_err(|error| PactError::codec(format!("encode chunk facade: {error}")))?,
        );
    }
    for branch in ledger.branches.values() {
        push("branch", super::storage::branch_record_payload(branch)?);
        for node in super::storage::cleaned_component_records(branch).into_values() {
            push(
                "node",
                serde_json::to_value(node)
                    .map_err(|error| PactError::codec(format!("encode node facade: {error}")))?,
            );
        }
    }
    let mut traces = ledger
        .event_index
        .transaction_traces
        .values()
        .collect::<Vec<_>>();
    traces.sort_by(|left, right| trace_facade_order(left).cmp(&trace_facade_order(right)));
    for trace in traces {
        push(
            "trace",
            serde_json::to_value(trace)
                .map_err(|error| PactError::codec(format!("encode trace facade: {error}")))?,
        );
    }
    for view in super::v2_view_records(&ledger)?.views {
        push(
            "view",
            serde_json::to_value(view)
                .map_err(|error| PactError::codec(format!("encode view facade: {error}")))?,
        );
    }
    scanned.manifest.record_count = scanned.records.len();
    enrich_manifest_from_records(&mut scanned.manifest, &scanned.records);
    Ok(scanned)
}

fn trace_facade_order(trace: &PactTransactionTrace) -> (u64, u64, &str, &str) {
    let (from_rev, to_rev) = trace.replay_trace.as_ref().map_or_else(
        || {
            let revision = trace.authored_ref.revision.unwrap_or(u64::MAX);
            (revision, revision)
        },
        |replay| (replay.from_rev, replay.to_rev),
    );
    (
        from_rev,
        to_rev,
        trace.branch_id.as_str(),
        trace.transaction_id.as_str(),
    )
}

pub(crate) struct PactReadSession {
    scanned: Arc<ScannedFile>,
}

impl PactReadSession {
    pub(crate) fn open(path: &Path) -> PactResult<Self> {
        Ok(Self {
            scanned: scan_file_cached(path, true)?,
        })
    }

    pub(crate) fn records(&self) -> &[ScannedRecord] {
        &self.scanned.records
    }

    pub(crate) fn errors(&self) -> &[String] {
        &self.scanned.errors
    }

    pub(crate) fn validation(&self) -> super::wire::PactFileValidation {
        validation_from_scan_ref(&self.scanned)
    }
}
