use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::transaction::PactTransaction;
use crate::error::PactResult;

#[cfg(test)]
mod tests;

pub(crate) trait PactFileBackend {
    type Snapshot;

    fn create(&self, path: &Path, genesis: &PactTransaction) -> PactResult<CommitReceipt>;
    fn open_snapshot(&self, path: &Path) -> PactResult<Self::Snapshot>;
    fn commit(&self, path: &Path, transaction: &PactTransaction) -> PactResult<CommitReceipt>;
    fn validate(&self, path: &Path) -> PactResult<ValidationReport>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommitReceipt {
    pub(crate) generation: u64,
    pub(crate) transaction_address: u64,
    pub(crate) index_root_address: u64,
    pub(crate) committed_len: u64,
    pub(crate) counters: IoCounters,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ValidationReport {
    pub(crate) ok: bool,
    pub(crate) errors: Vec<String>,
    pub(crate) counters: IoCounters,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IoCounters {
    pub(crate) bytes_read: u64,
    pub(crate) bytes_written: u64,
    pub(crate) pages_read: u64,
    pub(crate) pages_written: u64,
    pub(crate) records_decoded: u64,
    pub(crate) operations_replayed: u64,
    pub(crate) rows_materialized: u64,
    pub(crate) mappings_touched: u64,
    pub(crate) components_hydrated: u64,
    pub(crate) content_ranges_hydrated: u64,
}

#[derive(Default)]
pub(crate) struct IoCounterSet {
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    pages_read: AtomicU64,
    pages_written: AtomicU64,
    records_decoded: AtomicU64,
    operations_replayed: AtomicU64,
    rows_materialized: AtomicU64,
    mappings_touched: AtomicU64,
    components_hydrated: AtomicU64,
    content_ranges_hydrated: AtomicU64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PactStorageCounters {
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub pages_read: u64,
    pub pages_written: u64,
    pub records_decoded: u64,
    pub operations_replayed: u64,
    pub rows_materialized: u64,
    pub mappings_touched: u64,
    pub components_hydrated: u64,
    pub content_ranges_hydrated: u64,

    pub ledger_hydrations: u64,
}

static GLOBAL_COUNTERS: std::sync::OnceLock<IoCounterSet> = std::sync::OnceLock::new();
static LEDGER_HYDRATIONS: AtomicU64 = AtomicU64::new(0);

fn global_counters() -> &'static IoCounterSet {
    GLOBAL_COUNTERS.get_or_init(IoCounterSet::default)
}

pub fn storage_counters() -> PactStorageCounters {
    let counters = global_counters().snapshot();
    PactStorageCounters {
        bytes_read: counters.bytes_read,
        bytes_written: counters.bytes_written,
        pages_read: counters.pages_read,
        pages_written: counters.pages_written,
        records_decoded: counters.records_decoded,
        operations_replayed: counters.operations_replayed,
        rows_materialized: counters.rows_materialized,
        mappings_touched: counters.mappings_touched,
        components_hydrated: counters.components_hydrated,
        content_ranges_hydrated: counters.content_ranges_hydrated,
        ledger_hydrations: LEDGER_HYDRATIONS.load(Ordering::Relaxed),
    }
}

pub fn reset_storage_counters() {
    global_counters().reset();
    LEDGER_HYDRATIONS.store(0, Ordering::Relaxed);
}

pub(crate) fn record_ledger_hydration() {
    LEDGER_HYDRATIONS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_global(delta: &IoCounters) {
    global_counters().add(delta);
}

impl IoCounterSet {
    pub(crate) fn record(&self, delta: &IoCounters) {
        self.add(delta);
        let global = global_counters();
        if !std::ptr::eq(self, global) {
            global.add(delta);
        }
    }

    fn reset(&self) {
        for counter in [
            &self.bytes_read,
            &self.bytes_written,
            &self.pages_read,
            &self.pages_written,
            &self.records_decoded,
            &self.operations_replayed,
            &self.rows_materialized,
            &self.mappings_touched,
            &self.components_hydrated,
            &self.content_ranges_hydrated,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }

    fn add(&self, delta: &IoCounters) {
        self.bytes_read
            .fetch_add(delta.bytes_read, Ordering::Relaxed);
        self.bytes_written
            .fetch_add(delta.bytes_written, Ordering::Relaxed);
        self.pages_read
            .fetch_add(delta.pages_read, Ordering::Relaxed);
        self.pages_written
            .fetch_add(delta.pages_written, Ordering::Relaxed);
        self.records_decoded
            .fetch_add(delta.records_decoded, Ordering::Relaxed);
        self.operations_replayed
            .fetch_add(delta.operations_replayed, Ordering::Relaxed);
        self.rows_materialized
            .fetch_add(delta.rows_materialized, Ordering::Relaxed);
        self.mappings_touched
            .fetch_add(delta.mappings_touched, Ordering::Relaxed);
        self.components_hydrated
            .fetch_add(delta.components_hydrated, Ordering::Relaxed);
        self.content_ranges_hydrated
            .fetch_add(delta.content_ranges_hydrated, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> IoCounters {
        IoCounters {
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            pages_read: self.pages_read.load(Ordering::Relaxed),
            pages_written: self.pages_written.load(Ordering::Relaxed),
            records_decoded: self.records_decoded.load(Ordering::Relaxed),
            operations_replayed: self.operations_replayed.load(Ordering::Relaxed),
            rows_materialized: self.rows_materialized.load(Ordering::Relaxed),
            mappings_touched: self.mappings_touched.load(Ordering::Relaxed),
            components_hydrated: self.components_hydrated.load(Ordering::Relaxed),
            content_ranges_hydrated: self.content_ranges_hydrated.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
pub(crate) struct CandidateBackend {
    counters: std::sync::Arc<IoCounterSet>,
}

impl PactFileBackend for CandidateBackend {
    type Snapshot = super::snapshot::Snapshot;

    fn create(&self, path: &Path, genesis: &PactTransaction) -> PactResult<CommitReceipt> {
        super::log::create(path, genesis, &self.counters, None)
    }

    fn open_snapshot(&self, path: &Path) -> PactResult<Self::Snapshot> {
        super::snapshot::Snapshot::open(path, std::sync::Arc::clone(&self.counters))
    }

    fn commit(&self, path: &Path, transaction: &PactTransaction) -> PactResult<CommitReceipt> {
        super::log::commit_with_fault(path, transaction, &self.counters, None)
    }

    fn validate(&self, path: &Path) -> PactResult<ValidationReport> {
        super::validate::fast_validate(path)
    }
}
