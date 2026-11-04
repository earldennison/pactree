use std::path::Path;

use crate::error::PactResult;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CompactionPolicy {
    pub(crate) max_derived_amplification_permille: u16,
    pub(crate) max_hot_segments: usize,
    pub(crate) max_compaction_input_bytes: u64,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            max_derived_amplification_permille: 1_250,
            max_hot_segments: 16,
            max_compaction_input_bytes: 8 * 1024 * 1024,
        }
    }
}

pub(crate) fn maintain_after_append(
    path: &Path,
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<()> {
    maintain_after_append_with_policy(path, ledger, CompactionPolicy::default())
}

fn maintain_after_append_with_policy(
    path: &Path,
    ledger: &crate::store::ContextTreeLedger,
    policy: CompactionPolicy,
) -> PactResult<()> {
    if ledger.branches.len() != 1 {
        return Ok(());
    }

    if ledger.is_lazy() {
        return Ok(());
    }
    let file_len = std::fs::metadata(path)?.len();
    if file_len > policy.max_compaction_input_bytes {
        return Ok(());
    }
    let bytes = std::fs::read(path)?;
    let selected = super::root_slot::select_root_slot(&bytes)?;
    let metrics = super::layout_metrics::measure_committed_layout(&bytes)?;
    let amplification_permille = (metrics.derived_amplification * 1000.0) as u64;
    let needs_maintenance = selected.slot.generation as usize > policy.max_hot_segments
        || file_len > 1_000_000
        || amplification_permille > u64::from(policy.max_derived_amplification_permille);
    if !needs_maintenance {
        return Ok(());
    }

    let file_identity = super::envelope::file_identity(&bytes)?;
    let compacted = super::encode_cold_file_baseline_with_identity(ledger, file_identity)?;
    if compacted.len() >= bytes.len() {
        return Ok(());
    }
    crate::atomic_output::publish_bytes(
        path,
        &compacted,
        crate::atomic_output::PublishMode::ReplaceExisting,
    )
}
