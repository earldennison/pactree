use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::checkpoint_stream::{MembershipSink, stream_branch_membership};
use super::index::IndexKey;
use super::snapshot::Snapshot;
use super::stream_decode::{baseline_checkpoint_reader, transaction_ledger_delta};
use super::transaction_stream::GENERATION_PAGE_ENTRIES;
use crate::error::{PactError, PactResult};
use crate::store::{
    BranchEventIndex, ContentStore, ContextTreeLedger, LedgerHistory, PACT_DERIVED_DEPTH_AUX_KEY,
    PactBranch, PactRef, PactView, ROOT_STORE_SCHEMA,
};

#[cfg(test)]
mod tests;

#[derive(Default)]
struct HeadMembership {
    first_seen: BTreeMap<String, u64>,
    lifecycle_seen: BTreeMap<String, u64>,
}

impl MembershipSink for HeadMembership {
    fn first_seen(&mut self, component_id: &str, revision: u64) -> PactResult<()> {
        self.first_seen.insert(component_id.to_string(), revision);
        Ok(())
    }

    fn lifecycle_seen(&mut self, component_id: &str, tick: u64) -> PactResult<()> {
        self.lifecycle_seen.insert(component_id.to_string(), tick);
        Ok(())
    }
}

pub(crate) fn head_ledger(snapshot: &Snapshot) -> PactResult<Option<ContextTreeLedger>> {
    let Some(metadata) = snapshot.metadata_optional()? else {
        return Ok(None);
    };
    if metadata.branch_count != 1
        || !metadata.composition_branches.is_empty()
        || !snapshot.has_parent_child_facet()?
    {
        return Ok(None);
    }
    let branch_id = metadata.current_branch_id.clone();
    let Some(head_revision) = metadata.head_revisions.get(&branch_id).copied() else {
        return Ok(None);
    };

    let addresses = transaction_addresses(snapshot)?;
    let Some((&baseline, later)) = addresses.split_first() else {
        return Ok(None);
    };
    if later.is_empty() {
        return Ok(None);
    }

    let Some((reader, in_sections)) = baseline_checkpoint_reader(snapshot.mapping(), baseline)?
    else {
        return Ok(None);
    };
    let mut membership = HeadMembership::default();
    let scalars = stream_branch_membership(reader, in_sections, &branch_id, &mut membership)?;
    if scalars.has_parent_branch || scalars.has_fork_ref {
        return Ok(None);
    }
    let mut lifecycle_tick = scalars.lifecycle_tick;
    let mut base_ref = scalars.base_ref.unwrap_or_else(|| PactRef {
        root_id: metadata.root_id.clone(),
        branch_id: branch_id.clone(),
        revision: None,
        transaction_id: None,
        component_id: None,
        content_ref: None,
    });
    let mut derived_depth = scalars.derived_depth;
    for address in later {
        let Some(delta) = transaction_ledger_delta(snapshot.mapping(), *address)? else {
            continue;
        };
        apply_delta(
            &delta,
            &mut membership,
            &mut lifecycle_tick,
            &mut base_ref,
            &mut derived_depth,
        )?;
    }

    if !derived_depth {
        return Ok(None);
    }

    let Some(mut root_shell) = scalars.root else {
        return Ok(None);
    };
    let children = snapshot.root_children_subtrees(&branch_id, &metadata.root_id)?;
    root_shell.insert("children".to_string(), Value::Array(children));
    let mut root = Value::Object(root_shell);
    crate::mutation::derive_turn_depths_value(&mut root);

    let mut content_ids = BTreeSet::new();
    collect_content_ids(&root, &mut content_ids);
    let mut content = ContentStore::default();
    for record in snapshot.content_records(&content_ids)? {
        content.records.insert(record.id.clone(), record);
    }
    content.rebuild_hash_index();

    let projection = crate::store::build_projection(&root, &membership.first_seen, 1024)?;
    let mut view = PactView::from_projection_with_lifecycle_seen(
        projection,
        branch_id.clone(),
        head_revision,
        membership.lifecycle_seen,
        lifecycle_tick.saturating_add(1),
    );
    view.tree.first_seen = membership.first_seen;

    let newest = later.last().copied().unwrap_or(baseline);
    let (transaction, _) =
        snapshot.decode_transaction_at_with(newest, Some(&metadata.derived_depth_branches))?;
    let mut public_trace = transaction.public_trace;
    if transaction
        .ledger_delta
        .as_ref()
        .is_some_and(|delta| delta.record_public_trace)
    {
        public_trace.replay_trace = Some(transaction.trace);
    }
    let mut event_index = BranchEventIndex::default();
    event_index
        .by_transaction
        .insert(public_trace.transaction_id.clone(), vec![branch_id.clone()]);
    event_index
        .transaction_traces
        .insert(public_trace.transaction_id.clone(), public_trace);

    let mut aux = BTreeMap::new();
    if derived_depth {
        aux.insert(PACT_DERIVED_DEPTH_AUX_KEY.to_string(), Value::Bool(true));
    }
    let branch = PactBranch {
        branch_id: branch_id.clone(),
        root_id: metadata.root_id.clone(),
        parent_branch_id: None,
        fork_ref: None,
        base_ref,
        head_revision,
        lifecycle_tick,
        transaction_traces: Vec::new(),
        current: view,
        aux,
    };
    let parent = scalars
        .ledger_parent
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| PactError::corrupt(format!("invalid Pact ledger parent: {error}")))?;
    snapshot.release_oversized_cache_entries();
    Ok(Some(ContextTreeLedger {
        schema: ROOT_STORE_SCHEMA.to_string(),
        root_id: metadata.root_id.clone(),
        parent,
        base: None,
        branches: BTreeMap::from([(branch_id.clone(), branch)]),
        content,
        event_index,
        current_branch_id: branch_id,
        history: LedgerHistory::Lazy {
            trace_count: metadata.trace_count,
        },
    }))
}

fn transaction_addresses(snapshot: &Snapshot) -> PactResult<Vec<u64>> {
    let mut addresses = Vec::new();
    let mut next_generation = 0_u64;
    loop {
        let entries = snapshot.index_range(
            &IndexKey::Generation {
                generation: next_generation,
            },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            GENERATION_PAGE_ENTRIES,
            false,
        )?;
        if entries.is_empty() {
            break;
        }
        let mut last = next_generation;
        for (key, value) in &entries {
            let IndexKey::Generation { generation } = IndexKey::decode(key)? else {
                return Err(PactError::corrupt(
                    "Pact generation range returned another namespace",
                ));
            };
            last = generation;
            addresses.push(super::codec::decode_u64_value(
                value,
                "Pact transaction address",
            )?);
        }
        if entries.len() < GENERATION_PAGE_ENTRIES {
            break;
        }
        next_generation = last
            .checked_add(1)
            .ok_or_else(|| PactError::corrupt("Pact generation overflow"))?;
    }
    Ok(addresses)
}

fn apply_delta(
    delta: &Value,
    membership: &mut HeadMembership,
    lifecycle_tick: &mut u64,
    base_ref: &mut PactRef,
    derived_depth: &mut bool,
) -> PactResult<()> {
    if let Some(tick) = delta.get("lifecycle_tick").and_then(Value::as_u64) {
        *lifecycle_tick = tick;
    }
    if let Some(value) = delta.get("base_ref") {
        *base_ref = serde_json::from_value(value.clone()).map_err(|error| {
            PactError::corrupt(format!("invalid Pact ledger delta base_ref: {error}"))
        })?;
    }
    if let Some(additions) = delta.get("first_seen_additions").and_then(Value::as_object) {
        for (id, revision) in additions {
            membership
                .first_seen
                .insert(id.clone(), revision.as_u64().unwrap_or_default());
        }
    }
    if let Some(additions) = delta
        .get("lifecycle_seen_additions")
        .and_then(Value::as_object)
    {
        for (id, tick) in additions {
            membership
                .lifecycle_seen
                .insert(id.clone(), tick.as_u64().unwrap_or_default());
        }
    }
    if let Some(removals) = delta
        .get("lifecycle_seen_removals")
        .and_then(Value::as_array)
    {
        for id in removals.iter().filter_map(Value::as_str) {
            membership.lifecycle_seen.remove(id);
        }
    }
    if delta.get("derived_depth").and_then(Value::as_bool) == Some(true) {
        *derived_depth = true;
    }
    Ok(())
}

fn collect_content_ids(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, field) in object {
                if key.ends_with("_ref") {
                    match field {
                        Value::String(id) => {
                            out.insert(id.clone());
                        }
                        Value::Object(reference) => {
                            if let Some(id) = reference.get("id").and_then(Value::as_str) {
                                out.insert(id.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                collect_content_ids(field, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_content_ids(item, out);
            }
        }
        _ => {}
    }
}
