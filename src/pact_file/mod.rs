pub(crate) mod allocator;
pub(crate) mod append_commit;
pub(crate) mod append_prepare;
pub(crate) mod append_state;
pub(crate) mod backend;
pub(crate) mod btree;
pub(crate) mod byte_lru;
pub(crate) mod change_feed;
pub(crate) mod checkpoint;
pub(crate) mod checkpoint_stream;
pub(crate) mod codec;
pub(crate) mod compact_codec;
pub(crate) mod compaction;
pub(crate) mod content;
pub(crate) mod envelope;
pub(crate) mod extension;
pub(crate) mod facet_projection;
pub(crate) mod head_checkpoint;
pub(crate) mod index;
pub(crate) mod layout;
pub(crate) mod layout_metrics;
pub(crate) mod ledger_cache;
pub(crate) mod log;
pub(crate) mod memory_accounting;
pub(crate) mod message_stream;
pub(crate) mod root_slot;
pub(crate) mod scratch;
pub(crate) mod search_change_feed;
pub(crate) mod search_fields;
pub(crate) mod search_selector;
pub(crate) mod search_source;
pub(crate) mod segment;
pub(crate) mod snapshot;
pub(crate) mod stream;
pub(crate) mod stream_decode;
pub(crate) mod stream_encode;
pub(crate) mod stream_maintenance;
pub(crate) mod transaction;
pub(crate) mod transaction_stream;
pub(crate) mod validate;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use crate::error::{PactError, PactResult};
use crate::store::{
    ContentRecord, ContextTreeLedger, PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA,
};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;

pub(crate) fn encode_file_baseline(ledger: &ContextTreeLedger) -> PactResult<Vec<u8>> {
    encode_cold_file_baseline(ledger)
}

pub(crate) fn encode_cold_file_baseline(ledger: &ContextTreeLedger) -> PactResult<Vec<u8>> {
    encode_cold_file_baseline_with_identity(ledger, envelope::new_file_identity()?)
}

pub(crate) fn encode_cold_file_baseline_with_identity(
    ledger: &ContextTreeLedger,
    file_identity: [u8; layout::FILE_IDENTITY_BYTES],
) -> PactResult<Vec<u8>> {
    let branch = ledger
        .branches
        .get(&ledger.current_branch_id)
        .ok_or_else(|| PactError::corrupt("cold Pact baseline current branch is missing"))?;
    let baseline_branches = index::baseline_component_ops_by_branch_from_ledger(ledger)?;
    let text_changes = change_feed::indexed_text_changes_from_ledger(ledger)?;
    let search_changes = search_change_feed::indexed_search_changes_from_ledger(ledger)?;
    let ledger_value = head_checkpoint::compact_ledger_value(ledger)?;
    let mut genesis = transaction::PactTransaction::genesis_with_metadata(
        format!("genesis_{}", ledger.root_id),
        ledger.root_id.clone(),
        ledger.current_branch_id.clone(),
        branch.head_revision,
        ledger_value,
        Some(index::HeadMetadata::from_ledger(ledger)),
    );
    genesis.new_content = ledger.content.records.values().cloned().collect();
    log::encode_cold_genesis_prepared_with_identity(
        genesis,
        &baseline_branches,
        &text_changes,
        &search_changes,
        file_identity,
    )
}

pub(crate) fn encode_canonical_file(ledger: &ContextTreeLedger) -> PactResult<Vec<u8>> {
    encode_cold_file_baseline(ledger)
}

pub(crate) fn canonicalize_component_depth(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            values.iter_mut().for_each(canonicalize_component_depth);
        }
        serde_json::Value::Object(object) => {
            for value in object.values_mut() {
                canonicalize_component_depth(value);
            }
            if !object.contains_key("componentType") {
                return;
            }
            let aux_depth = object
                .get("aux")
                .and_then(serde_json::Value::as_object)
                .and_then(|aux| aux.get("pact_depth").or_else(|| aux.get("depth")))
                .cloned();
            if !object.contains_key("pact_depth")
                && let Some(depth) = aux_depth
            {
                object.insert("pact_depth".to_string(), depth);
            }
            let pact_depth = object.get("pact_depth").cloned();
            if let Some(serde_json::Value::Object(aux)) = object.get_mut("aux") {
                for key in ["pact_depth", "depth"] {
                    if aux.get(key) == pact_depth.as_ref() {
                        aux.remove(key);
                    }
                }
                if aux.is_empty() {
                    object.remove("aux");
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn open_shared_ledger(path: &Path) -> PactResult<Arc<ContextTreeLedger>> {
    let canonical = path.canonicalize()?;
    let metadata = std::fs::metadata(&canonical)?;

    let snapshot =
        snapshot::Snapshot::open(&canonical, Arc::new(backend::IoCounterSet::default()))?;
    let key = ledger_cache::LedgerCacheKey::new(
        &canonical,
        &metadata,
        snapshot.source_file_identity(),
        snapshot.generation(),
        snapshot.committed_len(),
    );
    if let Some(ledger) = ledger_cache::get(&key) {
        return Ok(ledger);
    }

    #[cfg(test)]
    crate::tests::instrumentation::LEDGER_HYDRATION_COUNT.with(|count| count.set(count.get() + 1));
    backend::record_ledger_hydration();
    let ledger = Arc::new(snapshot.replay_ledger()?);
    ledger_cache::put(key, &ledger);
    Ok(ledger)
}

pub(crate) fn open_ledger(path: &Path) -> PactResult<ContextTreeLedger> {
    open_shared_ledger(path).map(|ledger| ledger.as_ref().clone())
}

pub(crate) fn open_head_ledger(path: &Path) -> PactResult<Arc<ContextTreeLedger>> {
    let canonical = path.canonicalize()?;
    let snapshot =
        snapshot::Snapshot::open(&canonical, Arc::new(backend::IoCounterSet::default()))?;
    if let Some(ledger) = facet_projection::head_ledger(&snapshot)? {
        return Ok(Arc::new(ledger));
    }

    open_shared_ledger(&canonical)
}

pub(crate) fn open_ledger_bytes(bytes: &[u8]) -> PactResult<ContextTreeLedger> {
    let selected = root_slot::select_root_slot(bytes)?;
    let addresses = index::generation_addresses(
        bytes,
        selected.slot.index_root_address,
        selected.slot.generation,
    )?;

    let decode = |address: u64| -> PactResult<transaction::PactTransaction> {
        let generation = codec::decode_record_generation_at(bytes, address)?;
        let decoded = codec::decode_transaction(bytes, address, generation)?;
        let mut transaction = decoded.transaction;
        if transaction
            .head_checkpoint
            .as_ref()
            .is_some_and(head_checkpoint::is_compact_ledger)
        {
            transaction.head_checkpoint = Some(head_checkpoint::hydrate_ledger(
                bytes,
                decoded.marker.index_root_address,
                transaction
                    .head_checkpoint
                    .as_ref()
                    .ok_or_else(|| PactError::internal("compact Pact ledger disappeared"))?,
            )?);
        }
        Ok(transaction)
    };
    let mut addresses = addresses.into_iter();
    let baseline = decode(
        addresses
            .next()
            .ok_or_else(|| PactError::corrupt("Pact file has no baseline transaction"))?,
    )?;
    let value = baseline.head_checkpoint.ok_or_else(|| {
        PactError::corrupt("latest Pact transaction is missing its ledger checkpoint")
    })?;
    let mut ledger: ContextTreeLedger = serde_json::from_value(value)
        .map_err(|error| PactError::corrupt(format!("invalid Pact ledger checkpoint: {error}")))?;
    ledger.content.rebuild_hash_index();
    replay_transactions_from_baseline(ledger, baseline.new_content, addresses.map(decode))
}

pub(crate) fn replay_transactions(
    transactions: Vec<transaction::PactTransaction>,
) -> PactResult<ContextTreeLedger> {
    replay_transactions_with_metrics(transactions).map(|(ledger, _)| ledger)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReplayMetrics {
    pub(crate) projections_built: usize,
}

struct ReplayBranchState {
    root: serde_json::Value,
    first_seen: BTreeMap<String, u64>,
    lifecycle_seen: BTreeMap<String, u64>,
    lifecycle_birth_tick: u64,
}

pub(crate) fn replay_transactions_with_metrics(
    transactions: Vec<transaction::PactTransaction>,
) -> PactResult<(ContextTreeLedger, ReplayMetrics)> {
    let mut transactions = transactions.into_iter();
    let baseline = transactions
        .next()
        .ok_or_else(|| PactError::corrupt("Pact file has no baseline transaction"))?;
    let baseline_content = baseline.new_content;
    let value = baseline.head_checkpoint.ok_or_else(|| {
        PactError::corrupt("latest Pact transaction is missing its ledger checkpoint")
    })?;
    let mut ledger: ContextTreeLedger = serde_json::from_value(value)
        .map_err(|error| PactError::corrupt(format!("invalid Pact ledger checkpoint: {error}")))?;
    for content in baseline_content {
        ledger.content.records.insert(content.id.clone(), content);
    }
    ledger.content.rebuild_hash_index();
    let mut branches = BTreeMap::<String, ReplayBranchState>::new();
    let mut operations_replayed = 0_u64;
    for transaction in transactions {
        operations_replayed =
            operations_replayed.saturating_add(transaction.trace.ops.len() as u64);
        replay_ledger_delta_deferred(&mut ledger, &mut branches, transaction)?;
    }
    backend::record_global(&backend::IoCounters {
        operations_replayed,
        ..backend::IoCounters::default()
    });
    finish_replay(ledger, branches)
}

pub(crate) fn replay_transactions_from_baseline(
    ledger: ContextTreeLedger,
    baseline_content: Vec<ContentRecord>,
    transactions: impl IntoIterator<Item = PactResult<transaction::PactTransaction>>,
) -> PactResult<ContextTreeLedger> {
    replay_transactions_from_baseline_with_metrics(ledger, baseline_content, transactions)
        .map(|(ledger, _)| ledger)
}

pub(crate) fn strip_derived_depth_ops(
    ledger: &mut ContextTreeLedger,
    derived_branches: &BTreeSet<String>,
) {
    let is_depth_op = |op: &crate::transaction_trace::TraceOp| matches!(&op.kind, crate::transaction_trace::TraceOpKind::SetField { key, .. } if key == "pact_depth");
    for (branch_id, branch) in &mut ledger.branches {
        if !(derived_branches.contains(branch_id) || branch.derived_depth()) {
            continue;
        }
        for trace in &mut branch.transaction_traces {
            trace.ops.retain(|op| !is_depth_op(op));
        }
        for public_trace in ledger.event_index.transaction_traces.values_mut() {
            if &public_trace.branch_id == branch_id
                && let Some(replay) = public_trace.replay_trace.as_mut()
            {
                replay.ops.retain(|op| !is_depth_op(op));
            }
        }
    }
}

fn replay_transactions_from_baseline_with_metrics(
    mut ledger: ContextTreeLedger,
    baseline_content: Vec<ContentRecord>,
    transactions: impl IntoIterator<Item = PactResult<transaction::PactTransaction>>,
) -> PactResult<(ContextTreeLedger, ReplayMetrics)> {
    strip_derived_depth_ops(&mut ledger, &BTreeSet::new());
    if !baseline_content.is_empty() {
        for content in baseline_content {
            ledger.content.records.insert(content.id.clone(), content);
        }
        ledger.content.rebuild_hash_index();
    }
    let mut branches = BTreeMap::<String, ReplayBranchState>::new();
    let mut operations_replayed = 0_u64;
    for transaction in transactions {
        let transaction = transaction?;
        operations_replayed =
            operations_replayed.saturating_add(transaction.trace.ops.len() as u64);
        replay_ledger_delta_deferred(&mut ledger, &mut branches, transaction)?;
    }
    backend::record_global(&backend::IoCounters {
        operations_replayed,
        ..backend::IoCounters::default()
    });
    finish_replay(ledger, branches)
}

fn finish_replay(
    mut ledger: ContextTreeLedger,
    branches: BTreeMap<String, ReplayBranchState>,
) -> PactResult<(ContextTreeLedger, ReplayMetrics)> {
    let mut metrics = ReplayMetrics::default();
    for (branch_id, mut state) in branches {
        let branch = ledger
            .branches
            .get_mut(&branch_id)
            .ok_or_else(|| PactError::corrupt("Pact replay branch disappeared"))?;
        if branch.derived_depth() {
            crate::mutation::derive_turn_depths_value(&mut state.root);
        }
        let projection = crate::store::build_projection(&state.root, &state.first_seen, 1024)?;
        let mut view = crate::store::PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_id,
            branch.head_revision,
            state.lifecycle_seen,
            state.lifecycle_birth_tick,
        );
        view.tree.first_seen = state.first_seen;
        branch.current = view;
        metrics.projections_built += 1;
    }
    ledger.content.rebuild_hash_index();
    Ok((ledger, metrics))
}

pub(crate) fn is_v2_file(bytes: &[u8]) -> bool {
    bytes.starts_with(layout::FILE_MAGIC)
}

pub(crate) fn committed_prefix(bytes: &[u8]) -> PactResult<&[u8]> {
    let selected = root_slot::select_root_slot(bytes)?;
    let committed_len = usize::try_from(selected.slot.file_length_watermark)
        .map_err(|_| PactError::corrupt("Pact committed length exceeds platform limits"))?;
    bytes
        .get(..committed_len)
        .ok_or_else(|| PactError::corrupt("Pact root slot exceeds the available file length"))
}

pub(crate) fn validate(path: &Path) -> PactResult<backend::ValidationReport> {
    validate::fast_validate(path)
}

pub(crate) struct LedgerCommit<'a> {
    pub(crate) ledger: &'a ContextTreeLedger,
    pub(crate) branch_id: &'a str,
    pub(crate) boundary: BoundaryKind,
    pub(crate) trace: TransactionTrace,
    pub(crate) public_trace: Option<PactTransactionTrace>,
    pub(crate) delta: transaction::LedgerDelta,
    pub(crate) new_content: Vec<ContentRecord>,

    pub(crate) maintain: bool,
}

pub(crate) fn commit_ledger(path: &Path, request: LedgerCommit<'_>) -> PactResult<()> {
    let ledger = request.ledger;
    let transaction_id = request
        .public_trace
        .as_ref()
        .map(|trace| trace.transaction_id.clone())
        .unwrap_or_else(|| format!("tx_storage_{}_{}", request.branch_id, request.trace.to_rev));
    let public_trace = request.public_trace.unwrap_or_else(|| {
        synthetic_public_trace(
            &request.ledger.root_id,
            request.branch_id,
            &transaction_id,
            request.trace.from_rev,
            request.trace.to_rev,
        )
    });
    let mut transaction = transaction::PactTransaction::delta(
        transaction_id,
        request.branch_id.to_string(),
        request.boundary,
        request.trace,
        public_trace,
        request.new_content,
        request.delta,
    );
    transaction.head_metadata = Some(index::HeadMetadata::from_ledger(request.ledger));
    log::commit_with_fault(path, &transaction, &backend::IoCounterSet::default(), None)?;
    if request.maintain {
        let _ = compaction::maintain_after_append(path, ledger);
    }
    Ok(())
}

pub(crate) fn append_extension_records(
    path: &Path,
    branch_id: &str,
    records: Vec<crate::PactExtensionRecord>,
) -> PactResult<crate::PactExtensionCommit> {
    crate::extension::validate_extension_batch(&records)?;
    let snapshot = snapshot::Snapshot::open(path, Arc::new(backend::IoCounterSet::default()))?;
    let mut metadata = snapshot.metadata()?.clone();
    let from_revision = snapshot
        .branch_head(branch_id)?
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    for record in &records {
        match extension::record(&snapshot, branch_id, &record.record_id) {
            Ok(_) => {
                return Err(PactError::invalid(format!(
                    "duplicate Pact extension record id: {}",
                    record.record_id
                )));
            }
            Err(PactError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }

    let revision = from_revision.saturating_add(1);
    let transaction_id = format!("tx_ext_{branch_id}_{revision}");
    let mut public_trace = synthetic_public_trace(
        &metadata.root_id,
        branch_id,
        &transaction_id,
        from_revision,
        revision,
    );
    public_trace.aux.insert(
        crate::PACT_EXTENSIONS_AUX_KEY.to_string(),
        serde_json::to_value(records)
            .map_err(|error| PactError::codec(format!("encode Pact extensions: {error}")))?,
    );
    public_trace
        .aux
        .insert("boundary".to_string(), serde_json::json!("adhoc"));
    public_trace.aux.insert(
        "applied".to_string(),
        serde_json::json!(["extension_append"]),
    );
    public_trace
        .aux
        .insert("events".to_string(), serde_json::json!([]));

    metadata
        .head_revisions
        .insert(branch_id.to_string(), revision);
    metadata
        .head_transaction_ids
        .insert(branch_id.to_string(), transaction_id.clone());
    metadata.trace_count = metadata.trace_count.saturating_add(1);
    let mut transaction = transaction::PactTransaction::new(
        transaction_id.clone(),
        branch_id.to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: from_revision,
            to_rev: revision,
            ops: Vec::new(),
        },
        public_trace,
        Vec::new(),
    );
    transaction.head_metadata = Some(metadata);
    log::commit_with_fault(path, &transaction, &backend::IoCounterSet::default(), None)?;
    maintain_after_extension_append(path)?;
    Ok(crate::PactExtensionCommit {
        branch_id: branch_id.to_string(),
        revision,
        transaction_id,
    })
}

fn maintain_after_extension_append(path: &Path) -> PactResult<()> {
    let snapshot = snapshot::Snapshot::open(path, Arc::new(backend::IoCounterSet::default()))?;
    if snapshot.generation() <= 16 && std::fs::metadata(path)?.len() <= 1_000_000 {
        return Ok(());
    }
    let ledger = open_ledger(path)?;
    compaction::maintain_after_append(path, &ledger)
}

fn replay_ledger_delta_deferred(
    ledger: &mut ContextTreeLedger,
    replay_branches: &mut BTreeMap<String, ReplayBranchState>,
    transaction: transaction::PactTransaction,
) -> PactResult<()> {
    let transaction::PactTransaction {
        branch_id,
        trace,
        mut public_trace,
        new_content,
        ledger_delta,
        ..
    } = transaction;
    let Some(delta) = ledger_delta else {
        let records = crate::extension::records_from_trace(&public_trace)?;
        if records.is_empty() || !trace.ops.is_empty() {
            return Err(PactError::corrupt(
                "Pact transaction is missing its ledger delta",
            ));
        }
        let branch = ledger.branches.get_mut(&branch_id).ok_or_else(|| {
            PactError::corrupt("Pact extension transaction references a missing branch")
        })?;
        if branch.head_revision != trace.from_rev {
            return Err(PactError::corrupt(format!(
                "Pact extension revision {} does not follow branch head {}",
                trace.from_rev, branch.head_revision
            )));
        }
        branch.head_revision = trace.to_rev;
        branch.current.head_revision = trace.to_rev;
        public_trace.replay_trace = Some(trace);
        ledger
            .event_index
            .by_transaction
            .entry(public_trace.transaction_id.clone())
            .or_default()
            .push(branch_id);
        ledger
            .event_index
            .transaction_traces
            .insert(public_trace.transaction_id.clone(), public_trace);
        return Ok(());
    };
    let branch = ledger
        .branches
        .get(&branch_id)
        .ok_or_else(|| PactError::corrupt("Pact ledger delta references a missing branch"))?;
    if branch.head_revision != trace.from_rev {
        return Err(PactError::corrupt(format!(
            "Pact ledger delta revision {} does not follow branch head {}",
            trace.from_rev, branch.head_revision
        )));
    }
    let state = if let Some(state) = replay_branches.get_mut(&branch_id) {
        state
    } else {
        let root = branch.current.tree.root.project_component_value();
        replay_branches
            .entry(branch_id.clone())
            .or_insert_with(|| ReplayBranchState {
                root,
                first_seen: branch.current.tree.first_seen.clone(),
                lifecycle_seen: branch.current.lifecycle_seen.clone(),
                lifecycle_birth_tick: branch.lifecycle_tick.saturating_add(1),
            })
    };
    state.root = trace.replay_owned(std::mem::take(&mut state.root))?;
    state.first_seen.extend(delta.first_seen_additions);
    state.lifecycle_seen.extend(delta.lifecycle_seen_additions);
    for component_id in delta.lifecycle_seen_removals {
        state.lifecycle_seen.remove(&component_id);
    }
    state.lifecycle_birth_tick = delta.lifecycle_birth_tick;
    let branch = ledger
        .branches
        .get_mut(&branch_id)
        .ok_or_else(|| PactError::corrupt("Pact replay branch disappeared"))?;
    branch.head_revision = trace.to_rev;
    branch.lifecycle_tick = delta.lifecycle_tick;
    branch.base_ref = delta.base_ref;
    if delta.derived_depth {
        branch.mark_derived_depth();
    }
    if delta.record_canonical_trace {
        branch.transaction_traces.push(trace.clone());
    }
    for content in new_content {
        ledger
            .content
            .records_by_hash
            .insert(content.hash.clone(), content.id.clone());
        ledger.content.records.insert(content.id.clone(), content);
    }
    if delta.record_public_trace {
        public_trace.replay_trace = Some(trace);
        ledger
            .event_index
            .by_transaction
            .entry(public_trace.transaction_id.clone())
            .or_default()
            .push(branch_id);
        ledger
            .event_index
            .transaction_traces
            .insert(public_trace.transaction_id.clone(), public_trace);
    }
    Ok(())
}

fn synthetic_public_trace(
    root_id: &str,
    branch_id: &str,
    transaction_id: &str,
    from_rev: u64,
    to_rev: u64,
) -> PactTransactionTrace {
    let pact_ref = PactRef {
        root_id: root_id.to_string(),
        branch_id: branch_id.to_string(),
        revision: Some(to_rev),
        transaction_id: Some(transaction_id.to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: root_id.to_string(),
        branch_id: branch_id.to_string(),
        transaction_id: transaction_id.to_string(),
        authored_ref: pact_ref.clone(),
        pre_request_ref: PactRef {
            revision: Some(from_rev),
            ..pact_ref.clone()
        },
        post_request_alteration_ref: pact_ref,
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: None,
        alteration_ids: Vec::new(),
        commit: None,
        replay_trace: None,
        aux: std::collections::BTreeMap::new(),
    }
}
