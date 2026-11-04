use std::path::Path;

use crate::PactResult;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct PhysicalLayoutMetrics {
    pub(crate) file_bytes: u64,
    pub(crate) envelope_bytes: u64,
    pub(crate) canonical_bytes: u64,
    pub(crate) checkpoint_bytes: u64,
    pub(crate) content_bytes: u64,
    pub(crate) facet_bytes: u64,
    pub(crate) directory_bytes: u64,
    pub(crate) allocator_bytes: u64,
    pub(crate) commit_bytes: u64,
    pub(crate) derived_amplification: f64,
    pub(crate) compaction_whole_ledger_hydrations: u64,
}

impl PhysicalLayoutMetrics {
    pub(crate) fn accounted_bytes(&self) -> u64 {
        self.envelope_bytes
            + self.canonical_bytes
            + self.checkpoint_bytes
            + self.content_bytes
            + self.facet_bytes
            + self.directory_bytes
            + self.allocator_bytes
            + self.commit_bytes
    }
}

pub(crate) fn measure_committed_layout(bytes: &[u8]) -> PactResult<PhysicalLayoutMetrics> {
    super::codec::measure_record_layout(bytes)
}

pub(crate) fn measure_path(path: &Path) -> PactResult<PhysicalLayoutMetrics> {
    measure_committed_layout(&std::fs::read(path)?)
}
