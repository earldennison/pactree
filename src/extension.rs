use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{PactError, PactResult};
use crate::store::PactTransactionTrace;

#[cfg(test)]
mod tests;

pub const PACT_EXTENSIONS_AUX_KEY: &str = "pact.extensions";
pub const PACT_EXTENSION_PAGE_SCHEMA: &str = "pact.extension.page.v1";
const MAX_EXTENSION_RECORDS_PER_TRANSACTION: usize = 16_384;
const MAX_EXTENSION_RECORD_BYTES: usize = 1 << 20;
const MAX_EXTENSION_TRANSACTION_BYTES: usize = 16 << 20;
const MAX_EXTENSION_INDEX_KEYS: usize = 16_384;
const MAX_EXTENSION_INDEX_KEY_BYTES: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactExtensionRecord {
    pub schema: String,
    pub kind: String,
    pub record_id: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub content_refs: Vec<String>,
    #[serde(default)]
    pub external_refs: Vec<Value>,

    #[serde(default)]
    pub index_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ns: Option<u64>,
}

impl PactExtensionRecord {
    pub fn new(
        kind: impl Into<String>,
        schema: impl Into<String>,
        record_id: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            schema: schema.into(),
            kind: kind.into(),
            record_id: record_id.into(),
            payload,
            content_refs: Vec::new(),
            external_refs: Vec::new(),
            index_keys: Vec::new(),
            created_at_ns: None,
        }
    }

    pub fn validate(&self) -> PactResult<()> {
        validate_identity("extension kind", &self.kind)?;
        validate_identity("extension schema", &self.schema)?;
        validate_identity("extension record id", &self.record_id)?;
        if self.content_refs.iter().any(|value| value.is_empty()) {
            return Err(PactError::invalid(
                "Pact extension content refs must not be empty",
            ));
        }
        if self.index_keys.len() > MAX_EXTENSION_INDEX_KEYS {
            return Err(PactError::invalid(format!(
                "Pact extension record exceeds {MAX_EXTENSION_INDEX_KEYS} index keys"
            )));
        }
        let mut index_keys = BTreeSet::new();
        for key in &self.index_keys {
            if key.is_empty() {
                return Err(PactError::invalid(
                    "Pact extension index keys must not be empty",
                ));
            }
            if key.len() > MAX_EXTENSION_INDEX_KEY_BYTES {
                return Err(PactError::invalid(format!(
                    "Pact extension index key exceeds {MAX_EXTENSION_INDEX_KEY_BYTES} bytes"
                )));
            }
            if !index_keys.insert(key) {
                return Err(PactError::invalid(format!(
                    "duplicate Pact extension index key: {key}"
                )));
            }
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| PactError::codec(format!("invalid Pact extension: {error}")))?;
        if bytes.len() > MAX_EXTENSION_RECORD_BYTES {
            return Err(PactError::invalid(format!(
                "Pact extension record exceeds {MAX_EXTENSION_RECORD_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactExtensionCursor {
    pub revision: u64,
    pub ordinal: u64,
    pub record_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactExtensionRow {
    pub branch_id: String,
    pub revision: u64,
    pub transaction_id: String,
    pub ordinal: u64,
    pub record: PactExtensionRecord,
}

impl PactExtensionRow {
    pub fn cursor(&self) -> PactExtensionCursor {
        PactExtensionCursor {
            revision: self.revision,
            ordinal: self.ordinal,
            record_id: self.record.record_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactExtensionPage {
    pub schema: String,
    pub branch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_key: Option<String>,
    pub rows: Vec<PactExtensionRow>,
    pub has_before: bool,
    pub has_after: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_cursor: Option<PactExtensionCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_cursor: Option<PactExtensionCursor>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactExtensionCommit {
    pub branch_id: String,
    pub revision: u64,
    pub transaction_id: String,
}

pub struct PactExtensionWriter {
    path: PathBuf,
    branch_id: String,
}

impl PactExtensionWriter {
    pub fn open(path: impl AsRef<Path>, branch_id: Option<&str>) -> PactResult<Self> {
        let path = path.as_ref().to_path_buf();
        let handle = crate::PactFileReadHandle::open(&path, branch_id)?;
        let info = handle.info()?;
        let branch_id = info["branch_id"]
            .as_str()
            .ok_or_else(|| PactError::corrupt("Pact head branch id is not a string"))?
            .to_string();
        Ok(Self { path, branch_id })
    }

    pub fn append(&self, records: Vec<PactExtensionRecord>) -> PactResult<PactExtensionCommit> {
        crate::pact_file::append_extension_records(&self.path, &self.branch_id, records)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn branch_id(&self) -> &str {
        &self.branch_id
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct IndexedExtensionValue {
    pub(crate) branch_id: String,
    pub(crate) revision: u64,
    pub(crate) ordinal: u64,
    pub(crate) transaction_id: String,
    pub(crate) record: PactExtensionRecord,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct IndexedExtensionLocator {
    pub(crate) record_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IndexedExtensionRecord {
    pub(crate) branch_id: String,
    pub(crate) revision: u64,
    pub(crate) ordinal: u64,
    pub(crate) transaction_id: String,
    pub(crate) record: PactExtensionRecord,
}

pub(crate) fn validate_extension_batch(records: &[PactExtensionRecord]) -> PactResult<()> {
    if records.is_empty() {
        return Err(PactError::invalid(
            "Pact extension batch must contain at least one record",
        ));
    }
    if records.len() > MAX_EXTENSION_RECORDS_PER_TRANSACTION {
        return Err(PactError::invalid(format!(
            "Pact extension batch exceeds {MAX_EXTENSION_RECORDS_PER_TRANSACTION} records"
        )));
    }
    let mut ids = BTreeSet::new();
    let mut total = 0usize;
    for record in records {
        record.validate()?;
        if !ids.insert(record.record_id.as_str()) {
            return Err(PactError::invalid(format!(
                "duplicate Pact extension record id: {}",
                record.record_id
            )));
        }
        total = total.saturating_add(
            serde_json::to_vec(record)
                .map_err(|error| PactError::codec(format!("invalid Pact extension: {error}")))?
                .len(),
        );
    }
    if total > MAX_EXTENSION_TRANSACTION_BYTES {
        return Err(PactError::invalid(format!(
            "Pact extension batch exceeds {MAX_EXTENSION_TRANSACTION_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn records_from_trace(
    trace: &PactTransactionTrace,
) -> PactResult<Vec<PactExtensionRecord>> {
    records_from_aux(&trace.aux)
}

pub(crate) fn records_from_aux(
    aux: &BTreeMap<String, Value>,
) -> PactResult<Vec<PactExtensionRecord>> {
    let Some(value) = aux.get(PACT_EXTENSIONS_AUX_KEY) else {
        return Ok(Vec::new());
    };
    let records = serde_json::from_value::<Vec<PactExtensionRecord>>(value.clone())
        .map_err(|error| PactError::corrupt(format!("invalid Pact extension records: {error}")))?;
    validate_extension_batch(&records)
        .map_err(|error| PactError::corrupt(format!("invalid Pact extension batch: {error}")))?;
    Ok(records)
}

pub(crate) fn trace_revision(trace: &PactTransactionTrace) -> Option<u64> {
    trace
        .post_response_ref
        .as_ref()
        .and_then(|value| value.revision)
        .or(trace.post_request_alteration_ref.revision)
        .or_else(|| trace.replay_trace.as_ref().map(|value| value.to_rev))
        .or(trace.authored_ref.revision)
}

pub(crate) fn indexed_records_from_transaction(
    transaction: &crate::pact_file::transaction::PactTransaction,
) -> PactResult<Vec<IndexedExtensionRecord>> {
    let mut traces = Vec::<PactTransactionTrace>::new();
    if let Some(checkpoint) = transaction.head_checkpoint.as_ref() {
        let ledger = checkpoint.get("ledger").unwrap_or(checkpoint);
        if let Some(value) = ledger.pointer("/event_index/transaction_traces") {
            let baseline =
                serde_json::from_value::<BTreeMap<String, PactTransactionTrace>>(value.clone())
                    .map_err(|error| {
                        PactError::corrupt(format!(
                            "invalid baseline Pact extension traces: {error}"
                        ))
                    })?;
            traces.extend(baseline.into_values());
        }
    }
    traces.push(transaction.public_trace.clone());
    traces.sort_by(|left, right| {
        (
            left.branch_id.as_str(),
            trace_revision(left).unwrap_or_default(),
            left.transaction_id.as_str(),
        )
            .cmp(&(
                right.branch_id.as_str(),
                trace_revision(right).unwrap_or_default(),
                right.transaction_id.as_str(),
            ))
    });

    let mut seen = BTreeSet::new();
    let mut indexed = Vec::new();
    for trace in traces {
        let records = records_from_trace(&trace)?;
        let Some(revision) = trace_revision(&trace) else {
            if records.is_empty() {
                continue;
            }
            return Err(PactError::corrupt(format!(
                "Pact extension trace {} has no logical revision",
                trace.transaction_id
            )));
        };
        for (ordinal, record) in records.into_iter().enumerate() {
            let ordinal = u64::try_from(ordinal)
                .map_err(|_| PactError::invalid("Pact extension ordinal overflow"))?;
            let identity = (trace.branch_id.clone(), revision, record.record_id.clone());
            if !seen.insert(identity) {
                continue;
            }
            indexed.push(IndexedExtensionRecord {
                branch_id: trace.branch_id.clone(),
                revision,
                ordinal,
                transaction_id: trace.transaction_id.clone(),
                record,
            });
        }
    }
    Ok(indexed)
}

fn validate_identity(label: &str, value: &str) -> PactResult<()> {
    if value.is_empty() {
        return Err(PactError::invalid(format!("{label} must not be empty")));
    }
    if value.len() > 512 {
        return Err(PactError::invalid(format!("{label} exceeds 512 bytes")));
    }
    if value.bytes().any(|byte| byte == 0) {
        return Err(PactError::invalid(format!("{label} contains NUL")));
    }
    Ok(())
}
