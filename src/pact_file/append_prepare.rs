use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::append_state::{Admission, AppendState, classify_incoming, is_shiftable};
use super::change_feed::{IndexedTextChange, content_record_bytes, text_changes_from_op};
use super::index::{HeadMetadata, IndexKey, MESSAGE_ORDER_RANK_BITS, MessageOrderRepairPlan};
use super::search_change_feed::{IndexedSearchChange, search_change_from_op};
use super::snapshot::Snapshot;
use super::transaction::LedgerDelta;
use crate::components::{Component, DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES};
use crate::error::{PactError, PactResult};
use crate::memory::PactMemoryOptions;
use crate::ops::PactOperation;
use crate::resolver::externalize_node_payloads;
use crate::resolver::message_authoring::{
    AuthoringIdentity, SealedTraceRequest, author_components, build_sealed_public_trace,
    depth_trace_op, insert_subtree_trace_ops, lower_message, produced_refs_for_insert,
    stamp_insert_identity, trace_identity,
};
use crate::store::{
    ContentRecord, ContentStore, PactRef, PactStoreStats, PactTransactionTrace,
    externalize_large_content_in_value, project_branch_component_tree,
};
use crate::trace::{BoundaryKind, ResolvedOperationTrace};
use crate::transaction_trace::TraceOp;
use crate::transaction_trace::stream_codec::TraceSpool;

#[cfg(test)]
mod tests;

const SHIFT_PAGE_ENTRIES: usize = 256;

pub(crate) struct AppendMessage {
    pub(crate) role: String,
    pub(crate) parts: Vec<Component>,
    pub(crate) aux: BTreeMap<String, Value>,
}

pub(crate) struct PreparedInsert {
    pub(crate) parent_id: String,
    pub(crate) index: usize,

    pub(crate) component: Component,

    pub(crate) projected: Value,

    pub(crate) produced_ids: Vec<String>,
}

pub(crate) struct PreparedTransaction {
    pub(crate) transaction_id: String,
    pub(crate) from_revision: u64,
    pub(crate) to_revision: u64,
    pub(crate) trace: TraceSpool,
    pub(crate) public_trace: PactTransactionTrace,
    pub(crate) delta: LedgerDelta,
    pub(crate) new_content: Vec<ContentRecord>,
    pub(crate) inserts: Vec<PreparedInsert>,

    pub(crate) shift_ops: u64,

    pub(crate) index_ops: Vec<(u64, TraceOp)>,
    pub(crate) text_changes: Vec<IndexedTextChange>,
    pub(crate) search_changes: Vec<IndexedSearchChange>,

    pub(crate) metadata: HeadMetadata,
    pub(crate) inserted_nodes: u64,
    pub(crate) inserted_messages: u64,

    pub(crate) trace_symbols: u64,

    pub(crate) trace_scratch_bytes: u64,

    pub(crate) order_repair: Option<MessageOrderRepairPlan>,
}

fn count_message_turns(component: &Component) -> u64 {
    let own = u64::from(component.component_type() == "message_turn");
    own + component
        .children
        .iter()
        .map(count_message_turns)
        .sum::<u64>()
}

fn inline_content_chars(component: &Component) -> usize {
    component
        .content()
        .map(|text| text.chars().count())
        .unwrap_or_default()
        + component
            .children
            .iter()
            .map(inline_content_chars)
            .sum::<usize>()
}

fn subtree_ids(component: &Component, out: &mut Vec<String>) {
    out.push(component.id.clone());
    for child in &component.children {
        subtree_ids(child, out);
    }
}

fn offset_i32(offset: i64) -> PactResult<i32> {
    i32::try_from(offset).map_err(|_| {
        PactError::unsupported(format!(
            "component offset {offset} exceeds the format limit"
        ))
    })
}

impl AppendState {
    pub(crate) fn prepare(
        &mut self,
        snapshot: &Snapshot,
        messages: Vec<AppendMessage>,
        extensions: Vec<crate::PactExtensionRecord>,
        identity: &mut impl AuthoringIdentity,
        options: &PactMemoryOptions,
    ) -> PactResult<PreparedTransaction> {
        options.validate()?;
        if messages.is_empty() {
            return Err(PactError::invalid("append batch has no messages"));
        }

        let message_turns = messages
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| 1 + message.parts.iter().map(count_message_turns).sum::<u64>())
            .sum::<u64>();
        if message_turns > 1_u64 << MESSAGE_ORDER_RANK_BITS {
            return Err(PactError::unsupported(format!(
                "Pact message order supports at most {} message turns per transaction; the batch carries {message_turns}",
                1_u64 << MESSAGE_ORDER_RANK_BITS
            )));
        }
        let root_id = self.root_id.clone();
        let branch_id = self.branch_id.clone();
        let from_revision = self.head_revision;
        let to_revision = from_revision
            .checked_add(1)
            .ok_or_else(|| PactError::unsupported("branch revision exceeds the format limit"))?;
        let mut trace = TraceSpool::new(
            from_revision,
            to_revision,
            options.io_chunk_bytes,
            options.scratch_cache_bytes,
        )?;
        let mut content = ContentStore::default();
        let mut events = Vec::with_capacity(messages.len());
        let mut applied = Vec::with_capacity(messages.len());
        let mut inserts = Vec::with_capacity(messages.len());
        let mut first_seen_additions = BTreeMap::new();
        let mut lifecycle_seen_additions = BTreeMap::new();
        let birth_tick = self.lifecycle_tick + 1;
        let mut shift_ops = 0_u64;
        let mut counter = 0_u64;
        let mut index_ops: Vec<(u64, TraceOp)> = Vec::new();
        let mut op_ordinal = 0_u64;
        let mut inserted_nodes = 0_u64;
        let mut inserted_messages = 0_u64;
        let mut inserted_chars = 0_usize;

        let note_admission = |admission: Admission,
                              first_seen: &mut BTreeMap<String, u64>,
                              lifecycle: &mut BTreeMap<String, u64>| {
            for id in admission.first_seen_added {
                first_seen.insert(id, to_revision);
            }
            for id in admission.lifecycle_added {
                lifecycle.insert(id, birth_tick);
            }
        };

        for message in messages {
            let mut authored = BTreeSet::new();
            let parts = author_components(
                &root_id,
                message.parts,
                identity,
                &mut |id| self.is_current_id(id),
                &mut authored,
            )?;
            let Some(mut op) = lower_message(
                &root_id,
                &branch_id,
                &message.role,
                parts,
                message.aux,
                identity,
            )?
            else {
                return Err(PactError::invalid(
                    "system message has no text; an append batch cannot carry a no-op message",
                ));
            };
            stamp_insert_identity(&mut op, identity);
            op.validate()?;
            let PactOperation::Insert {
                envelope,
                parent_id,
                component,
                ..
            } = &op
            else {
                return Err(PactError::resolver(
                    "message lowering produced a non-insert",
                ));
            };
            let created_at_ns = envelope
                .host_timestamp_ns
                .ok_or_else(|| PactError::resolver("lowered message has no timestamp"))?;
            let parent = parent_id
                .clone()
                .ok_or_else(|| PactError::resolver("lowered message has no parent"))?;

            let mut node = component.clone();
            externalize_node_payloads(&mut content, &mut node)?;
            classify_incoming(&node)?;

            let (index, admission) = if parent == root_id {
                crate::mutation::set_subtree_depth_component(&mut node, 0);
                node.parent_id = Some(root_id.clone());
                node.offset = offset_i32(self.next_offset)?;
                let index = usize::try_from(self.root_child_count).map_err(|_| {
                    PactError::unsupported("root child count exceeds platform limits")
                })?;

                insert_subtree_trace_ops(&node, &root_id, index, None, &mut |op| {
                    trace.push_op(&op)?;
                    index_ops.push((op_ordinal, op));
                    op_ordinal += 1;
                    Ok(())
                })?;
                let shifted = if self.derived_depth {
                    0
                } else {
                    self.emit_depth_shifts(&mut trace)?
                };
                shift_ops += shifted;
                op_ordinal += shifted;
                let admission = self.admit_root_turn(&node, to_revision)?;
                (index, admission)
            } else {
                let parent_row = self.node(&parent)?.ok_or_else(|| {
                    PactError::invalid(format!("insert parent `{parent}` not found"))
                })?;
                let depth = node
                    .pact_depth
                    .or(parent_row.depth.and_then(|depth| i32::try_from(depth).ok()));
                if let Some(depth) = depth {
                    crate::mutation::set_subtree_depth_component(&mut node, depth);
                }
                node.parent_id = Some(parent.clone());
                let turn_id = parent_row.turn.clone();
                let (siblings, max_offset) = self.direct_children(&turn_id, &parent)?;
                node.offset = offset_i32(max_offset.map_or(0, |offset| offset + 1))?;
                let index = siblings.len();
                let mut child_ids = siblings;
                child_ids.push(node.id.clone());
                insert_subtree_trace_ops(&node, &parent, index, Some(&child_ids), &mut |op| {
                    trace.push_op(&op)?;
                    index_ops.push((op_ordinal, op));
                    op_ordinal += 1;
                    Ok(())
                })?;
                let admission = self.admit_child(&turn_id, &node, to_revision)?;
                (index, admission)
            };
            note_admission(
                admission,
                &mut first_seen_additions,
                &mut lifecycle_seen_additions,
            );

            counter += 1;
            let trace_id = trace_identity(&op, created_at_ns, counter);
            events.push(ResolvedOperationTrace {
                trace_id,
                op_kind: op.kind().to_string(),
                idempotency_key: None,
                created_at_ns,
                ops: Vec::new(),
                produced_refs: produced_refs_for_insert(&node, &root_id, &branch_id),
                aux: envelope.aux.clone(),
            });
            applied.push(op.kind().to_string());

            let mut projected = node.project_component_value();
            externalize_large_content_in_value(
                &mut projected,
                &mut content,
                DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES,
            );
            let mut produced_ids = Vec::new();
            subtree_ids(&node, &mut produced_ids);
            inserted_nodes += produced_ids.len() as u64;
            inserted_messages += count_message_turns(&node);
            inserted_chars += inline_content_chars(&node);
            inserts.push(PreparedInsert {
                parent_id: parent,
                index,
                component: node,
                projected,
                produced_ids,
            });
        }
        trace.check_limits()?;
        if trace.op_count() != op_ordinal {
            return Err(PactError::internal(format!(
                "Pact trace spool holds {} ops but {op_ordinal} were emitted",
                trace.op_count()
            )));
        }
        let trace_symbols = trace.symbol_count();
        let trace_scratch_bytes = trace.scratch_bytes();

        let repaired = self.repair_membership(to_revision)?;
        note_admission(
            repaired,
            &mut first_seen_additions,
            &mut lifecycle_seen_additions,
        );
        let lifecycle_seen_removals = self.stale_lifecycle_ids()?;
        let mut base_ref = self.base_ref.clone();
        if from_revision == 0 && base_ref.content_ref.is_none() {
            let record = birth_base_record(snapshot, &branch_id, &mut content)?;
            base_ref.revision = Some(0);
            base_ref.content_ref = Some(record);
        }
        let delta = LedgerDelta {
            lifecycle_tick: self.lifecycle_tick,
            lifecycle_birth_tick: birth_tick,
            base_ref: base_ref.clone(),
            first_seen_additions,
            lifecycle_seen_additions,
            lifecycle_seen_removals,
            record_canonical_trace: true,
            record_public_trace: true,
            derived_depth: self.derived_depth,
        };
        self.base_ref = base_ref;

        let mut new_content = Vec::new();
        for record in content.records.into_values() {
            let key = IndexKey::ContentId {
                id: record.id.clone(),
            };
            let persisted =
                super::index::read_index_value_from_pages(snapshot, self.index_root, &key)?
                    .is_some();
            if !persisted {
                new_content.push(record);
            }
        }

        let content_by_id = new_content
            .iter()
            .map(|record| (record.id.as_str(), record))
            .collect::<BTreeMap<_, _>>();
        let mut text_changes = Vec::new();
        let mut search_changes = Vec::new();
        for (ordinal, op) in &index_ops {
            for change in text_changes_from_op(&branch_id, to_revision, op, &mut |content_id| {
                content_by_id
                    .get(content_id)
                    .map(|record| content_record_bytes(record))
                    .transpose()
            })? {
                text_changes.push(IndexedTextChange {
                    revision: to_revision,
                    ordinal: text_changes.len() as u64,
                    change,
                });
            }
            if let Some(change) = search_change_from_op(op)? {
                search_changes.push(IndexedSearchChange {
                    revision: to_revision,
                    ordinal: *ordinal,
                    change,
                });
            }
        }

        let before_ref = PactRef {
            root_id: root_id.clone(),
            branch_id: branch_id.clone(),
            revision: Some(from_revision),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };
        let after_ref = PactRef {
            revision: Some(to_revision),
            ..before_ref.clone()
        };
        let public_trace = build_sealed_public_trace(SealedTraceRequest {
            root_id: &root_id,
            branch_id: &branch_id,
            next_rev: to_revision,
            boundary: BoundaryKind::Adhoc,
            before_ref: &before_ref,
            after_ref: &after_ref,
            applied,
            events: &events,
            extensions,
            replay_trace: None,
            aux: BTreeMap::new(),
        })?;

        let mut metadata = snapshot.metadata()?;
        metadata.trace_count = metadata.trace_count.saturating_add(1);
        *metadata.node_counts.entry(branch_id.clone()).or_default() += inserted_nodes as usize;
        *metadata
            .message_counts
            .entry(branch_id.clone())
            .or_default() += inserted_messages as usize;
        metadata
            .head_revisions
            .insert(branch_id.clone(), to_revision);
        metadata
            .head_transaction_ids
            .insert(branch_id.clone(), public_trace.transaction_id.clone());
        if self.derived_depth {
            metadata.derived_depth_branches.insert(branch_id.clone());
        }

        metadata
            .lifecycle_policy_free_branches
            .insert(branch_id.clone());
        let stats = metadata
            .stats
            .entry(branch_id.clone())
            .or_insert_with(|| PactStoreStats {
                node_count: 0,
                edge_count: 0,
                blob_count: 0,
                content_chars: 0,
            });
        stats.node_count += inserted_nodes as usize;
        stats.edge_count += inserted_nodes as usize;
        stats.content_chars += inserted_chars;

        Ok(PreparedTransaction {
            transaction_id: public_trace.transaction_id.clone(),
            from_revision,
            to_revision,
            trace,
            public_trace,
            delta,
            new_content,
            inserts,
            shift_ops,
            index_ops,
            text_changes,
            search_changes,
            metadata,
            inserted_nodes,
            inserted_messages,
            trace_symbols,
            trace_scratch_bytes,
            order_repair: None,
        })
    }

    pub(crate) fn prepare_maintenance(
        &mut self,
        snapshot: &Snapshot,
        options: &PactMemoryOptions,
        order_repair: MessageOrderRepairPlan,
        marker: (String, Value),
    ) -> PactResult<PreparedTransaction> {
        let root_id = self.root_id.clone();
        let branch_id = self.branch_id.clone();
        let from_revision = self.head_revision;
        if from_revision == 0 {
            return Err(PactError::internal(
                "Pact maintenance transaction requested at revision 0",
            ));
        }
        let to_revision = from_revision
            .checked_add(1)
            .ok_or_else(|| PactError::unsupported("branch revision exceeds the format limit"))?;
        let trace = TraceSpool::new(
            from_revision,
            to_revision,
            options.io_chunk_bytes,
            options.scratch_cache_bytes,
        )?;
        let delta = LedgerDelta {
            lifecycle_tick: self.lifecycle_tick,
            lifecycle_birth_tick: self.lifecycle_tick + 1,
            base_ref: self.base_ref.clone(),
            first_seen_additions: BTreeMap::new(),
            lifecycle_seen_additions: BTreeMap::new(),
            lifecycle_seen_removals: Vec::new(),
            record_canonical_trace: true,
            record_public_trace: true,
            derived_depth: self.derived_depth,
        };
        let before_ref = PactRef {
            root_id: root_id.clone(),
            branch_id: branch_id.clone(),
            revision: Some(from_revision),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };
        let after_ref = PactRef {
            revision: Some(to_revision),
            ..before_ref.clone()
        };
        let mut aux = BTreeMap::new();
        aux.insert(marker.0, marker.1);
        let public_trace = build_sealed_public_trace(SealedTraceRequest {
            root_id: &root_id,
            branch_id: &branch_id,
            next_rev: to_revision,
            boundary: BoundaryKind::Adhoc,
            before_ref: &before_ref,
            after_ref: &after_ref,
            applied: Vec::new(),
            events: &[],
            extensions: Vec::new(),
            replay_trace: None,
            aux,
        })?;
        let mut metadata = snapshot.metadata()?;
        metadata.trace_count = metadata.trace_count.saturating_add(1);
        metadata
            .head_revisions
            .insert(branch_id.clone(), to_revision);
        metadata
            .head_transaction_ids
            .insert(branch_id, public_trace.transaction_id.clone());
        let trace_symbols = trace.symbol_count();
        let trace_scratch_bytes = trace.scratch_bytes();
        Ok(PreparedTransaction {
            transaction_id: public_trace.transaction_id.clone(),
            from_revision,
            to_revision,
            trace,
            public_trace,
            delta,
            new_content: Vec::new(),
            inserts: Vec::new(),
            shift_ops: 0,
            index_ops: Vec::new(),
            text_changes: Vec::new(),
            search_changes: Vec::new(),
            metadata,
            inserted_nodes: 0,
            inserted_messages: 0,
            trace_symbols,
            trace_scratch_bytes,
            order_repair: Some(order_repair),
        })
    }

    fn emit_depth_shifts(&self, trace: &mut TraceSpool) -> PactResult<u64> {
        let mut emitted = 0;
        let mut turns = self.turn_pages(SHIFT_PAGE_ENTRIES);
        loop {
            let page = turns.next_page(self.pages())?;
            if page.is_empty() {
                break;
            }
            for (_, value) in page {
                let turn_id = String::from_utf8(value)
                    .map_err(|_| PactError::corrupt("Pact turn id is not UTF-8"))?;
                let row = self.node(&turn_id)?.ok_or_else(|| {
                    PactError::corrupt(format!("Pact turn {turn_id} is missing from the state"))
                })?;
                if !is_shiftable(&row) {
                    continue;
                }
                let to_depth = self
                    .current_depth(&row)?
                    .ok_or_else(|| PactError::corrupt("shiftable turn without a depth"))?
                    + 1;
                let mut nodes = self.turn_node_pages(&turn_id, SHIFT_PAGE_ENTRIES);
                loop {
                    let page = nodes.next_page(self.pages())?;
                    if page.is_empty() {
                        break;
                    }
                    for (_, value) in page {
                        let node_id = String::from_utf8(value)
                            .map_err(|_| PactError::corrupt("Pact node id is not UTF-8"))?;
                        trace.push_op(&depth_trace_op(&node_id, to_depth))?;
                        emitted += 1;
                    }
                }
            }
        }
        Ok(emitted)
    }
}

fn birth_base_record(
    snapshot: &Snapshot,
    branch_id: &str,
    content: &mut ContentStore,
) -> PactResult<String> {
    let ledger = snapshot.replay_ledger()?;
    let branch = ledger
        .branches
        .get(branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    let before_tree =
        project_branch_component_tree(&ledger, branch, Some(branch.head_revision))?.project_value();
    let base_json = serde_json::to_string(&before_tree)
        .map_err(|err| PactError::codec(format!("failed to encode branch birth base: {err}")))?;
    Ok(content
        .put_inline(
            "pact.branch.base",
            &base_json,
            DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES,
        )
        .id)
}
