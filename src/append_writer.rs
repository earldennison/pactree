use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::components::Component;
use crate::error::{PactError, PactErrorKind, PactResult};
use crate::extension::PactExtensionRecord;
use crate::memory::PactMemoryOptions;
use crate::pact_file::append_commit::{PinnedFile, PublishError, WriterLock, pin, publish};
use crate::pact_file::append_prepare::AppendMessage;
use crate::pact_file::append_state::AppendState;
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::index::{IndexKey, MessageOrderRepairPlan};
use crate::pact_file::log::FaultPoint;
use crate::pact_file::stream_maintenance::{
    MaintenancePolicy, RepackStage, assess, plan_message_order_repair, remove_stale_candidates,
    repack,
};
use crate::resolver::message_authoring::{
    AuthoringIdentity, DeterministicIdentity, NativeIdentity,
};
use crate::{ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs};

#[cfg(test)]
mod tests;

pub const PACT_APPEND_RECEIPT_KIND: &str = "pactree.append.receipt.v1";

pub const PACT_APPEND_REQUEST_SCHEMA: &str = "pactree.append.request.v1";

pub const PACT_APPEND_CAPABILITY: &str = "pact.append.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PactMaintenanceReport {
    pub generation: u64,
    pub file_bytes_before: u64,
    pub file_bytes_after: u64,
    pub derived_amplification_permille: u64,
    pub dead_page_permille: u64,
    pub needed: bool,
    pub repacked: bool,
    pub reasons: Vec<String>,
}

pub const PACT_MESSAGE_ORDER_REPAIR_MARKER: &str =
    crate::pact_file::stream_maintenance::MESSAGE_ORDER_REPAIR_MARKER;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PactMessageOrderRepair {
    pub transaction_id: Option<String>,

    pub revision: u64,

    pub rows_scanned: u64,

    pub rows_reindexed: u64,

    pub stale_keys_removed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PactFileVersion {
    pub file_identity: [u8; 32],
    pub generation: u64,
    pub branch_id: String,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct PactAppendMessage {
    pub row_id: String,
    pub role: String,
    pub parts: Vec<Component>,
    pub aux: PactAttrs,
}

#[derive(Clone, Debug)]
pub struct PactAppendBatch {
    pub source: String,
    pub batch_id: String,
    pub expected: PactFileVersion,
    pub previous_checkpoint: Option<String>,
    pub checkpoint: Option<String>,
    pub messages: Vec<PactAppendMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PactAppendReceipt {
    pub version: PactFileVersion,
    pub transaction_id: String,
    pub source: String,
    pub batch_id: String,
    pub request_digest: [u8; 32],
    pub checkpoint: Option<String>,
    pub row_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PactAppendOutcome {
    Committed(PactAppendReceipt),
    AlreadyCommitted(PactAppendReceipt),
}

impl PactAppendOutcome {
    pub fn receipt(&self) -> &PactAppendReceipt {
        match self {
            Self::Committed(receipt) | Self::AlreadyCommitted(receipt) => receipt,
        }
    }
}

#[derive(Debug)]
pub enum PactAppendFailure {
    InvalidInput(String),
    UnsupportedSemantics(String),
    Conflict {
        actual: PactFileVersion,
    },
    IdempotencyConflict,
    Corrupt(String),
    FormatLimit(String),
    ResourceUnavailable(String),
    RejectedIo(std::io::Error),
    OutcomeUnknown {
        source: String,
        batch_id: String,
        cause: std::io::Error,
    },
}

impl PactAppendFailure {
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::UnsupportedSemantics(_) => "unsupported_semantics",
            Self::Conflict { .. } => "conflict",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::Corrupt(_) => "corrupt",
            Self::FormatLimit(_) => "format_limit",
            Self::ResourceUnavailable(_) => "resource_unavailable",
            Self::RejectedIo(_) => "rejected_io",
            Self::OutcomeUnknown { .. } => "outcome_unknown",
        }
    }
}

impl fmt::Display for PactAppendFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid append input: {message}"),
            Self::UnsupportedSemantics(message) => {
                write!(f, "unsupported append semantics: {message}")
            }
            Self::Conflict { actual } => write!(
                f,
                "append conflict: file is at generation {} revision {} of branch {}",
                actual.generation, actual.revision, actual.branch_id
            ),
            Self::IdempotencyConflict => {
                write!(f, "append batch key was reused with different input")
            }
            Self::Corrupt(message) => write!(f, "corrupt Pact file: {message}"),
            Self::FormatLimit(message) => write!(f, "Pact format limit: {message}"),
            Self::ResourceUnavailable(message) => write!(f, "resource unavailable: {message}"),
            Self::RejectedIo(error) => write!(f, "rejected I/O: {error}"),
            Self::OutcomeUnknown {
                source,
                batch_id,
                cause,
            } => write!(
                f,
                "publication outcome unknown for source {source} batch {batch_id}: {cause}"
            ),
        }
    }
}

impl std::error::Error for PactAppendFailure {}

enum Identity {
    Native(NativeIdentity),
    Deterministic(DeterministicIdentity),
}

impl AuthoringIdentity for Identity {
    fn next_local_id(&mut self) -> String {
        match self {
            Self::Native(identity) => identity.next_local_id(),
            Self::Deterministic(identity) => identity.next_local_id(),
        }
    }

    fn next_node_id(&mut self, prefix: &str) -> String {
        match self {
            Self::Native(identity) => identity.next_node_id(prefix),
            Self::Deterministic(identity) => identity.next_node_id(prefix),
        }
    }

    fn next_timestamp_ns(&mut self) -> u64 {
        match self {
            Self::Native(identity) => identity.next_timestamp_ns(),
            Self::Deterministic(identity) => identity.next_timestamp_ns(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PactAppendOptions {
    pub derived_depth: bool,
}

pub struct PactAppendWriter {
    path: PathBuf,
    options: PactMemoryOptions,
    append_options: PactAppendOptions,
    branch_id: String,
    version: PactFileVersion,
    pinned: PinnedFile,

    state: Option<AppendState>,
    reconciliation_required: bool,
    _lock: WriterLock,
    counters: Arc<IoCounterSet>,
    identity: Identity,
    deterministic: Option<DeterministicAuthoringOptions>,
    fault: Option<FaultPoint>,
    maintenance_policy: MaintenancePolicy,
    automatic_maintenance: bool,
    maintenance_debt: Option<String>,
    last_maintenance: Option<PactMaintenanceReport>,
    maintenance_fault: Option<RepackStage>,
}

fn length_prefixed(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn batch_key(source: &str, batch_id: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    length_prefixed(&mut hasher, source);
    length_prefixed(&mut hasher, batch_id);
    format!("batch:{}", hasher.finalize().to_hex())
}

fn source_key(source: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    length_prefixed(&mut hasher, source);
    format!("source:{}", hasher.finalize().to_hex())
}

fn row_key(source: &str, row_id: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    length_prefixed(&mut hasher, source);
    length_prefixed(&mut hasher, row_id);
    format!("row:{}", hasher.finalize().to_hex())
}

fn record_id(source: &str, batch_id: &str) -> String {
    format!(
        "pactree.append.receipt:{}",
        &batch_key(source, batch_id)["batch:".len()..]
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_hex_32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (index, chunk) in text.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).ok()?;
        out[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

struct HashWriter(blake3::Hasher);

impl std::io::Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn request_digest(
    file_identity: &str,
    branch_id: &str,
    batch: &PactAppendBatch,
) -> PactResult<[u8; 32]> {
    let mut writer = HashWriter(blake3::Hasher::new());
    let rows = batch
        .messages
        .iter()
        .map(|message| {
            json!({
                "aux": message.aux,
                "parts": message.parts.iter().map(Component::project_component_value).collect::<Vec<_>>(),
                "role": message.role,
                "row_id": message.row_id,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_writer(
        &mut writer,
        &json!({
            "schema": PACT_APPEND_REQUEST_SCHEMA,
            "file_identity": file_identity,
            "branch_id": branch_id,
            "source": batch.source,
            "batch_id": batch.batch_id,
            "previous_checkpoint": batch.previous_checkpoint,
            "checkpoint": batch.checkpoint,
            "rows": rows,
        }),
    )
    .map_err(|error| PactError::codec(format!("encode Pact append request: {error}")))?;
    Ok(*writer.0.finalize().as_bytes())
}

fn receipt_record(
    file_identity: &str,
    branch_id: &str,
    generation: u64,
    revision: u64,
    transaction_id: &str,
    digest: &[u8; 32],
    batch: &PactAppendBatch,
) -> PactExtensionRecord {
    let row_ids = batch
        .messages
        .iter()
        .map(|message| message.row_id.clone())
        .collect::<Vec<_>>();
    let mut record = PactExtensionRecord::new(
        PACT_APPEND_RECEIPT_KIND,
        PACT_APPEND_RECEIPT_KIND,
        record_id(&batch.source, &batch.batch_id),
        json!({
            "schema": PACT_APPEND_RECEIPT_KIND,
            "request_digest": hex(digest),
            "file_identity": file_identity,
            "branch_id": branch_id,
            "generation": generation,
            "revision": revision,
            "transaction_id": transaction_id,
            "source": batch.source,
            "batch_id": batch.batch_id,
            "previous_checkpoint": batch.previous_checkpoint,
            "checkpoint": batch.checkpoint,
            "row_ids": row_ids,
        }),
    );
    let mut keys = vec![
        batch_key(&batch.source, &batch.batch_id),
        source_key(&batch.source),
    ];
    keys.extend(
        batch
            .messages
            .iter()
            .map(|message| row_key(&batch.source, &message.row_id)),
    );
    record.index_keys = keys;
    record
}

struct StoredReceipt {
    receipt: PactAppendReceipt,
    file_identity: String,
    branch_id: String,
}

fn parse_receipt(payload: &Value, current: &PactFileVersion) -> PactResult<StoredReceipt> {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| PactError::corrupt(format!("Pact append receipt lacks {key}")))
    };
    let digest = parse_hex_32(&text("request_digest")?)
        .ok_or_else(|| PactError::corrupt("Pact append receipt digest is not 32 bytes"))?;
    let revision = payload
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| PactError::corrupt("Pact append receipt lacks revision"))?;
    let generation = payload
        .get("generation")
        .and_then(Value::as_u64)
        .unwrap_or(current.generation);
    let row_ids = payload
        .get("row_ids")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let file_identity = text("file_identity")?;
    let branch_id = text("branch_id")?;
    Ok(StoredReceipt {
        receipt: PactAppendReceipt {
            version: PactFileVersion {
                file_identity: parse_hex_32(&file_identity).unwrap_or(current.file_identity),
                generation,
                branch_id: branch_id.clone(),
                revision,
            },
            transaction_id: text("transaction_id")?,
            source: text("source")?,
            batch_id: text("batch_id")?,
            request_digest: digest,
            checkpoint: payload
                .get("checkpoint")
                .and_then(Value::as_str)
                .map(str::to_string),
            row_ids,
        },
        file_identity,
        branch_id,
    })
}

const FORMAT_LIMIT_MARKERS: &[&str] = &[
    "format limit",
    "message order supports",
    "exceeds fixed limit",
    "exceeds the",
    "exceeds ",
];

impl PactAppendWriter {
    pub fn open(
        path: impl AsRef<Path>,
        branch: &str,
        options: PactMemoryOptions,
    ) -> Result<Self, PactAppendFailure> {
        Self::open_inner(
            path.as_ref(),
            branch,
            options,
            PactAppendOptions::default(),
            None,
        )
    }

    pub fn open_with_options(
        path: impl AsRef<Path>,
        branch: &str,
        options: PactMemoryOptions,
        append_options: PactAppendOptions,
    ) -> Result<Self, PactAppendFailure> {
        Self::open_inner(path.as_ref(), branch, options, append_options, None)
    }

    pub fn open_with_deterministic_authoring(
        path: impl AsRef<Path>,
        branch: &str,
        options: PactMemoryOptions,
        authoring: DeterministicAuthoringOptions,
    ) -> Result<Self, PactAppendFailure> {
        Self::open_inner(
            path.as_ref(),
            branch,
            options,
            PactAppendOptions::default(),
            Some(authoring),
        )
    }

    fn open_inner(
        path: &Path,
        branch: &str,
        options: PactMemoryOptions,
        append_options: PactAppendOptions,
        deterministic: Option<DeterministicAuthoringOptions>,
    ) -> Result<Self, PactAppendFailure> {
        options.validate().map_err(map_precommit)?;
        if branch.is_empty() {
            return Err(PactAppendFailure::InvalidInput(
                "branch id must not be empty".to_string(),
            ));
        }
        if !path.exists() {
            let mut tree = ContextTree::with_id_and_options(
                crate::context_tree::mint_context_tree_id(),
                ContextTreeOptions {
                    derived_depth: false,
                    deterministic_authoring: deterministic.clone(),
                },
            )
            .map_err(map_precommit)?;
            tree.save_file(path).map_err(map_precommit)?;
        }
        let canonical = path.canonicalize().map_err(PactAppendFailure::RejectedIo)?;
        let lock = WriterLock::acquire(&canonical).map_err(map_precommit)?;
        remove_stale_candidates(&canonical).map_err(map_precommit)?;
        let counters = Arc::new(IoCounterSet::default());
        let pinned = pin(&canonical, Arc::clone(&counters), &options).map_err(map_precommit)?;
        let identity = match &deterministic {
            Some(authoring) => Identity::Deterministic(
                DeterministicIdentity::new(authoring.clone()).map_err(map_precommit)?,
            ),
            None => Identity::Native(NativeIdentity::default()),
        };
        let mut writer = Self {
            path: canonical,
            options,
            append_options,
            branch_id: branch.to_string(),
            version: PactFileVersion {
                file_identity: [0; 32],
                generation: 0,
                branch_id: branch.to_string(),
                revision: 0,
            },
            pinned,
            state: None,
            reconciliation_required: false,
            _lock: lock,
            counters,
            identity,
            deterministic,
            fault: None,
            maintenance_policy: MaintenancePolicy::default(),
            automatic_maintenance: true,
            maintenance_debt: None,
            last_maintenance: None,
            maintenance_fault: None,
        };
        writer.refresh_version()?;
        writer.bootstrap_state()?;
        Ok(writer)
    }

    #[allow(dead_code)]
    pub(crate) fn set_fault_point(&mut self, fault: Option<FaultPoint>) {
        self.fault = fault;
    }

    fn refresh_version(&mut self) -> Result<(), PactAppendFailure> {
        let snapshot = &self.pinned.snapshot;
        let metadata = snapshot.metadata().map_err(map_precommit)?;
        let revision = metadata
            .head_revisions
            .get(&self.branch_id)
            .copied()
            .ok_or_else(|| {
                PactAppendFailure::InvalidInput(format!("branch not found: {}", self.branch_id))
            })?;
        let file_identity = parse_hex_32(&snapshot.source_file_identity()).ok_or_else(|| {
            PactAppendFailure::Corrupt("Pact file identity is not 32 bytes".to_string())
        })?;
        self.version = PactFileVersion {
            file_identity,
            generation: snapshot.generation(),
            branch_id: self.branch_id.clone(),
            revision,
        };
        Ok(())
    }

    fn bootstrap_state(&mut self) -> Result<(), PactAppendFailure> {
        match AppendState::bootstrap(&self.pinned.snapshot, &self.branch_id, &self.options) {
            Ok(mut state) => {
                if self.append_options.derived_depth {
                    state.derived_depth = true;
                }
                self.state = Some(state);
                Ok(())
            }
            Err(error)
                if self.version.revision == 0
                    && error.kind() == PactErrorKind::Unsupported
                    && error.to_string().contains("hollow") =>
            {
                self.state = None;
                Ok(())
            }
            Err(error) => Err(map_precommit(error)),
        }
    }

    fn repin(&mut self) -> Result<(), PactAppendFailure> {
        self.pinned =
            pin(&self.path, Arc::clone(&self.counters), &self.options).map_err(map_precommit)?;
        self.refresh_version()?;
        self.bootstrap_state()
    }

    fn repin_after_publish(&mut self) -> Result<(), PactAppendFailure> {
        self.pinned =
            pin(&self.path, Arc::clone(&self.counters), &self.options).map_err(map_precommit)?;
        self.refresh_version()?;
        match self.state.as_ref() {
            Some(state) if state.verify_pinned(&self.pinned.snapshot).is_ok() => Ok(()),
            _ => self.bootstrap_state(),
        }
    }

    pub fn version(&self) -> &PactFileVersion {
        &self.version
    }

    pub fn set_automatic_maintenance(&mut self, enabled: bool) {
        self.automatic_maintenance = enabled;
    }

    pub fn maintenance_debt(&self) -> Option<&str> {
        self.maintenance_debt.as_deref()
    }

    pub fn last_maintenance(&self) -> Option<&PactMaintenanceReport> {
        self.last_maintenance.as_ref()
    }

    pub fn maintain(&mut self) -> Result<PactMaintenanceReport, PactAppendFailure> {
        self.maintain_with_observer(&mut |_| Ok(()))
    }

    fn maintain_with_observer(
        &mut self,
        observer: &mut dyn FnMut(RepackStage) -> PactResult<()>,
    ) -> Result<PactMaintenanceReport, PactAppendFailure> {
        if self.reconciliation_required {
            return Err(PactAppendFailure::ResourceUnavailable(
                "the writer must be reopened after an unknown publication outcome".to_string(),
            ));
        }
        let assessment = assess(
            &self.pinned.snapshot,
            &self.options,
            &self.maintenance_policy,
        )
        .map_err(map_precommit)?;
        let mut report = PactMaintenanceReport {
            generation: assessment.generation,
            file_bytes_before: assessment.file_bytes,
            file_bytes_after: assessment.file_bytes,
            derived_amplification_permille: assessment.derived_amplification_permille,
            dead_page_permille: assessment.dead_page_permille,
            needed: assessment.needed,
            repacked: false,
            reasons: assessment
                .reasons
                .iter()
                .map(|reason| reason.to_string())
                .collect(),
        };
        if assessment.needed {
            let repacked = repack(&self.path, &self.pinned.snapshot, &self.options, observer)
                .map_err(map_precommit)?;
            report.file_bytes_after = repacked.bytes_after;
            report.repacked = true;
            self.repin()?;
        }
        self.maintenance_debt = None;
        self.last_maintenance = Some(report.clone());
        Ok(report)
    }

    #[allow(dead_code)]
    pub(crate) fn set_maintenance_fault(&mut self, stage: Option<RepackStage>) {
        self.maintenance_fault = stage;
    }

    #[allow(dead_code)]
    pub(crate) fn set_maintenance_policy(&mut self, policy: MaintenancePolicy) {
        self.maintenance_policy = policy;
    }

    fn maintain_after_commit(&mut self) {
        if !self.automatic_maintenance {
            return;
        }
        let fault = self.maintenance_fault;
        let result = self.maintain_with_observer(&mut |stage| {
            if fault == Some(stage) {
                Err(PactError::Io(std::io::Error::from_raw_os_error(28)))
            } else {
                Ok(())
            }
        });
        if let Err(failure) = result {
            self.maintenance_debt = Some(failure.to_string());
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn checkpoint(&self, source: &str) -> Result<Option<String>, PactAppendFailure> {
        let Some(record) = self
            .latest_record_for_key(&source_key(source))
            .map_err(map_precommit)?
        else {
            return Ok(None);
        };
        let stored = parse_receipt(&record.payload, &self.version).map_err(map_precommit)?;
        if stored.receipt.source != source {
            return Err(PactAppendFailure::Corrupt(
                "Pact append receipt source hash collision".to_string(),
            ));
        }
        Ok(stored.receipt.checkpoint)
    }

    pub fn latest_receipt(
        &self,
        source: &str,
    ) -> Result<Option<PactAppendReceipt>, PactAppendFailure> {
        let Some(record) = self
            .latest_record_for_key(&source_key(source))
            .map_err(map_precommit)?
        else {
            return Ok(None);
        };
        let stored = parse_receipt(&record.payload, &self.version).map_err(map_precommit)?;
        if stored.receipt.source != source {
            return Err(PactAppendFailure::Corrupt(
                "Pact append receipt source hash collision".to_string(),
            ));
        }
        if stored.file_identity != hex(&self.version.file_identity)
            || stored.branch_id != self.branch_id
        {
            return Err(PactAppendFailure::Corrupt(
                "Pact append receipt names another file or branch".to_string(),
            ));
        }
        Ok(Some(stored.receipt))
    }

    pub fn lookup(
        &self,
        source: &str,
        batch_id: &str,
    ) -> Result<Option<PactAppendReceipt>, PactAppendFailure> {
        let Some(record) = self
            .latest_record_for_key(&batch_key(source, batch_id))
            .map_err(map_precommit)?
        else {
            return Ok(None);
        };
        let stored = parse_receipt(&record.payload, &self.version).map_err(map_precommit)?;
        if stored.receipt.source != source || stored.receipt.batch_id != batch_id {
            return Err(PactAppendFailure::Corrupt(
                "Pact append receipt batch key collision".to_string(),
            ));
        }
        if stored.file_identity != hex(&self.version.file_identity)
            || stored.branch_id != self.branch_id
        {
            return Err(PactAppendFailure::Corrupt(
                "Pact append receipt names another file or branch".to_string(),
            ));
        }
        Ok(Some(stored.receipt))
    }

    fn latest_record_for_key(&self, key: &str) -> PactResult<Option<PactExtensionRecord>> {
        let snapshot = &self.pinned.snapshot;
        let key_hash = IndexKey::extension_key_hash(key);
        let start = IndexKey::ExtensionKey {
            branch: self.branch_id.clone(),
            kind: PACT_APPEND_RECEIPT_KIND.to_string(),
            key_hash,
            revision: 0,
            ordinal: 0,
        };
        let end = IndexKey::ExtensionKey {
            branch: self.branch_id.clone(),
            kind: PACT_APPEND_RECEIPT_KIND.to_string(),
            key_hash,
            revision: u64::MAX,
            ordinal: u64::MAX,
        };
        let entries = snapshot.index_range(&start, &end, 1, true)?;
        let Some((_, value)) = entries.into_iter().next() else {
            return Ok(None);
        };
        let locator: crate::extension::IndexedExtensionLocator = serde_json::from_slice(&value)
            .map_err(|error| {
                PactError::corrupt(format!("invalid Pact extension locator: {error}"))
            })?;
        let row =
            crate::pact_file::extension::record(snapshot, &self.branch_id, &locator.record_id)?;
        Ok(Some(row.record))
    }

    fn row_is_recorded(&self, source: &str, row_id: &str) -> PactResult<bool> {
        Ok(self
            .latest_record_for_key(&row_key(source, row_id))?
            .is_some())
    }

    fn validate_batch(&self, batch: &PactAppendBatch) -> Result<(), PactAppendFailure> {
        let invalid = |message: &str| PactAppendFailure::InvalidInput(message.to_string());
        if batch.source.is_empty() || batch.source.bytes().any(|byte| byte == 0) {
            return Err(invalid("source must be a nonempty string without NUL"));
        }
        if batch.batch_id.is_empty() || batch.batch_id.bytes().any(|byte| byte == 0) {
            return Err(invalid("batch_id must be a nonempty string without NUL"));
        }
        if batch.messages.is_empty() {
            return Err(invalid(
                "an empty batch cannot advance a checkpoint without rows",
            ));
        }
        let mut rows = BTreeSet::new();
        for message in &batch.messages {
            if message.row_id.is_empty() {
                return Err(invalid("row_id must not be empty"));
            }
            if !rows.insert(message.row_id.as_str()) {
                return Err(PactAppendFailure::InvalidInput(format!(
                    "duplicate row_id in batch: {}",
                    message.row_id
                )));
            }
            if !matches!(message.role.as_str(), "system" | "user" | "assistant") {
                return Err(PactAppendFailure::InvalidInput(format!(
                    "unsupported message role: {}",
                    message.role
                )));
            }
            if message.parts.is_empty() {
                return Err(invalid("message must contain at least one component"));
            }
        }
        if batch.expected.branch_id != self.branch_id {
            return Err(PactAppendFailure::Conflict {
                actual: self.version.clone(),
            });
        }
        Ok(())
    }

    pub fn append(
        &mut self,
        batch: PactAppendBatch,
    ) -> Result<PactAppendOutcome, PactAppendFailure> {
        if self.reconciliation_required {
            return Err(PactAppendFailure::ResourceUnavailable(
                "the writer must be reopened after an unknown publication outcome".to_string(),
            ));
        }
        self.validate_batch(&batch)?;
        let identity_hex = hex(&self.version.file_identity);
        let digest =
            request_digest(&identity_hex, &self.branch_id, &batch).map_err(map_precommit)?;

        if let Some(receipt) = self.lookup(&batch.source, &batch.batch_id)? {
            if receipt.request_digest == digest
                && receipt.version.file_identity == self.version.file_identity
                && receipt.version.branch_id == self.branch_id
            {
                return Ok(PactAppendOutcome::AlreadyCommitted(receipt));
            }
            return Err(PactAppendFailure::IdempotencyConflict);
        }

        if batch.expected != self.version {
            return Err(PactAppendFailure::Conflict {
                actual: self.version.clone(),
            });
        }
        let current_checkpoint = self.checkpoint(&batch.source)?;
        if batch.previous_checkpoint != current_checkpoint {
            return Err(PactAppendFailure::Conflict {
                actual: self.version.clone(),
            });
        }
        for message in &batch.messages {
            if self
                .row_is_recorded(&batch.source, &message.row_id)
                .map_err(map_precommit)?
            {
                return Err(PactAppendFailure::IdempotencyConflict);
            }
        }

        let revision = self.version.revision + 1;
        let generation = self.version.generation + 1;
        let transaction_id = format!("tx_{}_{}", self.branch_id, revision);
        let receipt = receipt_record(
            &identity_hex,
            &self.branch_id,
            generation,
            revision,
            &transaction_id,
            &digest,
            &batch,
        );
        receipt.validate().map_err(map_precommit)?;

        let outcome = if self.state.is_none() {
            self.append_through_eager_path(&batch, receipt)
        } else {
            self.append_bounded(&batch, receipt)
        };
        match outcome {
            Ok(()) => {
                let receipt = self
                    .lookup(&batch.source, &batch.batch_id)?
                    .ok_or_else(|| {
                        PactAppendFailure::Corrupt(
                            "committed batch has no readable receipt".to_string(),
                        )
                    })?;
                self.maintain_after_commit();
                Ok(PactAppendOutcome::Committed(receipt))
            }
            Err(failure) => Err(failure),
        }
    }

    fn append_through_eager_path(
        &mut self,
        batch: &PactAppendBatch,
        receipt: PactExtensionRecord,
    ) -> Result<(), PactAppendFailure> {
        let options = ContextTreeOptions {
            derived_depth: false,
            deterministic_authoring: self.deterministic.clone(),
        };
        let mut tree =
            ContextTree::open_file_with_options(&self.path, Some(&self.branch_id), options)
                .map_err(map_precommit)?;
        tree.set_automatic_compaction(self.automatic_maintenance);
        for message in &batch.messages {
            tree.add_message(&message.role, message.parts.clone(), message.aux.clone())
                .map_err(map_precommit)?;
        }
        tree.stage_extensions(vec![receipt])
            .map_err(map_precommit)?;
        let committed = tree
            .seal_staged(crate::trace::BoundaryKind::Adhoc)
            .map_err(|error| match error {
                PactError::Io(error) => PactAppendFailure::OutcomeUnknown {
                    source: batch.source.clone(),
                    batch_id: batch.batch_id.clone(),
                    cause: error,
                },
                other => map_precommit(other),
            })?;
        drop(tree);
        if committed.is_none() {
            return Err(PactAppendFailure::InvalidInput(
                "the batch produced no accepted operation".to_string(),
            ));
        }
        self.repin()
    }

    fn append_bounded(
        &mut self,
        batch: &PactAppendBatch,
        receipt: PactExtensionRecord,
    ) -> Result<(), PactAppendFailure> {
        let messages = batch
            .messages
            .iter()
            .map(|message| AppendMessage {
                role: message.role.clone(),
                parts: message.parts.clone(),
                aux: message.aux.clone(),
            })
            .collect::<Vec<_>>();
        let state = self.state.as_mut().ok_or_else(|| {
            PactAppendFailure::ResourceUnavailable(
                "bounded writer state is unavailable".to_string(),
            )
        })?;
        let prepared = state.prepare(
            &self.pinned.snapshot,
            messages,
            vec![receipt],
            &mut self.identity,
            &self.options,
        );
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.bootstrap_state()?;
                return Err(map_precommit(error));
            }
        };
        let published = publish(
            &self.path,
            &self.pinned,
            state,
            &prepared,
            &self.options,
            &self.counters,
            self.fault,
        );
        match published {
            Ok(_) => self.repin_after_publish(),
            Err(PublishError::Rejected(error)) => {
                let failure = self.map_rejected(error);
                self.repin()?;
                Err(failure)
            }
            Err(PublishError::Unknown(cause)) => {
                self.reconciliation_required = true;
                Err(PactAppendFailure::OutcomeUnknown {
                    source: batch.source.clone(),
                    batch_id: batch.batch_id.clone(),
                    cause,
                })
            }
        }
    }

    pub fn repair_message_order(&mut self) -> Result<PactMessageOrderRepair, PactAppendFailure> {
        if self.reconciliation_required {
            return Err(PactAppendFailure::ResourceUnavailable(
                "the writer must be reopened after an unknown publication outcome".to_string(),
            ));
        }
        if self.state.is_none() {
            return Ok(PactMessageOrderRepair {
                transaction_id: None,
                revision: self.version.revision,
                rows_scanned: 0,
                rows_reindexed: 0,
                stale_keys_removed: 0,
            });
        }
        let plan = plan_message_order_repair(&self.pinned.snapshot, &self.branch_id, &self.options)
            .map_err(map_precommit)?;
        self.commit_message_order_plan(plan)
    }

    pub(crate) fn commit_message_order_plan(
        &mut self,
        plan: MessageOrderRepairPlan,
    ) -> Result<PactMessageOrderRepair, PactAppendFailure> {
        let rows_scanned = plan.rows_scanned;
        let rows_reindexed = plan.rows_to_reindex;
        let stale_keys_removed = plan.stale_keys;
        if plan.is_empty() {
            return Ok(PactMessageOrderRepair {
                transaction_id: None,
                revision: self.version.revision,
                rows_scanned,
                rows_reindexed: 0,
                stale_keys_removed: 0,
            });
        }
        let marker = (
            PACT_MESSAGE_ORDER_REPAIR_MARKER.to_string(),
            serde_json::json!({
                "schema": PACT_MESSAGE_ORDER_REPAIR_MARKER,
                "rows_scanned": rows_scanned,
                "rows_reindexed": rows_reindexed,
                "stale_keys_removed": stale_keys_removed,
            }),
        );
        let state = self.state.as_mut().ok_or_else(|| {
            PactAppendFailure::ResourceUnavailable(
                "bounded writer state is unavailable".to_string(),
            )
        })?;
        let prepared =
            state.prepare_maintenance(&self.pinned.snapshot, &self.options, plan, marker);
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.bootstrap_state()?;
                return Err(map_precommit(error));
            }
        };
        let published = publish(
            &self.path,
            &self.pinned,
            state,
            &prepared,
            &self.options,
            &self.counters,
            self.fault,
        );
        match published {
            Ok(_) => self.repin_after_publish()?,
            Err(PublishError::Rejected(error)) => {
                let failure = self.map_rejected(error);
                self.repin()?;
                return Err(failure);
            }
            Err(PublishError::Unknown(cause)) => {
                self.reconciliation_required = true;
                return Err(PactAppendFailure::OutcomeUnknown {
                    source: "pact.maintenance".to_string(),
                    batch_id: PACT_MESSAGE_ORDER_REPAIR_MARKER.to_string(),
                    cause,
                });
            }
        }
        Ok(PactMessageOrderRepair {
            transaction_id: Some(prepared.transaction_id),
            revision: self.version.revision,
            rows_scanned,
            rows_reindexed,
            stale_keys_removed,
        })
    }

    fn map_rejected(&mut self, error: PactError) -> PactAppendFailure {
        let message = error.to_string();
        let conflict = error.kind() == PactErrorKind::Invalid
            && (message.contains("stale")
                || message.contains("advanced under the writer")
                || message.contains("under the writer"));
        if conflict {
            if let Err(failure) = self.repin() {
                return failure;
            }
            return PactAppendFailure::Conflict {
                actual: self.version.clone(),
            };
        }
        map_precommit(error)
    }

    pub fn close(self) {}
}

fn map_precommit(error: PactError) -> PactAppendFailure {
    let message = error.to_string();
    match error.kind() {
        PactErrorKind::Invalid
            if message.contains("Pact extension") && message.contains("exceeds") =>
        {
            PactAppendFailure::FormatLimit(message)
        }
        PactErrorKind::Invalid | PactErrorKind::NotFound | PactErrorKind::Resolver => {
            PactAppendFailure::InvalidInput(message)
        }
        PactErrorKind::Unsupported => {
            if FORMAT_LIMIT_MARKERS
                .iter()
                .any(|marker| message.contains(marker))
            {
                PactAppendFailure::FormatLimit(message)
            } else {
                PactAppendFailure::UnsupportedSemantics(message)
            }
        }
        PactErrorKind::Storage => PactAppendFailure::ResourceUnavailable(message),
        PactErrorKind::Io => match error {
            PactError::Io(error) => PactAppendFailure::RejectedIo(error),
            other => PactAppendFailure::RejectedIo(std::io::Error::other(other.to_string())),
        },
        _ => PactAppendFailure::Corrupt(message),
    }
}
