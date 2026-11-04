use crate::boundary::PendingExternalMutations;
use crate::components::{AuthoringComponentOptions, mint_component_local_id};
use crate::context_tree::DeterministicAuthoringOptions;
use crate::error::{PactError, PactResult};
use crate::file_format::PactFileManifest;
use crate::mutation::MutationRequest;
use crate::ops::PactOperation;
use crate::store::{ContextTreeLedger, bootstrap_ledger, new_ledger_json};
use crate::trace::ResolvedOperationTrace;
use crate::tree::ComponentTree;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

mod api;
mod commit;
mod lifecycle;
pub(crate) mod message_authoring;
mod mutations;
mod payloads;
mod tools;

pub(crate) use payloads::externalize_node_payloads;
pub(crate) use tools::is_hollow_context_container;

#[derive(Clone, Debug)]
pub(super) struct StagedTraceRecord {
    pub trace: ResolvedOperationTrace,
    pub boundary: crate::trace::BoundaryKind,
    pub op: PactOperation,
}

#[derive(Clone, Debug)]
pub(super) struct DeterministicAuthoringState {
    local_id_prefix: String,
    next_local_id: u64,
    start_timestamp_ns: u64,
    next_timestamp_ns: u64,
    timestamp_step_ns: u64,
}

impl DeterministicAuthoringState {
    fn from_options(options: DeterministicAuthoringOptions) -> PactResult<Self> {
        options.validate()?;
        Ok(Self {
            local_id_prefix: options.local_id_prefix().to_string(),
            next_local_id: 1,
            start_timestamp_ns: options.start_timestamp_ns(),
            next_timestamp_ns: options.start_timestamp_ns(),
            timestamp_step_ns: options.timestamp_step_ns(),
        })
    }

    fn file_identity(&self, root_id: &str) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pactree.deterministic.file_identity.v1");
        hasher.update(&(self.local_id_prefix.len() as u64).to_le_bytes());
        hasher.update(self.local_id_prefix.as_bytes());
        hasher.update(&self.start_timestamp_ns.to_le_bytes());
        hasher.update(&self.timestamp_step_ns.to_le_bytes());
        hasher.update(root_id.as_bytes());
        *hasher.finalize().as_bytes()
    }

    fn next_local_id(&mut self) -> String {
        let local_id = format!("{}{:08x}", self.local_id_prefix, self.next_local_id);
        self.next_local_id = self.next_local_id.saturating_add(1);
        local_id
    }

    fn next_timestamp_ns(&mut self) -> u64 {
        let timestamp = self.next_timestamp_ns;
        self.next_timestamp_ns = self
            .next_timestamp_ns
            .saturating_add(self.timestamp_step_ns);
        timestamp
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RecordedFileCommit {
    pub(crate) trace: crate::transaction_trace::TransactionTrace,
    pub(crate) public_trace: Option<crate::store::PactTransactionTrace>,
    pub(crate) delta: crate::pact_file::transaction::LedgerDelta,
    pub(crate) new_content: Vec<crate::store::ContentRecord>,
    pub(crate) head_metadata: crate::pact_file::index::HeadMetadata,
}

pub struct PactResolver {
    pub(super) ledger: Arc<ContextTreeLedger>,

    pub(super) pending: BTreeMap<String, Vec<PactOperation>>,

    pub(super) seen_keys: BTreeMap<String, BTreeSet<String>>,

    pub(super) staged_state: BTreeMap<String, (ComponentTree, Vec<MutationRequest>)>,

    pub(super) staged_traces: BTreeMap<String, Vec<ResolvedOperationTrace>>,

    pub(super) staged_trace_records: BTreeMap<String, Vec<StagedTraceRecord>>,

    pub(super) staged_extensions: BTreeMap<String, Vec<crate::PactExtensionRecord>>,

    pub(super) pending_external: BTreeMap<String, PendingExternalMutations>,

    pub(super) requirement_counter: u64,

    pub(super) node_counter: u64,

    pub(super) trace_counter: u64,

    pub(super) replay_trace_ids: VecDeque<(String, u64)>,

    pub(super) file_commit_log: Option<Vec<RecordedFileCommit>>,

    pub(super) automatic_compaction: bool,

    pub(super) deterministic_authoring: Option<DeterministicAuthoringState>,

    pub(super) file_path: Option<PathBuf>,

    pub(super) file_manifest: Option<PactFileManifest>,
    pub(super) persisted_content_ids: BTreeSet<String>,
    pub(super) blob_inline_limit: usize,
}

impl PactResolver {
    pub fn new(context_tree_id: &str, branch_id: Option<&str>) -> PactResult<Self> {
        Ok(Self::from_ledger(bootstrap_ledger(
            context_tree_id,
            branch_id,
            1024,
        )?))
    }

    pub fn from_initial_tree(initial_tree_json: &str, branch_id: Option<&str>) -> PactResult<Self> {
        let root_store_json = new_ledger_json(initial_tree_json, branch_id, 1024)?;
        let ledger: ContextTreeLedger = serde_json::from_str(&root_store_json)
            .map_err(|err| PactError::codec(format!("failed to decode root store: {err}")))?;
        Ok(Self {
            ledger: Arc::new(ledger),
            pending: BTreeMap::new(),
            seen_keys: BTreeMap::new(),
            staged_state: BTreeMap::new(),
            staged_traces: BTreeMap::new(),
            staged_trace_records: BTreeMap::new(),
            staged_extensions: BTreeMap::new(),
            pending_external: BTreeMap::new(),
            requirement_counter: 0,
            node_counter: 0,
            trace_counter: 0,
            replay_trace_ids: VecDeque::new(),
            file_commit_log: None,
            automatic_compaction: true,
            deterministic_authoring: None,
            file_path: None,
            file_manifest: None,
            persisted_content_ids: BTreeSet::new(),
            blob_inline_limit: 1024,
        })
    }

    pub fn open_file(path: PathBuf) -> PactResult<Self> {
        let ledger = crate::pact_file::open_head_ledger(&path).map_err(|error| {
            PactError::storage(format!("failed to open .pact file {path:?}: {error}"))
        })?;
        Ok(Self::from_shared_file_ledger(ledger, path))
    }

    pub(crate) fn shared_ledger(&self) -> Arc<ContextTreeLedger> {
        Arc::clone(&self.ledger)
    }

    pub(crate) fn file_path(&self) -> Option<&std::path::Path> {
        self.file_path.as_deref()
    }

    pub(crate) fn from_shared_file_ledger(ledger: Arc<ContextTreeLedger>, path: PathBuf) -> Self {
        let manifest = candidate_manifest(&path, &ledger);
        let persisted_content_ids = ledger.content.records.keys().cloned().collect();
        Self {
            ledger,
            pending: BTreeMap::new(),
            seen_keys: BTreeMap::new(),
            staged_state: BTreeMap::new(),
            staged_traces: BTreeMap::new(),
            staged_trace_records: BTreeMap::new(),
            staged_extensions: BTreeMap::new(),
            pending_external: BTreeMap::new(),
            requirement_counter: 0,
            node_counter: 0,
            trace_counter: 0,
            replay_trace_ids: VecDeque::new(),
            file_commit_log: None,
            automatic_compaction: true,
            deterministic_authoring: None,
            file_path: Some(path),
            file_manifest: Some(manifest),
            persisted_content_ids,
            blob_inline_limit: 1024,
        }
    }

    pub fn from_ledger(ledger: ContextTreeLedger) -> Self {
        Self {
            ledger: Arc::new(ledger),
            pending: BTreeMap::new(),
            seen_keys: BTreeMap::new(),
            staged_state: BTreeMap::new(),
            staged_traces: BTreeMap::new(),
            staged_trace_records: BTreeMap::new(),
            staged_extensions: BTreeMap::new(),
            pending_external: BTreeMap::new(),
            requirement_counter: 0,
            node_counter: 0,
            trace_counter: 0,
            replay_trace_ids: VecDeque::new(),
            file_commit_log: None,
            automatic_compaction: true,
            deterministic_authoring: None,
            file_path: None,
            file_manifest: None,
            persisted_content_ids: BTreeSet::new(),
            blob_inline_limit: 1024,
        }
    }

    pub(crate) fn from_shared_ledger(ledger: Arc<ContextTreeLedger>) -> Self {
        Self {
            ledger,
            pending: BTreeMap::new(),
            seen_keys: BTreeMap::new(),
            staged_state: BTreeMap::new(),
            staged_traces: BTreeMap::new(),
            staged_trace_records: BTreeMap::new(),
            staged_extensions: BTreeMap::new(),
            pending_external: BTreeMap::new(),
            requirement_counter: 0,
            node_counter: 0,
            trace_counter: 0,
            replay_trace_ids: VecDeque::new(),
            file_commit_log: None,
            automatic_compaction: true,
            deterministic_authoring: None,
            file_path: None,
            file_manifest: None,
            persisted_content_ids: BTreeSet::new(),
            blob_inline_limit: 1024,
        }
    }

    pub(crate) fn replace_ledger(&mut self, ledger: ContextTreeLedger) {
        self.ledger = Arc::new(ledger);
    }

    pub(crate) fn set_automatic_compaction(&mut self, enabled: bool) {
        self.automatic_compaction = enabled;
    }

    pub(crate) fn mark_derived_depth(&mut self, branch_id: &str) {
        if let Some(branch) = Arc::make_mut(&mut self.ledger).branches.get_mut(branch_id) {
            branch.mark_derived_depth();
        }
    }

    pub(crate) fn set_deterministic_authoring(
        &mut self,
        options: Option<DeterministicAuthoringOptions>,
    ) -> PactResult<()> {
        self.deterministic_authoring = options
            .map(DeterministicAuthoringState::from_options)
            .transpose()?;
        Ok(())
    }

    pub(crate) fn copy_authoring_source_from(&mut self, other: &Self) {
        self.deterministic_authoring = other.deterministic_authoring.clone();
    }

    pub(crate) fn next_authoring_component_options(&mut self) -> AuthoringComponentOptions {
        let Some(state) = self.deterministic_authoring.as_mut() else {
            return AuthoringComponentOptions::default();
        };
        AuthoringComponentOptions {
            local_id: Some(state.next_local_id()),
            created_at_ns: Some(state.next_timestamp_ns()),
            ..AuthoringComponentOptions::default()
        }
    }

    pub(crate) fn record_file_commits(&mut self) {
        self.file_commit_log.get_or_insert_with(Vec::new);
    }

    pub(crate) fn take_file_commits(&mut self) -> Vec<RecordedFileCommit> {
        self.file_commit_log
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    pub(crate) fn next_authoring_component_local_id(&mut self) -> String {
        self.deterministic_authoring
            .as_mut()
            .map(DeterministicAuthoringState::next_local_id)
            .unwrap_or_else(mint_component_local_id)
    }

    pub(crate) fn next_authoring_timestamp_ns(&mut self) -> u64 {
        self.deterministic_authoring
            .as_mut()
            .map(DeterministicAuthoringState::next_timestamp_ns)
            .unwrap_or_else(commit::monotonic_ns)
    }

    pub(crate) fn deterministic_file_identity(&self) -> Option<[u8; 32]> {
        self.deterministic_authoring
            .as_ref()
            .map(|state| state.file_identity(&self.ledger.root_id))
    }

    pub(crate) fn encode_new_file_baseline(&self) -> PactResult<Vec<u8>> {
        match self.deterministic_file_identity() {
            Some(identity) => {
                crate::pact_file::encode_cold_file_baseline_with_identity(&self.ledger, identity)
            }
            None => crate::pact_file::encode_file_baseline(&self.ledger),
        }
    }

    pub fn into_ledger(self) -> ContextTreeLedger {
        match Arc::try_unwrap(self.ledger) {
            Ok(ledger) => ledger,
            Err(shared) => shared.as_ref().clone(),
        }
    }

    pub(crate) fn merge_content_store(&mut self, content_store: &crate::store::ContentStore) {
        let ledger = Arc::make_mut(&mut self.ledger);
        for (content_id, record) in &content_store.records {
            ledger
                .content
                .records
                .entry(content_id.clone())
                .or_insert_with(|| record.clone());
        }
        ledger.content.rebuild_hash_index();
    }
}

fn candidate_manifest(path: &std::path::Path, ledger: &ContextTreeLedger) -> PactFileManifest {
    let node_count = ledger
        .branches
        .get(&ledger.current_branch_id)
        .map(|branch| branch.current.tree.index.by_id.len())
        .unwrap_or_default();
    let trace_count = ledger.trace_count();
    PactFileManifest {
        schema: "pact.file.v2".to_string(),
        version: 2,
        file_kind: "branch".to_string(),
        file_bytes: std::fs::metadata(path)
            .map(|metadata| metadata.len() as usize)
            .unwrap_or_default(),
        branch_count: ledger.branches.len(),
        trace_count,
        record_count: node_count + trace_count + 3,
        node_count,
        ..PactFileManifest::default()
    }
}

#[cfg(test)]
mod tests;
