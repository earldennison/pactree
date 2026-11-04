use crate::store::{ContentRecord, PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;
use crate::{PactError, PactResult};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PactTransaction {
    pub(crate) transaction_id: String,
    pub(crate) branch_id: String,
    pub(crate) boundary: BoundaryKind,
    pub(crate) trace: TransactionTrace,
    pub(crate) public_trace: PactTransactionTrace,
    pub(crate) new_content: Vec<ContentRecord>,
    pub(crate) head_checkpoint: Option<serde_json::Value>,
    pub(crate) ledger_delta: Option<LedgerDelta>,
    pub(crate) head_metadata: Option<super::index::HeadMetadata>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct LedgerDelta {
    pub(crate) lifecycle_tick: u64,
    pub(crate) lifecycle_birth_tick: u64,
    pub(crate) base_ref: PactRef,
    pub(crate) first_seen_additions: BTreeMap<String, u64>,
    pub(crate) lifecycle_seen_additions: BTreeMap<String, u64>,
    pub(crate) lifecycle_seen_removals: Vec<String>,
    pub(crate) record_canonical_trace: bool,
    pub(crate) record_public_trace: bool,

    #[serde(default)]
    pub(crate) derived_depth: bool,
}

impl PactTransaction {
    pub(crate) fn new(
        transaction_id: String,
        branch_id: String,
        boundary: BoundaryKind,
        trace: TransactionTrace,
        mut public_trace: PactTransactionTrace,
        new_content: Vec<ContentRecord>,
    ) -> Self {
        public_trace.replay_trace = None;
        Self {
            transaction_id,
            branch_id,
            boundary,
            trace,
            public_trace,
            new_content,
            head_checkpoint: None,
            ledger_delta: None,
            head_metadata: None,
        }
    }

    pub(crate) fn genesis(
        transaction_id: String,
        root_id: String,
        branch_id: String,
        revision: u64,
        ledger: serde_json::Value,
    ) -> Self {
        let head_metadata = super::index::HeadMetadata::from_snapshot(&ledger);
        Self::genesis_with_metadata(
            transaction_id,
            root_id,
            branch_id,
            revision,
            ledger,
            head_metadata,
        )
    }

    pub(crate) fn genesis_with_metadata(
        transaction_id: String,
        root_id: String,
        branch_id: String,
        revision: u64,
        ledger: serde_json::Value,
        head_metadata: Option<super::index::HeadMetadata>,
    ) -> Self {
        let pact_ref = PactRef {
            root_id: root_id.clone(),
            branch_id: branch_id.clone(),
            revision: Some(revision),
            transaction_id: Some(transaction_id.clone()),
            component_id: None,
            content_ref: None,
        };
        Self {
            transaction_id: transaction_id.clone(),
            branch_id: branch_id.clone(),
            boundary: BoundaryKind::Adhoc,
            trace: TransactionTrace {
                from_rev: revision,
                to_rev: revision,
                ops: Vec::new(),
            },
            public_trace: PactTransactionTrace {
                schema: TRANSACTION_TRACE_SCHEMA.to_string(),
                root_id,
                branch_id,
                transaction_id,
                authored_ref: pact_ref.clone(),
                pre_request_ref: pact_ref.clone(),
                post_request_alteration_ref: pact_ref,
                request_ref: None,
                response_ref: None,
                post_response_alteration_ref: None,
                post_response_ref: None,
                alteration_ids: Vec::new(),
                commit: None,
                replay_trace: None,
                aux: BTreeMap::new(),
            },
            new_content: Vec::new(),
            head_checkpoint: Some(ledger),
            ledger_delta: None,
            head_metadata,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn delta(
        transaction_id: String,
        branch_id: String,
        boundary: BoundaryKind,
        trace: TransactionTrace,
        public_trace: PactTransactionTrace,
        new_content: Vec<ContentRecord>,
        ledger_delta: LedgerDelta,
    ) -> Self {
        let mut transaction = Self::new(
            transaction_id,
            branch_id,
            boundary,
            trace,
            public_trace,
            new_content,
        );
        transaction.ledger_delta = Some(ledger_delta);
        transaction
    }

    pub(crate) fn is_genesis(&self) -> bool {
        self.head_checkpoint.is_some() && self.trace.from_rev == self.trace.to_rev
    }

    pub(crate) fn validate(&self) -> PactResult<()> {
        if self.transaction_id.is_empty() {
            return Err(PactError::invalid(
                "Pact transaction requires transaction_id",
            ));
        }
        if self.branch_id.is_empty() {
            return Err(PactError::invalid("Pact transaction requires branch_id"));
        }
        self.trace.validate()?;
        let valid_genesis = self.is_genesis() && self.trace.ops.is_empty();
        if !valid_genesis && self.trace.to_rev != self.trace.from_rev.saturating_add(1) {
            return Err(PactError::invalid(format!(
                "Pact transaction revisions must be contiguous: {} -> {}",
                self.trace.from_rev, self.trace.to_rev
            )));
        }
        self.public_trace.validate()?;
        if self.public_trace.transaction_id != self.transaction_id
            || self.public_trace.branch_id != self.branch_id
        {
            return Err(PactError::invalid(
                "Pact transaction public trace identity mismatch",
            ));
        }
        if self.public_trace.replay_trace.is_some() {
            return Err(PactError::invalid(
                "Pact transaction public trace duplicates canonical replay operations",
            ));
        }

        let mut content_hashes = BTreeMap::<&str, &str>::new();
        for content in &self.new_content {
            if content.id.is_empty() || content.hash.is_empty() {
                return Err(PactError::invalid(
                    "Pact transaction content requires id and hash",
                ));
            }
            if let Some(existing_hash) = content_hashes.insert(&content.id, &content.hash)
                && existing_hash != content.hash
            {
                return Err(PactError::invalid(format!(
                    "Pact transaction content id {} has conflicting hashes",
                    content.id
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn validate_against(
        &self,
        before: &crate::tree::ComponentTree,
        after: &crate::tree::ComponentTree,
    ) -> PactResult<()> {
        self.validate()?;
        let replayed = self.trace.replay_owned(before.project_value())?;
        let replayed = crate::tree::ComponentTree::from_value(&replayed)?;
        if replayed.project_value() != after.project_value() {
            return Err(PactError::invalid(
                "Pact transaction operations do not replay to the staged component tree",
            ));
        }
        Ok(())
    }
}
