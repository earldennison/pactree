use super::*;

use crate::atomic_output::{PublishMode, publish_bytes};
use crate::error::{PactError, PactResult};
use crate::file_format::storage::{node_records_by_id, node_records_from_records};
use crate::transaction_trace::TraceOpKind;

pub(super) fn branch_heads_from_records(records: &[ScannedRecord]) -> BTreeMap<String, u64> {
    let mut heads = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "branch") {
        if let (Some(branch_id), Some(head_revision)) = (
            record.payload.get("branch_id").and_then(Value::as_str),
            record.payload.get("head_revision").and_then(Value::as_u64),
        ) {
            heads.insert(branch_id.to_string(), head_revision);
        }
    }
    heads
}

fn latest_lifecycle_tick_for_branch(records: &[ScannedRecord], branch_id: &str) -> Option<u64> {
    records
        .iter()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .filter(|commit| commit.branch_id == branch_id)
        .filter_map(|commit| commit.lifecycle_tick)
        .max()
}

fn lifecycle_seen_from_commit_records(
    records: &[ScannedRecord],
    branch_id: &str,
    mut lifecycle_seen: BTreeMap<String, u64>,
    initial_tick: u64,
) -> BTreeMap<String, u64> {
    let mut commits = records
        .iter()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .filter(|commit| commit.branch_id == branch_id)
        .collect::<Vec<_>>();
    commits.sort_by_key(|commit| (commit.revision, commit.cycle));

    let mut current_tick = initial_tick;
    for commit in commits {
        let birth_tick = commit.lifecycle_tick.unwrap_or(current_tick + 1);
        for op in &commit.ops {
            match op {
                CommitRecordOp::InsertComponent { node_ref, .. }
                | CommitRecordOp::ReplaceComponent { node_ref, .. } => {
                    lifecycle_seen.entry(node_ref.clone()).or_insert(birth_tick);
                }
                CommitRecordOp::AppendComponents { node_refs, .. } => {
                    for node_ref in node_refs {
                        lifecycle_seen.entry(node_ref.clone()).or_insert(birth_tick);
                    }
                }
                CommitRecordOp::DeleteComponent { component_id } => {
                    lifecycle_seen.remove(component_id);
                }
                CommitRecordOp::SetComponentAux { .. }
                | CommitRecordOp::UnsetComponentAux { .. }
                | CommitRecordOp::AttachContentRef { .. }
                | CommitRecordOp::ReplaceContentRange { .. } => {}
            }
        }
        if let Some(tick) = commit.lifecycle_tick {
            current_tick = tick;
        }
    }
    lifecycle_seen
}

pub(crate) fn ledger_from_records(records: &[ScannedRecord]) -> PactResult<ContextTreeLedger> {
    ledger_from_records_with_commits(records, true)
}

pub(super) fn ledger_from_records_with_commits(
    records: &[ScannedRecord],
    apply_commits: bool,
) -> PactResult<ContextTreeLedger> {
    let root = records
        .iter()
        .find(|record| record.kind == "root")
        .ok_or_else(|| PactError::corrupt(".pact file has no root record".to_string()))?;
    let root_id = root
        .payload
        .get("root_id")
        .and_then(Value::as_str)
        .ok_or_else(|| PactError::corrupt("root record missing root_id".to_string()))?
        .to_string();
    let current_branch_id = root
        .payload
        .get("current_branch_id")
        .and_then(Value::as_str)
        .ok_or_else(|| PactError::corrupt("root record missing current_branch_id".to_string()))?
        .to_string();
    let parent_provenance = match root.payload.get("parent") {
        Some(Value::Null) | None => None,
        Some(value) => Some(
            serde_json::from_value::<crate::store::PactParentProvenance>(value.clone()).map_err(
                |err| PactError::codec(format!("invalid root record parent provenance: {err}")),
            )?,
        ),
    };
    let base_provenance = match root.payload.get("base") {
        Some(Value::Null) | None => None,
        Some(value) => Some(
            serde_json::from_value::<crate::store::PactBaseProvenance>(value.clone()).map_err(
                |err| PactError::codec(format!("invalid root record base provenance: {err}")),
            )?,
        ),
    };

    let chunks = content_chunks_from_records(records)?;

    let mut content = crate::store::ContentStore::default();
    let mut file_local_content_ids = Vec::new();
    for record in records.iter().filter(|record| record.kind == "content") {
        let payload: ContentPayload = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid content record payload: {err}")))?;
        if payload.kind != "pact.branch.base" {
            file_local_content_ids.push(payload.id.clone());
        }
        let (inline, inline_bytes) = if payload.text.is_some() {
            let bytes = decode_content_payload_bytes(&payload)?;
            if payload.chars.is_some() {
                (
                    Some(String::from_utf8(bytes).map_err(|err| {
                        PactError::corrupt(format!(
                            "content {} payload is not UTF-8: {err}",
                            payload.id
                        ))
                    })?),
                    None,
                )
            } else {
                (None, Some(bytes))
            }
        } else {
            let bytes = chunks
                .get(&payload.id)
                .map(|parts| {
                    Ok::<Vec<u8>, PactError>(
                        parts
                            .iter()
                            .flat_map(|(_, chunk)| chunk.iter().copied())
                            .collect::<Vec<_>>(),
                    )
                })
                .transpose()?;
            if let Some(bytes) = bytes {
                if payload.chars.is_some() {
                    (
                        Some(String::from_utf8(bytes).map_err(|err| {
                            PactError::corrupt(format!(
                                "content {} chunks are not UTF-8: {err}",
                                payload.id
                            ))
                        })?),
                        None,
                    )
                } else {
                    (None, Some(bytes))
                }
            } else {
                (None, None)
            }
        };
        let preview = payload
            .preview_inline
            .clone()
            .unwrap_or_else(|| inline.as_deref().map(inline_preview).unwrap_or_default());
        let storage = if let Some(inline) = inline {
            ContentStorageRef {
                kind: "inline".to_string(),
                inline: Some(inline),
                inline_bytes: None,
                path: None,
                external_ref: None,
            }
        } else if let Some(bytes) = inline_bytes {
            ContentStorageRef {
                kind: "inline_bytes".to_string(),
                inline: None,
                inline_bytes: Some(bytes),
                path: None,
                external_ref: None,
            }
        } else {
            ContentStorageRef {
                kind: "external".to_string(),
                inline: None,
                inline_bytes: None,
                path: None,
                external_ref: Some(crate::components::ExternalRef {
                    kind: "content_storage".to_string(),
                    uri: Some(payload.id.clone()),
                    role: None,
                    data: None,
                }),
            }
        };
        let content_record = ContentRecord {
            id: payload.id.clone(),
            kind: payload.kind,
            hash: payload.hash.clone(),
            bytes: payload.bytes,
            chars: payload.chars,
            preview,
            storage,
        };
        content
            .records_by_hash
            .insert(payload.hash, payload.id.clone());
        content.records.insert(payload.id, content_record);
    }

    let node_records = node_records_from_records(records);
    let mut branches = BTreeMap::new();
    let mut branch_node_refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "branch") {
        let branch_id = record
            .payload
            .get("branch_id")
            .and_then(Value::as_str)
            .ok_or_else(|| PactError::codec("branch record missing branch_id".to_string()))?
            .to_string();
        let refs = branch_current_node_refs(&record.payload);
        crate::store::reject_removed_patch_history(&record.payload)?;
        let branch: PactBranch = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid branch record payload: {err}")))?;
        branch_node_refs.insert(branch_id.clone(), refs);
        branches.insert(branch.branch_id.clone(), branch);
    }
    if !branches.contains_key(&current_branch_id) {
        return Err(PactError::corrupt(format!(
            "current branch {current_branch_id} missing from branch records"
        )));
    }
    for (branch_id, branch) in branches.iter_mut() {
        hydrate_branch_current_from_node_records(
            branch,
            branch_node_refs.get(branch_id),
            &node_records,
        )?;
    }

    let mut event_index = crate::store::BranchEventIndex::default();
    for record in records.iter().filter(|record| record.kind == "trace") {
        let trace: PactTransactionTrace = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid transaction trace payload: {err}")))?;
        if let Some(replay_trace) = trace.replay_trace.clone()
            && let Some(branch) = branches.get_mut(&trace.branch_id)
            && !branch
                .transaction_traces
                .iter()
                .any(|existing| existing == &replay_trace)
        {
            branch.transaction_traces.push(replay_trace);
        }
        event_index
            .by_transaction
            .entry(trace.transaction_id.clone())
            .or_default()
            .push(trace.branch_id.clone());
        event_index
            .transaction_traces
            .insert(trace.transaction_id.clone(), trace);
    }

    let mut root_store = ContextTreeLedger {
        history: Default::default(),
        schema: crate::store::ROOT_STORE_SCHEMA.to_string(),
        root_id,
        parent: parent_provenance,
        base: base_provenance,
        branches,
        content,
        event_index,
        current_branch_id,
    };

    hydrate_commit_record_branches(records, apply_commits, &mut root_store)?;

    if !file_local_content_ids.is_empty() {
        for branch in root_store.branches.values_mut() {
            rewrite_file_local_blob_refs(&mut branch.current, &file_local_content_ids);
        }
    }

    Ok(root_store)
}

fn hydrate_commit_record_branches(
    records: &[ScannedRecord],
    apply_commits: bool,
    root_store: &mut ContextTreeLedger,
) -> PactResult<()> {
    if !apply_commits || !records.iter().any(|record| record.kind == "commit") {
        return Ok(());
    }

    for branch_id in commit_record_branch_ids(records) {
        ensure_commit_record_branch(root_store, &branch_id)?;
        let Some(latest_cycle) = latest_commit_cycle_for_branch(records, &branch_id) else {
            continue;
        };
        if latest_cycle == 0 {
            continue;
        }
        hydrate_commit_record_branch(records, root_store, &branch_id, latest_cycle)?;
    }
    Ok(())
}

fn hydrate_commit_record_branch(
    records: &[ScannedRecord],
    root_store: &mut ContextTreeLedger,
    branch_id: &str,
    latest_cycle: u64,
) -> PactResult<()> {
    let content_texts = content_texts_from_records(records);
    let (
        commit_trace_start_cycle,
        derive_commit_traces,
        synthesize_commit_trace_base_ref,
        base_lifecycle_seen,
        base_lifecycle_tick,
        base_first_seen,
    ) = {
        let branch = root_store
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        let contiguous_trace_revision = contiguous_transaction_trace_revision(branch);
        let first_commit_cycle = first_commit_cycle_for_branch(records, branch_id);
        let derive_commit_traces = contiguous_trace_revision < latest_cycle;
        let synthesize_commit_trace_base_ref = branch.base_ref.content_ref.is_none()
            && (derive_commit_traces || contiguous_trace_revision > 0);
        let commit_trace_start_cycle = first_commit_cycle
            .filter(|cycle| *cycle > contiguous_trace_revision)
            .map(|_| contiguous_trace_revision)
            .unwrap_or(0);
        (
            commit_trace_start_cycle,
            derive_commit_traces,
            synthesize_commit_trace_base_ref,
            branch.current.lifecycle_seen.clone(),
            branch.lifecycle_tick,
            branch.current.tree.first_seen.clone(),
        )
    };
    let commit_trace_base_content_id = if synthesize_commit_trace_base_ref {
        let commit_trace_base_tree =
            commit_trace_base_from_records(records, branch_id, commit_trace_start_cycle)?;
        let commit_trace_base_json =
            serde_json::to_string(&commit_trace_base_tree).map_err(|err| {
                PactError::codec(format!("failed to encode commit trace base tree: {err}"))
            })?;
        Some(
            root_store
                .content
                .put_inline(
                    "pact.branch.commit_trace_base",
                    &commit_trace_base_json,
                    1024,
                )
                .id,
        )
    } else {
        None
    };
    let commit_traces = if derive_commit_traces {
        transaction_traces_from_commit_records_from(
            records,
            branch_id,
            commit_trace_start_cycle,
            latest_cycle,
        )?
    } else {
        Vec::new()
    };
    let authoritative_trace_components = records
        .iter()
        .filter(|record| record.kind == "trace")
        .filter_map(|record| {
            serde_json::from_value::<PactTransactionTrace>(record.payload.clone()).ok()
        })
        .filter(|trace| trace.branch_id == branch_id)
        .filter_map(|trace| {
            trace.replay_trace.map(|replay| {
                (
                    (replay.from_rev, replay.to_rev),
                    replay
                        .ops
                        .into_iter()
                        .map(|op| op.component_id)
                        .collect::<BTreeSet<_>>(),
                )
            })
        })
        .fold(
            BTreeMap::<_, BTreeSet<_>>::new(),
            |mut by_interval, (interval, ids)| {
                by_interval.entry(interval).or_default().extend(ids);
                by_interval
            },
        );
    if let Some(branch) = root_store.branches.get_mut(branch_id) {
        if synthesize_commit_trace_base_ref && branch.base_ref.content_ref.is_none() {
            branch.base_ref.revision = Some(commit_trace_start_cycle);
            branch.base_ref.content_ref = commit_trace_base_content_id.clone();
        }
        for mut trace in commit_traces {
            if let Some(component_ids) =
                authoritative_trace_components.get(&(trace.from_rev, trace.to_rev))
            {
                trace
                    .ops
                    .retain(|op| !component_ids.contains(&op.component_id));
            }
            if trace.ops.is_empty() {
                continue;
            }
            if let Some(existing) = branch.transaction_traces.iter_mut().find(|existing| {
                existing.from_rev == trace.from_rev && existing.to_rev == trace.to_rev
            }) {
                for op in trace.ops {
                    if !existing.ops.contains(&op) {
                        existing.ops.push(op);
                    }
                }
            } else if !branch
                .transaction_traces
                .iter()
                .any(|existing| existing == &trace)
            {
                branch.transaction_traces.push(trace);
            }
        }
    }

    let node_records = node_records_by_id(records);
    if let Some(branch) = root_store.branches.get_mut(branch_id) {
        try_assemble_branch_current_from_head_records(
            records,
            branch,
            &node_records,
            latest_cycle,
        )?;
    }

    let branch = root_store
        .branches
        .get(branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?
        .clone();
    let projected =
        crate::store::project_branch_component_tree(root_store, &branch, Some(latest_cycle))?
            .project_value();
    let latest_lifecycle_tick =
        latest_lifecycle_tick_for_branch(records, branch_id).unwrap_or(base_lifecycle_tick);
    let lifecycle_seen = lifecycle_seen_from_commit_records(
        records,
        branch_id,
        base_lifecycle_seen,
        base_lifecycle_tick,
    );
    let mut first_seen = base_first_seen;
    let probe = crate::store::build_projection(&projected, &first_seen, 1024)?;
    for component_id in probe.component_ids() {
        first_seen
            .entry(component_id.clone())
            .or_insert(latest_cycle);
    }
    let mut projection = crate::store::build_projection(&projected, &first_seen, 1024)?;
    rewrite_synthetic_blob_refs_from_projected_tree(&mut projection, &projected, &content_texts);
    if let Some(branch) = root_store.branches.get_mut(branch_id) {
        branch.current = crate::store::PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_id.to_string(),
            latest_cycle,
            lifecycle_seen,
            latest_lifecycle_tick,
        );
        branch.head_revision = branch.head_revision.max(latest_cycle);
        branch.lifecycle_tick = branch.lifecycle_tick.max(latest_lifecycle_tick);
    }
    Ok(())
}

fn try_assemble_branch_current_from_head_records(
    records: &[ScannedRecord],
    branch: &mut PactBranch,
    node_records: &BTreeMap<String, Value>,
    latest_cycle: u64,
) -> PactResult<()> {
    if node_records.is_empty() {
        return Ok(());
    }
    let Some(head) = latest_head_record_for_branch(records, &branch.branch_id) else {
        return Ok(());
    };
    if head.head_revision != latest_cycle || head.head_cycle != latest_cycle {
        log_head_reconstruction_fallback(
            &branch.branch_id,
            format!(
                "head record revision/cycle {}/{} does not match latest commit cycle {latest_cycle}",
                head.head_revision, head.head_cycle
            ),
        );
        return Ok(());
    }
    if super::projection_replay::has_frozen_anchor_at_or_before(records, head.head_cycle)? {
        return Ok(());
    }
    let Some(root_node_ref) = head.root_node_ref.as_deref() else {
        log_head_reconstruction_fallback(&branch.branch_id, "head record has no root_node_ref");
        return Ok(());
    };

    let children_by_parent = children_by_parent_from_node_records(node_records);
    let mut overlay = head_trace_overlay(branch, head.head_revision);
    apply_paired_tool_deletes_from_head_overlay(node_records, &mut overlay);
    let mut seen = BTreeSet::new();
    let tree = match node_record_head_tree_value(
        root_node_ref,
        node_records,
        &children_by_parent,
        &overlay,
        root_node_ref,
        &mut seen,
    ) {
        Ok(tree) => tree,
        Err(err) => {
            log_head_reconstruction_fallback(&branch.branch_id, err);
            return Ok(());
        }
    };

    let first_seen = branch.current.tree.first_seen.clone();
    let lifecycle_seen = branch.current.lifecycle_seen.clone();
    let branch_refs = branch.current.branch_refs.clone();
    let content_refs = branch.current.content_refs.clone();
    let projection = match crate::store::build_projection(&tree, &first_seen, 1024) {
        Ok(projection) => projection,
        Err(err) => {
            log_head_reconstruction_fallback(&branch.branch_id, err);
            return Ok(());
        }
    };
    if !head_projection_matches_latest_commit(records, &head, &projection)? {
        log_head_reconstruction_fallback(
            &branch.branch_id,
            "reconstructed head does not contain the latest commit surface",
        );
        return Ok(());
    }
    let mut view = crate::store::PactView::from_projection_with_lifecycle_seen(
        projection,
        branch.branch_id.clone(),
        head.head_revision,
        lifecycle_seen,
        branch.lifecycle_tick,
    );
    view.tree.first_seen = first_seen;
    view.lifecycle_seen = branch.current.lifecycle_seen.clone();
    view.branch_refs = branch_refs;
    if !content_refs.is_empty() {
        view.content_refs = content_refs;
    }
    branch.current = view;
    branch.head_revision = head.head_revision;
    if let Some(lifecycle_tick) = head.lifecycle_tick {
        branch.lifecycle_tick = branch.lifecycle_tick.max(lifecycle_tick);
    }
    Ok(())
}

fn head_projection_matches_latest_commit(
    records: &[ScannedRecord],
    head: &HeadRecord,
    projection: &crate::store::PactProjection,
) -> PactResult<bool> {
    let Some(commit) = records
        .iter()
        .rev()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .find(|commit| {
            commit.branch_id == head.branch_id
                && commit.revision == head.head_revision
                && commit.cycle == head.head_cycle
        })
    else {
        return Ok(false);
    };
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent { node_ref, .. }
            | CommitRecordOp::ReplaceComponent { node_ref, .. } => {
                if projection.component(node_ref).is_none() {
                    return Ok(false);
                }
            }
            CommitRecordOp::AppendComponents { node_refs, .. } => {
                if node_refs
                    .iter()
                    .any(|node_ref| projection.component(node_ref).is_none())
                {
                    return Ok(false);
                }
            }
            CommitRecordOp::DeleteComponent { component_id } => {
                if projection.component(component_id).is_some() {
                    return Ok(false);
                }
            }
            CommitRecordOp::SetComponentAux { component_id, .. }
            | CommitRecordOp::UnsetComponentAux { component_id, .. }
            | CommitRecordOp::AttachContentRef { component_id, .. }
            | CommitRecordOp::ReplaceContentRange { component_id, .. } => {
                if projection.component(component_id).is_none() {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn latest_head_record_for_branch(records: &[ScannedRecord], branch_id: &str) -> Option<HeadRecord> {
    records
        .iter()
        .rev()
        .filter(|record| record.kind == "head")
        .filter_map(|record| serde_json::from_value::<HeadRecord>(record.payload.clone()).ok())
        .find(|head| head.branch_id == branch_id)
}

fn node_record_head_tree_value(
    component_id: &str,
    records: &BTreeMap<String, Value>,
    children_by_parent: &BTreeMap<String, Vec<String>>,
    overlay: &HeadTraceOverlay,
    root_node_ref: &str,
    seen: &mut BTreeSet<String>,
) -> PactResult<Value> {
    let record = records
        .get(component_id)
        .ok_or_else(|| PactError::not_found(format!("missing node record: {component_id}")))?;
    let root_static_children = raw_child_ids(record).into_iter().collect::<BTreeSet<_>>();
    let ctx = HeadTreeBuildContext {
        records,
        children_by_parent,
        overlay,
        root_node_ref,
        root_static_children,
    };
    node_record_head_tree_value_from_payload(component_id, record, &ctx, seen, None)
}

fn node_record_head_tree_value_from_payload(
    component_id: &str,
    record: &Value,
    ctx: &HeadTreeBuildContext<'_>,
    seen: &mut BTreeSet<String>,
    parent_id: Option<&str>,
) -> PactResult<Value> {
    if ctx.overlay.deleted.contains(component_id) {
        return Err(PactError::not_found(format!(
            "deleted node referenced from head tree: {component_id}"
        )));
    }
    if !seen.insert(component_id.to_string()) {
        return Err(PactError::storage(format!(
            "node record tree contains a cycle or duplicate id at {component_id}"
        )));
    }
    let mut value = record.clone();
    apply_head_component_overlay(component_id, &mut value, ctx.overlay)?;
    let embedded_children = embedded_children_by_id(record);
    let child_ids = merged_head_child_ids(&value, ctx.children_by_parent, ctx.overlay);
    let object = value.as_object_mut().ok_or_else(|| {
        PactError::storage(format!(
            "node record did not decode as object: {component_id}"
        ))
    })?;
    object.remove("child_ids");
    object.remove("children");
    if component_id == ctx.root_node_ref {
        object.remove("parent_id");
    } else if parent_id == Some(ctx.root_node_ref)
        && ctx.root_static_children.contains(component_id)
    {
        object.insert("parent_id".to_string(), Value::Null);
    } else if let Some(parent_id) = parent_id {
        object
            .entry("parent_id".to_string())
            .or_insert_with(|| Value::String(parent_id.to_string()));
    }
    let children = child_ids
        .iter()
        .map(|child_id| {
            if ctx.overlay.deleted.contains(child_id) {
                return Err(PactError::not_found(format!(
                    "deleted node referenced from head tree: {child_id}"
                )));
            }
            if let Some(child_record) = ctx.overlay.components.get(child_id) {
                node_record_head_tree_value_from_payload(
                    child_id,
                    child_record,
                    ctx,
                    seen,
                    Some(component_id),
                )
            } else if let Some(child_record) = ctx.records.get(child_id) {
                node_record_head_tree_value_from_payload(
                    child_id,
                    child_record,
                    ctx,
                    seen,
                    Some(component_id),
                )
            } else if let Some(child_record) = embedded_children.get(child_id) {
                node_record_head_tree_value_from_payload(
                    child_id,
                    child_record,
                    ctx,
                    seen,
                    Some(component_id),
                )
            } else {
                Err(PactError::not_found(format!(
                    "missing node record: {child_id}"
                )))
            }
        })
        .collect::<PactResult<Vec<_>>>()?;
    if !children.is_empty() {
        object.insert("children".to_string(), Value::Array(children));
    }
    Ok(value)
}

struct HeadTreeBuildContext<'a> {
    records: &'a BTreeMap<String, Value>,
    children_by_parent: &'a BTreeMap<String, Vec<String>>,
    overlay: &'a HeadTraceOverlay,
    root_node_ref: &'a str,
    root_static_children: BTreeSet<String>,
}

#[derive(Default)]
struct HeadTraceOverlay {
    child_ids: BTreeMap<String, Vec<String>>,
    components: BTreeMap<String, Value>,
    deleted: BTreeSet<String>,
    ops: BTreeMap<String, Vec<HeadOverlayOp>>,
}

enum HeadOverlayOp {
    SetField {
        key: String,
        value: Value,
    },
    UnsetField {
        key: String,
    },
    SetAttr {
        key: String,
        value: Value,
    },
    UnsetAttr {
        key: String,
    },
    SetContent {
        content: Option<String>,
        content_ref: Option<Value>,
        content_preview: Option<String>,
    },
    ReplaceContentRange {
        start: usize,
        end: usize,
        replacement: String,
    },
    SetLifecycle {
        value: Value,
    },
}

fn head_trace_overlay(branch: &PactBranch, target_revision: u64) -> HeadTraceOverlay {
    let mut overlay = HeadTraceOverlay::default();
    let mut traces = branch
        .transaction_traces
        .iter()
        .filter(|trace| trace.to_rev <= target_revision)
        .collect::<Vec<_>>();
    traces.sort_by_key(|trace| (trace.to_rev, trace.from_rev));

    for trace in traces {
        for op in &trace.ops {
            let component_id = op.component_id.clone();
            match &op.kind {
                TraceOpKind::InsertComponent {
                    parent_id,
                    component,
                    ..
                } => {
                    overlay.deleted.remove(&component_id);
                    overlay.ops.remove(&component_id);
                    overlay.child_ids.remove(&component_id);
                    let mut component = component.clone();
                    if let Some(object) = component.as_object_mut() {
                        object
                            .entry("parent_id".to_string())
                            .or_insert_with(|| Value::String(parent_id.clone()));
                    }
                    overlay.components.insert(component_id, component);
                }
                TraceOpKind::DeleteComponent => {
                    overlay.deleted.insert(component_id.clone());
                    overlay.child_ids.remove(&component_id);
                    overlay.ops.remove(&component_id);
                    overlay.components.remove(&component_id);
                }
                TraceOpKind::MoveComponent { parent_id, .. } => {
                    overlay
                        .ops
                        .entry(component_id)
                        .or_default()
                        .push(HeadOverlayOp::SetField {
                            key: "parent_id".to_string(),
                            value: Value::String(parent_id.clone()),
                        });
                }
                TraceOpKind::SetField { key, value } => {
                    if key == "child_ids" {
                        overlay.child_ids.insert(
                            component_id.clone(),
                            value
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect(),
                        );
                    }
                    overlay
                        .ops
                        .entry(component_id)
                        .or_default()
                        .push(HeadOverlayOp::SetField {
                            key: key.clone(),
                            value: value.clone(),
                        });
                }
                TraceOpKind::UnsetField { key } => {
                    overlay
                        .ops
                        .entry(component_id)
                        .or_default()
                        .push(HeadOverlayOp::UnsetField { key: key.clone() });
                }
                TraceOpKind::SetAttr { key, value } => {
                    overlay
                        .ops
                        .entry(component_id)
                        .or_default()
                        .push(HeadOverlayOp::SetAttr {
                            key: key.clone(),
                            value: value.clone(),
                        });
                }
                TraceOpKind::UnsetAttr { key } => {
                    overlay
                        .ops
                        .entry(component_id)
                        .or_default()
                        .push(HeadOverlayOp::UnsetAttr { key: key.clone() });
                }
                TraceOpKind::SetContent {
                    content,
                    content_ref,
                    content_preview,
                } => {
                    overlay
                        .ops
                        .entry(component_id)
                        .or_default()
                        .push(HeadOverlayOp::SetContent {
                            content: content.clone(),
                            content_ref: content_ref.clone(),
                            content_preview: content_preview.clone(),
                        });
                }
                TraceOpKind::ReplaceContentRange {
                    start,
                    end,
                    replacement,
                } => {
                    overlay.ops.entry(component_id).or_default().push(
                        HeadOverlayOp::ReplaceContentRange {
                            start: *start,
                            end: *end,
                            replacement: replacement.clone(),
                        },
                    );
                }
                TraceOpKind::SetLifecycle { value } => {
                    overlay.ops.entry(component_id).or_default().push(
                        HeadOverlayOp::SetLifecycle {
                            value: value.clone(),
                        },
                    );
                }
            }
        }
    }
    overlay
}

fn apply_head_component_overlay(
    component_id: &str,
    value: &mut Value,
    overlay: &HeadTraceOverlay,
) -> PactResult<()> {
    let Some(ops) = overlay.ops.get(component_id) else {
        return Ok(());
    };
    let object = value.as_object_mut().ok_or_else(|| {
        PactError::storage(format!(
            "node record did not decode as object: {component_id}"
        ))
    })?;
    for op in ops {
        match op {
            HeadOverlayOp::SetField { key, value } => {
                object.insert(key.clone(), value.clone());
            }
            HeadOverlayOp::UnsetField { key } => {
                object.remove(key);
            }
            HeadOverlayOp::SetAttr { key, value } => {
                let attrs = object
                    .entry("aux".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(attrs) = attrs.as_object_mut() {
                    attrs.insert(key.clone(), value.clone());
                }
            }
            HeadOverlayOp::UnsetAttr { key } => {
                if let Some(attrs) = object.get_mut("aux").and_then(Value::as_object_mut) {
                    attrs.remove(key);
                }
            }
            HeadOverlayOp::SetContent {
                content,
                content_ref,
                content_preview,
            } => {
                match content {
                    Some(text) => {
                        object.insert("content".to_string(), Value::String(text.clone()));
                    }
                    None => {
                        object.remove("content");
                    }
                }
                match content_ref {
                    Some(content_ref) => {
                        object.insert("content_ref".to_string(), content_ref.clone());
                    }
                    None => {
                        object.remove("content_ref");
                    }
                }
                match content_preview {
                    Some(preview) => {
                        object.insert(
                            "content_preview".to_string(),
                            Value::String(preview.clone()),
                        );
                    }
                    None => {
                        object.remove("content_preview");
                    }
                }
            }
            HeadOverlayOp::ReplaceContentRange {
                start,
                end,
                replacement,
            } => {
                let current = object
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if *start <= *end
                    && *end <= current.len()
                    && current.is_char_boundary(*start)
                    && current.is_char_boundary(*end)
                {
                    let mut updated = String::with_capacity(current.len());
                    updated.push_str(&current[..*start]);
                    updated.push_str(replacement);
                    updated.push_str(&current[*end..]);
                    object.insert("content".to_string(), Value::String(updated));
                }
            }
            HeadOverlayOp::SetLifecycle { value } => {
                object.insert("lifecycle".to_string(), value.clone());
            }
        }
    }
    Ok(())
}

fn apply_paired_tool_deletes_from_head_overlay(
    node_records: &BTreeMap<String, Value>,
    overlay: &mut HeadTraceOverlay,
) {
    if overlay.deleted.is_empty() {
        return;
    }
    let mut tool_call_ids = BTreeMap::new();
    for record in node_records.values() {
        collect_tool_call_ids(record, &mut tool_call_ids);
    }
    for component in overlay.components.values() {
        collect_tool_call_ids(component, &mut tool_call_ids);
    }

    let deleted = overlay.deleted.iter().cloned().collect::<Vec<_>>();
    for component_id in deleted {
        let Some(tool_call_id) = tool_call_ids.get(&component_id).cloned() else {
            continue;
        };
        let paired_ids = tool_call_ids
            .iter()
            .filter(|(_, candidate_tool_call_id)| *candidate_tool_call_id == &tool_call_id)
            .map(|(component_id, _)| component_id.clone())
            .collect::<Vec<_>>();
        for paired_id in paired_ids {
            overlay.deleted.insert(paired_id.clone());
            overlay.child_ids.remove(&paired_id);
            overlay.ops.remove(&paired_id);
            overlay.components.remove(&paired_id);
        }
    }
}

fn collect_tool_call_ids(value: &Value, out: &mut BTreeMap<String, String>) {
    if let Some(object) = value.as_object() {
        if let (Some(component_id), Some(tool_call_id)) = (
            object.get("id").and_then(Value::as_str),
            object.get("tool_call_id").and_then(Value::as_str),
        ) {
            out.insert(component_id.to_string(), tool_call_id.to_string());
        }
        if let Some(children) = object.get("children").and_then(Value::as_array) {
            for child in children {
                collect_tool_call_ids(child, out);
            }
        }
    }
}

fn merged_head_child_ids(
    record: &Value,
    children_by_parent: &BTreeMap<String, Vec<String>>,
    overlay: &HeadTraceOverlay,
) -> Vec<String> {
    let Some(component_id) = record.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let component_type = record.get("componentType").and_then(Value::as_str);
    let should_merge_parent_index = matches!(component_type, Some("root" | "system_header"));
    let mut child_ids = if should_merge_parent_index {
        raw_child_ids(record)
    } else {
        overlay
            .child_ids
            .get(component_id)
            .cloned()
            .unwrap_or_else(|| raw_child_ids(record))
    };
    let mut seen = child_ids.iter().cloned().collect::<BTreeSet<_>>();
    if should_merge_parent_index {
        if let Some(overlay_child_ids) = overlay.child_ids.get(component_id) {
            for child_id in overlay_child_ids {
                if seen.insert(child_id.clone()) {
                    child_ids.push(child_id.clone());
                }
            }
        }
        if let Some(parent_indexed) = children_by_parent.get(component_id) {
            for child_id in parent_indexed {
                if seen.insert(child_id.clone()) {
                    child_ids.push(child_id.clone());
                }
            }
        }
    }
    child_ids
        .into_iter()
        .filter(|child_id| !overlay.deleted.contains(child_id))
        .collect()
}

fn raw_child_ids(record: &Value) -> Vec<String> {
    record
        .get("child_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn embedded_children_by_id(record: &Value) -> BTreeMap<String, Value> {
    record
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|child| {
            child
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), child.clone()))
        })
        .collect()
}

fn children_by_parent_from_node_records(
    records: &BTreeMap<String, Value>,
) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for record in records.values() {
        let Some(parent_id) = record.get("parent_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(component_id) = record.get("id").and_then(Value::as_str) else {
            continue;
        };
        if parent_id == component_id {
            continue;
        }
        grouped
            .entry(parent_id.to_string())
            .or_default()
            .push(record);
    }
    grouped
        .into_iter()
        .map(|(parent_id, mut children)| {
            children.sort_by(|left, right| {
                (
                    node_record_offset(left),
                    node_record_creation_index(left),
                    node_record_created_at_ns(left),
                    node_record_id(left),
                )
                    .cmp(&(
                        node_record_offset(right),
                        node_record_creation_index(right),
                        node_record_created_at_ns(right),
                        node_record_id(right),
                    ))
            });
            (
                parent_id,
                children
                    .into_iter()
                    .filter_map(|record| {
                        record.get("id").and_then(Value::as_str).map(str::to_string)
                    })
                    .collect(),
            )
        })
        .collect()
}

fn node_record_offset(record: &Value) -> i64 {
    record.get("offset").and_then(Value::as_i64).unwrap_or(0)
}

fn node_record_creation_index(record: &Value) -> u64 {
    record
        .get("creation_index")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn node_record_created_at_ns(record: &Value) -> u64 {
    record
        .get("created_at_ns")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn node_record_id(record: &Value) -> &str {
    record.get("id").and_then(Value::as_str).unwrap_or("")
}

fn log_head_reconstruction_fallback(branch_id: &str, reason: impl std::fmt::Display) {
    if std::env::var_os("PACTREE_LOG_HEAD_RECONSTRUCTION_FALLBACK").is_some() {
        eprintln!("pactree: skipped head reconstruction for branch {branch_id}: {reason}");
    }
}

fn contiguous_transaction_trace_revision(branch: &PactBranch) -> u64 {
    let mut traces = branch.transaction_traces.iter().collect::<Vec<_>>();
    traces.sort_by_key(|trace| (trace.to_rev, trace.from_rev));
    let mut covered = 0;
    for trace in traces {
        if trace.to_rev <= covered {
            continue;
        }
        if trace.from_rev != covered {
            break;
        }
        covered = trace.to_rev;
    }
    covered
}

fn first_commit_cycle_for_branch(records: &[ScannedRecord], branch_id: &str) -> Option<u64> {
    records
        .iter()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .filter(|commit| commit.branch_id == branch_id)
        .map(|commit| commit.cycle)
        .min()
}

fn commit_record_branch_ids(records: &[ScannedRecord]) -> Vec<String> {
    let mut branch_ids = records
        .iter()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .map(|commit| commit.branch_id)
        .collect::<Vec<_>>();
    branch_ids.sort();
    branch_ids.dedup();
    branch_ids
}

fn latest_commit_cycle_for_branch(records: &[ScannedRecord], branch_id: &str) -> Option<u64> {
    records
        .iter()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .filter(|commit| commit.branch_id == branch_id)
        .map(|commit| commit.cycle)
        .max()
}

fn ensure_commit_record_branch(
    root_store: &mut ContextTreeLedger,
    branch_id: &str,
) -> PactResult<()> {
    if root_store.branches.contains_key(branch_id) {
        return Ok(());
    }
    let source_branch_id = root_store.current_branch_id.clone();
    let source_branch = root_store
        .branches
        .get(&source_branch_id)
        .ok_or_else(|| PactError::not_found(format!("missing current branch: {source_branch_id}")))?
        .clone();
    let base_tree =
        crate::store::project_branch_component_tree(root_store, &source_branch, Some(0))?
            .project_value();
    let projection = crate::store::build_projection(&base_tree, &BTreeMap::new(), 1024)?;
    let fork_ref = PactRef {
        root_id: root_store.root_id.clone(),
        branch_id: source_branch_id.clone(),
        revision: Some(0),
        transaction_id: None,
        component_id: None,
        content_ref: None,
    };
    let base_ref = PactRef {
        root_id: root_store.root_id.clone(),
        branch_id: branch_id.to_string(),
        revision: Some(0),
        transaction_id: None,
        component_id: None,
        content_ref: None,
    };
    let branch = PactBranch {
        branch_id: branch_id.to_string(),
        root_id: root_store.root_id.clone(),
        parent_branch_id: Some(source_branch_id),
        fork_ref: Some(fork_ref),
        base_ref,
        head_revision: 0,
        lifecycle_tick: 0,
        transaction_traces: Vec::new(),
        current: crate::store::PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_id.to_string(),
            0,
            BTreeMap::new(),
            0,
        ),
        aux: BTreeMap::new(),
    };
    root_store.branches.insert(branch_id.to_string(), branch);
    Ok(())
}

fn branch_current_node_refs(payload: &Value) -> Vec<String> {
    payload
        .get("current")
        .and_then(|current| current.get("node_refs"))
        .and_then(Value::as_object)
        .map(|refs| {
            refs.values()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn hydrate_branch_current_from_node_records(
    branch: &mut PactBranch,
    node_refs: Option<&Vec<String>>,
    records: &BTreeMap<String, ComponentRecord>,
) -> PactResult<()> {
    if records.is_empty() {
        return Ok(());
    }
    let allowed = node_refs
        .filter(|refs| !refs.is_empty())
        .map(|refs| refs.iter().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_else(|| records.keys().cloned().collect::<BTreeSet<_>>());
    let root_id = if allowed.contains(&branch.current.tree.root_id) {
        branch.current.tree.root_id.clone()
    } else if allowed.contains(&branch.root_id) {
        branch.root_id.clone()
    } else {
        records
            .values()
            .find(|record| {
                record.parent_id.is_none() && (allowed.is_empty() || allowed.contains(&record.id))
            })
            .map(|record| record.id.clone())
            .unwrap_or_else(|| branch.current.tree.root_id.clone())
    };
    if !records.contains_key(&root_id) {
        return Ok(());
    }
    let tree = node_record_tree_value(&root_id, records, &allowed)?;
    let first_seen = branch.current.tree.first_seen.clone();
    let lifecycle_seen = branch.current.lifecycle_seen.clone();
    let branch_refs = branch.current.branch_refs.clone();
    let content_refs = branch.current.content_refs.clone();
    let projection = crate::store::build_projection(&tree, &first_seen, 1024)?;
    let mut view = crate::store::PactView::from_projection_with_lifecycle_seen(
        projection,
        branch.branch_id.clone(),
        branch.current.head_revision,
        lifecycle_seen,
        branch.lifecycle_tick,
    );
    view.branch_refs = branch_refs;
    if !content_refs.is_empty() {
        view.content_refs = content_refs;
    }
    branch.current = view;
    Ok(())
}

fn node_record_tree_value(
    component_id: &str,
    records: &BTreeMap<String, ComponentRecord>,
    allowed: &BTreeSet<String>,
) -> PactResult<Value> {
    let record = records
        .get(component_id)
        .ok_or_else(|| PactError::not_found(format!("missing node record: {component_id}")))?;
    let mut value = serde_json::to_value(record)
        .map_err(|err| PactError::codec(format!("failed to encode node {component_id}: {err}")))?;
    let object = value.as_object_mut().ok_or_else(|| {
        PactError::storage(format!(
            "node record did not encode as object: {component_id}"
        ))
    })?;
    object.remove("parent_id");
    object.remove("child_ids");
    let children = record
        .child_ids
        .iter()
        .filter(|child_id| allowed.contains(*child_id))
        .map(|child_id| node_record_tree_value(child_id, records, allowed))
        .collect::<PactResult<Vec<_>>>()?;
    if !children.is_empty() {
        object.insert("children".to_string(), Value::Array(children));
    }
    Ok(value)
}

fn rewrite_file_local_blob_refs(view: &mut crate::store::PactView, content_ids: &[String]) {
    let component_ids = view.component_ids().cloned().collect::<Vec<_>>();
    for component_id in component_ids {
        let Some(blob_ref) = view
            .component(&component_id)
            .and_then(|component| component.content_ref())
            .map(|content_ref| content_ref.id.clone())
        else {
            continue;
        };
        if let Some(index_text) = blob_ref.strip_prefix("blob_")
            && let Ok(index) = index_text.parse::<usize>()
            && let Some(content_id) = content_ids.get(index)
            && let Some(component) = view.component_mut(&component_id)
        {
            component.externalize_content(crate::components::PactContentRef {
                id: content_id.clone(),
                field: Some("$.content".to_string()),
                chars: None,
                preview: None,
                source: None,
            });
        }
    }
}

fn rewrite_synthetic_blob_refs_from_projected_tree(
    projection: &mut crate::store::PactProjection,
    projected: &Value,
    content_texts: &BTreeMap<String, String>,
) {
    let mut content_id_by_text = BTreeMap::new();
    for (content_id, text) in content_texts {
        content_id_by_text
            .entry(text.clone())
            .or_insert_with(|| content_id.clone());
    }

    let mut text_by_node_id = BTreeMap::new();
    collect_projected_node_texts(projected, &mut text_by_node_id);

    let component_ids = projection.component_ids().cloned().collect::<Vec<_>>();
    for component_id in component_ids {
        let Some(blob_ref) = projection
            .component(&component_id)
            .and_then(|component| component.content_ref())
            .map(|content_ref| content_ref.id.clone())
        else {
            continue;
        };
        if !blob_ref.starts_with("blob_") {
            continue;
        }
        let Some(text) = text_by_node_id.get(&component_id) else {
            continue;
        };
        let Some(content_id) = content_id_by_text.get(text) else {
            continue;
        };
        if let Some(component) = projection.tree.component_mut(&component_id) {
            component.externalize_content(crate::components::PactContentRef {
                id: content_id.clone(),
                field: Some("$.content".to_string()),
                chars: None,
                preview: None,
                source: None,
            });
        }
    }
}

fn collect_projected_node_texts(value: &Value, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(object) => {
            if let (Some(id), Some(content)) = (
                object.get("id").and_then(Value::as_str),
                object.get("content").and_then(Value::as_str),
            ) {
                out.insert(id.to_string(), content.to_string());
            }
            if let Some(children) = object.get("children").and_then(Value::as_array) {
                for child in children {
                    collect_projected_node_texts(child, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_projected_node_texts(item, out);
            }
        }
        _ => {}
    }
}

pub(super) fn apply_slice_spec(
    root_store: &mut ContextTreeLedger,
    spec: &PactFileSliceSpec,
) -> PactResult<()> {
    match spec.mode {
        PactFileSliceMode::FullRoot => {
            mark_slice_branches(root_store, spec)?;
            Ok(())
        }
        PactFileSliceMode::TransactionRange => apply_transaction_range_slice(root_store, spec),
        PactFileSliceMode::CurrentHeadOnly
        | PactFileSliceMode::BranchWithLineage
        | PactFileSliceMode::BranchCopyOnly => {
            let branch_id = spec
                .branch_id
                .as_deref()
                .unwrap_or(&root_store.current_branch_id)
                .to_string();
            if !root_store.branches.contains_key(&branch_id) {
                return Err(PactError::not_found(format!(
                    "slice branch not found: {branch_id}"
                )));
            }
            let mut keep = BTreeMap::new();
            let mut cursor = Some(branch_id.clone());
            while let Some(id) = cursor {
                let Some(branch) = root_store.branches.get(&id).cloned() else {
                    break;
                };
                cursor = if matches!(spec.mode, PactFileSliceMode::BranchWithLineage) {
                    branch.parent_branch_id.clone()
                } else {
                    None
                };
                keep.insert(id, branch);
            }
            root_store.branches = keep;
            root_store.current_branch_id = branch_id;
            mark_slice_branches(root_store, spec)?;
            Ok(())
        }
    }
}

fn apply_transaction_range_slice(
    root_store: &mut ContextTreeLedger,
    spec: &PactFileSliceSpec,
) -> PactResult<()> {
    let branch_id = spec
        .branch_id
        .as_deref()
        .unwrap_or(&root_store.current_branch_id)
        .to_string();
    let source_branch = root_store
        .branches
        .get(&branch_id)
        .cloned()
        .ok_or_else(|| PactError::not_found(format!("slice branch not found: {branch_id}")))?;
    let start_revision = slice_boundary_revision(
        root_store,
        &branch_id,
        spec.start_transaction_id.as_deref(),
        spec.start_cycle,
        source_branch.base_ref.revision.unwrap_or(0),
        "start",
    )?;
    let end_revision = slice_boundary_revision(
        root_store,
        &branch_id,
        spec.end_transaction_id.as_deref(),
        spec.end_cycle,
        source_branch.head_revision,
        "end",
    )?;
    if start_revision > end_revision || end_revision > source_branch.head_revision {
        return Err(PactError::invalid(format!(
            "invalid slice revision range {start_revision}..{end_revision} for branch head {}",
            source_branch.head_revision
        )));
    }

    let start_tree = crate::store::project_branch_component_tree(
        root_store,
        &source_branch,
        Some(start_revision),
    )?;
    let end_tree = crate::store::project_branch_component_tree(
        root_store,
        &source_branch,
        Some(end_revision),
    )?;
    let mut start_value = start_tree.project_value();
    let mut end_value = end_tree.project_value();
    let inline_limit = crate::components::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES;
    crate::store::externalize_large_content_in_value(
        &mut start_value,
        &mut root_store.content,
        inline_limit,
    );
    crate::store::externalize_large_content_in_value(
        &mut end_value,
        &mut root_store.content,
        inline_limit,
    );
    let base_json = serde_json::to_string(&start_value)
        .map_err(|error| PactError::codec(format!("encode slice base: {error}")))?;
    let base_record =
        root_store
            .content
            .put_inline("pact.branch.slice_base", &base_json, inline_limit);
    let end_projection = crate::store::build_projection(
        &end_value,
        &source_branch.current.tree.first_seen,
        inline_limit,
    )?;
    let mut current = crate::store::PactView::from_projection_with_lifecycle_seen(
        end_projection,
        branch_id.clone(),
        end_revision,
        source_branch.current.lifecycle_seen.clone(),
        source_branch.lifecycle_tick,
    );
    current.content_refs = source_branch.current.content_refs.clone();

    let mut branch = source_branch;
    branch.parent_branch_id = None;
    branch.fork_ref = None;
    branch.base_ref = crate::store::PactRef {
        root_id: root_store.root_id.clone(),
        branch_id: branch_id.clone(),
        revision: Some(start_revision),
        transaction_id: transaction_id_at_revision(root_store, &branch_id, start_revision),
        component_id: None,
        content_ref: Some(base_record.id),
    };
    branch.head_revision = end_revision;
    branch.current = current;
    branch
        .transaction_traces
        .retain(|trace| trace.from_rev >= start_revision && trace.to_rev <= end_revision);
    branch.aux.insert(
        "slice_source_range".to_string(),
        serde_json::json!({
            "branch_id": branch_id,
            "start_revision": start_revision,
            "end_revision": end_revision,
        }),
    );

    let retained_transactions = root_store
        .event_index
        .transaction_traces
        .iter()
        .filter_map(|(transaction_id, trace)| {
            let revision = public_trace_revision(trace)?;
            (trace.branch_id == branch_id && revision > start_revision && revision <= end_revision)
                .then_some(transaction_id.clone())
        })
        .collect::<BTreeSet<_>>();
    root_store
        .event_index
        .transaction_traces
        .retain(|transaction_id, _| retained_transactions.contains(transaction_id));
    root_store
        .event_index
        .by_transaction
        .retain(|transaction_id, _| retained_transactions.contains(transaction_id));
    root_store.branches.clear();
    root_store.branches.insert(branch_id.clone(), branch);
    root_store.current_branch_id = branch_id;
    root_store.parent = None;
    root_store.base = None;
    mark_slice_branches(root_store, spec)
}

fn slice_boundary_revision(
    root_store: &ContextTreeLedger,
    branch_id: &str,
    transaction_id: Option<&str>,
    cycle: Option<u64>,
    default_revision: u64,
    label: &str,
) -> PactResult<u64> {
    if transaction_id.is_some() && cycle.is_some() {
        return Err(PactError::invalid(format!(
            "slice {label} must use a transaction id or cycle, not both"
        )));
    }
    if let Some(transaction_id) = transaction_id {
        let trace = root_store
            .event_index
            .transaction_traces
            .get(transaction_id)
            .filter(|trace| trace.branch_id == branch_id)
            .ok_or_else(|| {
                PactError::not_found(format!(
                    "slice {label} transaction not found on branch {branch_id}: {transaction_id}"
                ))
            })?;
        return public_trace_revision(trace).ok_or_else(|| {
            PactError::corrupt(format!(
                "slice {label} transaction has no revision: {transaction_id}"
            ))
        });
    }
    Ok(cycle.unwrap_or(default_revision))
}

fn public_trace_revision(trace: &crate::store::PactTransactionTrace) -> Option<u64> {
    trace
        .post_response_ref
        .as_ref()
        .or(trace.post_response_alteration_ref.as_ref())
        .or(trace.response_ref.as_ref())
        .or(trace.request_ref.as_ref())
        .unwrap_or(&trace.post_request_alteration_ref)
        .revision
}

fn transaction_id_at_revision(
    root_store: &ContextTreeLedger,
    branch_id: &str,
    revision: u64,
) -> Option<String> {
    root_store
        .event_index
        .transaction_traces
        .values()
        .find(|trace| {
            trace.branch_id == branch_id && public_trace_revision(trace) == Some(revision)
        })
        .map(|trace| trace.transaction_id.clone())
}

pub(super) fn mark_slice_branches(
    root_store: &mut ContextTreeLedger,
    spec: &PactFileSliceSpec,
) -> PactResult<()> {
    let spec_value = serde_json::to_value(spec)
        .map_err(|err| PactError::codec(format!("failed to encode slice spec: {err}")))?;
    for branch in root_store.branches.values_mut() {
        branch.aux.insert(
            "pact_file_kind".to_string(),
            Value::String("slice".to_string()),
        );
        branch
            .aux
            .insert("slice_spec".to_string(), spec_value.clone());
    }
    Ok(())
}

pub fn file_append_commit_json(
    path: &Path,
    commit_json: &str,
    node_records_json: &str,
) -> PactResult<String> {
    let commit: CommitRecord = serde_json::from_str(commit_json)
        .map_err(|err| PactError::invalid(format!("invalid commit record JSON: {err}")))?;
    let nodes: Vec<Value> = serde_json::from_str(node_records_json)
        .map_err(|err| PactError::invalid(format!("invalid node records JSON: {err}")))?;
    let manifest = file_append_commit_record(path, &commit, &nodes)?;
    serde_json::to_string_pretty(&manifest)
        .map_err(|err| PactError::codec(format!("failed to encode .pact manifest: {err}")))
}

pub fn file_append_commit_with_content_json(
    path: &Path,
    commit_json: &str,
    node_records_json: &str,
    content_store_json: &str,
) -> PactResult<String> {
    let commit: CommitRecord = serde_json::from_str(commit_json)
        .map_err(|err| PactError::invalid(format!("invalid commit record JSON: {err}")))?;
    let nodes: Vec<Value> = serde_json::from_str(node_records_json)
        .map_err(|err| PactError::invalid(format!("invalid node records JSON: {err}")))?;
    let content_store: ContentStore = serde_json::from_str(content_store_json)
        .map_err(|err| PactError::invalid(format!("invalid content store JSON: {err}")))?;
    let manifest = file_append_commit_record_with_content(path, &commit, &nodes, &content_store)?;
    serde_json::to_string_pretty(&manifest)
        .map_err(|err| PactError::codec(format!("failed to encode .pact manifest: {err}")))
}

pub fn file_commit_context_tree_json(
    path: &Path,
    root_store_json: &str,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let root_store = crate::store::decode_context_tree_ledger(root_store_json)?;
    let manifest = file_commit_context_tree(path, root_store, branch_id, blob_inline_limit)?;
    serde_json::to_string_pretty(&manifest)
        .map_err(|err| PactError::codec(format!("failed to encode .pact manifest: {err}")))
}

pub fn file_commit_context_tree(
    path: &Path,
    mut root_store: ContextTreeLedger,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<PactFileManifest> {
    root_store.content.rebuild_hash_index();
    let branch_id = branch_id
        .unwrap_or(&root_store.current_branch_id)
        .to_string();
    let branch = root_store
        .branches
        .get(&branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?
        .clone();
    let mut target_tree =
        crate::store::project_branch_component_tree(&root_store, &branch, None)?.project_value();
    externalize_large_commit_content_refs(
        &mut target_tree,
        &mut root_store.content,
        blob_inline_limit,
    );

    if !path.exists() {
        let root_id = value_node_id(&target_tree).unwrap_or_else(|| root_store.root_id.clone());
        let seed_tree = json!({
            "id": root_id,
            "componentType": "root",
            "children": []
        });
        let seed_json = serde_json::to_string(&seed_tree)
            .map_err(|err| PactError::codec(format!("failed to encode seed Pact tree: {err}")))?;
        let seed_root_store_json =
            crate::store::new_ledger_json(&seed_json, Some(&branch_id), blob_inline_limit)?;
        file_create_from_ledger_json(path, &seed_root_store_json, blob_inline_limit)?;
    }

    let current_bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let current_scan = scan_file(&current_bytes, true);
    if current_scan.manifest.last_valid_offset <= FILE_HEADER_LEN && !current_scan.errors.is_empty()
    {
        return Err(PactError::corrupt(format!(
            "cannot commit root store to invalid .pact file: {}",
            current_scan.errors.join("; ")
        )));
    }
    let current_root_store = ledger_from_records(&current_scan.records)?;
    let current_branch = current_root_store.branches.get(&branch_id).ok_or_else(|| {
        PactError::not_found(format!("branch not found in .pact file: {branch_id}"))
    })?;
    let mut previous_tree =
        crate::store::project_branch_component_tree(&current_root_store, current_branch, None)?
            .project_value();
    let mut throwaway_content = current_root_store.content.clone();
    externalize_large_commit_content_refs(
        &mut previous_tree,
        &mut throwaway_content,
        blob_inline_limit,
    );

    let mut content_store = root_store.content.clone();
    content_store
        .records
        .retain(|_, record| record.kind != "pact.branch.base");
    content_store.rebuild_hash_index();

    let mut ops = Vec::new();
    let mut nodes = BTreeMap::new();

    diff_value_trees(
        &previous_tree,
        &target_tree,
        &mut ops,
        &mut nodes,
        &mut content_store,
    )?;

    let has_content_records = !content_store.records.is_empty();
    if ops.is_empty() && !has_content_records {
        return Ok(current_scan.manifest);
    }

    let next_revision = current_branch.head_revision + 1;
    let commit = CommitRecord {
        schema: "pact.commit_record.v1".to_string(),
        branch_id,
        cycle: next_revision,
        revision: next_revision,
        lifecycle_tick: None,
        ops,
    };
    let node_records = nodes.into_values().collect::<Vec<_>>();
    if has_content_records {
        file_append_commit_record_with_content(path, &commit, &node_records, &content_store)
    } else {
        file_append_commit_record(path, &commit, &node_records)
    }
}

pub fn file_append_commit_record(
    path: &Path,
    commit: &CommitRecord,
    node_records: &[Value],
) -> PactResult<PactFileManifest> {
    file_append_commit_record_inner(path, commit, node_records, None)
}

pub fn file_append_commit_record_with_content(
    path: &Path,
    commit: &CommitRecord,
    node_records: &[Value],
    content_store: &ContentStore,
) -> PactResult<PactFileManifest> {
    file_append_commit_record_inner(path, commit, node_records, Some(content_store))
}

pub(super) fn file_append_commit_record_inner(
    path: &Path,
    commit: &CommitRecord,
    nodes: &[Value],
    content_store: Option<&ContentStore>,
) -> PactResult<PactFileManifest> {
    if path_is_v2(path)? {
        return append_v2_commit(path, commit, nodes, content_store);
    }
    let mut scrubbed_nodes_storage: Vec<Value>;
    let nodes: &[Value] = if nodes
        .iter()
        .any(|node| node.get("content_capture").is_some())
    {
        scrubbed_nodes_storage = nodes.to_vec();
        for node in &mut scrubbed_nodes_storage {
            apply_capture_policy_to_node_value(node);
        }
        scrubbed_nodes_storage.as_slice()
    } else {
        nodes
    };

    if content_store.is_none()
        && let Some(manifest) = try_file_append_commit_record_fast(path, commit, nodes)?
    {
        return Ok(manifest);
    }

    let mut bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    if scanned.manifest.last_valid_offset <= FILE_HEADER_LEN && !scanned.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot append to invalid .pact file: {}",
            scanned.errors.join("; ")
        )));
    }

    let mut next_seq = scanned.manifest.record_count as u64 + 1;
    let appended_node_sequences = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            node.get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), next_seq + index as u64))
        })
        .collect::<BTreeMap<_, _>>();

    let mut next_page_seq = scanned
        .manifest
        .pages
        .iter()
        .map(|page| page.page_seq)
        .max()
        .unwrap_or(0)
        + 1;

    let mut content_records = Vec::new();
    if let Some(content_store) = content_store {
        let existing_content_ids = scanned
            .records
            .iter()
            .filter(|record| record.kind == RecordKind::Content.as_str())
            .filter_map(|record| {
                record
                    .payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<BTreeSet<_>>();
        let mut content = ContentCollector::with_compression();
        content.seed_chunk_index_from_records(&scanned.records);
        for record in content_store.records.values() {
            if !existing_content_ids.contains(&record.id) {
                content.put_content_record(record)?;
            }
        }
        for payload in content.by_hash.into_values() {
            content_records.push(EncodedRecord {
                kind: RecordKind::Content,
                seq: next_seq,
                payload: encode_record_payload(RecordKind::Content, &payload)?,
            });
            next_seq += 1;
        }
        for chunk in content.chunks {
            content_records.push(EncodedRecord {
                kind: RecordKind::ContentChunk,
                seq: next_seq,
                payload: encode_record_payload(RecordKind::ContentChunk, &chunk)?,
            });
            next_seq += 1;
        }
    }

    let mut records = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        records.push(EncodedRecord {
            kind: RecordKind::Node,
            seq: next_seq + index as u64,
            payload: encode_record_payload(RecordKind::Node, node)?,
        });
    }
    let commit_seq = next_seq + records.len() as u64;
    records.push(EncodedRecord {
        kind: RecordKind::Commit,
        seq: commit_seq,
        payload: encode_record_payload(RecordKind::Commit, &commit)?,
    });

    let append_offset = bytes.len() as u64;
    if !content_records.is_empty() {
        let refs = content_records.iter().collect::<Vec<_>>();
        write_page(&mut bytes, PageKind::Content, next_page_seq, &refs)?;
        next_page_seq += 1;
    }
    let record_page_offset = bytes.len() as u64;
    let head_record = build_head_record_for_commit(
        &scanned,
        commit,
        record_page_offset,
        nodes,
        &appended_node_sequences,
        content_store,
    )?;
    records.push(EncodedRecord {
        kind: RecordKind::Head,
        seq: commit_seq + 1,
        payload: encode_record_payload(RecordKind::Head, &head_record)?,
    });
    records.push(EncodedRecord {
        kind: RecordKind::Meta,
        seq: commit_seq + 2,
        payload: encode_record_payload(
            RecordKind::Meta,
            &incremental_manifest_payload(
                &scanned.manifest,
                content_records
                    .iter()
                    .filter(|record| record.kind == RecordKind::Content)
                    .count(),
                content_records
                    .iter()
                    .filter(|record| record.kind == RecordKind::ContentChunk)
                    .count(),
                records
                    .iter()
                    .filter(|record| record.kind == RecordKind::Node)
                    .count(),
            ),
        )?,
    });
    let refs = records.iter().collect::<Vec<_>>();
    write_page(&mut bytes, PageKind::Record, next_page_seq, &refs)?;
    let footer = scanned.manifest.latest_footer.unwrap_or_default();
    write_footer(
        &mut bytes,
        record_page_offset,
        record_page_offset,
        footer.hot_index_offset,
        record_page_offset,
    );

    let appended = &bytes[append_offset as usize..];
    append_bytes(path, appended)?;

    let refreshed = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to reread .pact file: {err}")))?;
    Ok(scan_file(&refreshed, true).manifest)
}

fn path_is_v2(path: &Path) -> PactResult<bool> {
    let mut file = fs::File::open(path)
        .map_err(|error| PactError::storage(format!("failed to open .pact file: {error}")))?;
    let mut magic = [0_u8; 8];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(crate::pact_file::is_v2_file(&magic)),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(PactError::Io(error)),
    }
}

fn append_v2_commit(
    path: &Path,
    commit: &CommitRecord,
    nodes: &[Value],
    content_store: Option<&ContentStore>,
) -> PactResult<PactFileManifest> {
    use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};

    let mut resolver = crate::resolver::PactResolver::open_file(path.to_path_buf())?;
    if !resolver.ledger().branches.contains_key(&commit.branch_id) {
        return Err(PactError::not_found(format!(
            "branch not found: {}",
            commit.branch_id
        )));
    }
    if let Some(content_store) = content_store {
        resolver.merge_content_store(content_store);
    }
    let components = nodes
        .iter()
        .filter_map(|node| {
            node.get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), node))
        })
        .collect::<BTreeMap<_, _>>();
    let envelope = || PactOperationEnvelope {
        branch_id: commit.branch_id.clone(),
        source: SourceActor::Host,
        source_id: Some("file_append_commit_record".to_string()),
        host_timestamp_ns: None,
        idempotency_key: None,
        aux: BTreeMap::new(),
    };
    let component = |node_ref: &str| -> PactResult<crate::components::Component> {
        let node = components.get(node_ref).ok_or_else(|| {
            PactError::invalid(format!("commit references missing node {node_ref}"))
        })?;
        crate::components::Component::from_component_value(node)
            .map_err(|error| PactError::invalid(error.to_string()))
    };
    let mut operations = Vec::new();
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent {
                parent_id,
                node_ref,
                position,
            } => operations.push(PactOperation::Insert {
                envelope: envelope(),
                target_selector: None,
                parent_id: Some(parent_id.clone()),
                depth: None,
                offset: position.map(|value| value.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                component: component(node_ref)?,
            }),
            CommitRecordOp::AppendComponents {
                parent_id,
                node_refs,
            } => {
                for node_ref in node_refs {
                    operations.push(PactOperation::Append {
                        envelope: envelope(),
                        target_component_id: Some(parent_id.clone()),
                        target_selector: None,
                        offset: None,
                        component: component(node_ref)?,
                    });
                }
            }
            CommitRecordOp::ReplaceComponent {
                component_id,
                node_ref,
            } => operations.push(PactOperation::Replace {
                envelope: envelope(),
                target_component_id: Some(component_id.clone()),
                target_selector: None,
                component: component(node_ref)?,
            }),
            CommitRecordOp::DeleteComponent { component_id } => {
                operations.push(PactOperation::Delete {
                    envelope: envelope(),
                    target_component_id: Some(component_id.clone()),
                    target_selector: None,
                });
            }
            CommitRecordOp::SetComponentAux {
                component_id,
                key_symbol,
                value_ref,
            } => operations.push(PactOperation::Update {
                envelope: envelope(),
                target_component_id: Some(component_id.clone()),
                target_selector: None,
                content: None,
                content_ref: None,
                fields: BTreeMap::new(),
                aux: BTreeMap::from([(
                    key_symbol.clone(),
                    serde_json::from_str(value_ref)
                        .unwrap_or_else(|_| Value::String(value_ref.clone())),
                )]),
            }),
            CommitRecordOp::AttachContentRef {
                component_id,
                blob_id,
            } => operations.push(PactOperation::Update {
                envelope: envelope(),
                target_component_id: Some(component_id.clone()),
                target_selector: None,
                content: None,
                content_ref: Some(blob_id.clone()),
                fields: BTreeMap::new(),
                aux: BTreeMap::new(),
            }),
            CommitRecordOp::UnsetComponentAux { .. }
            | CommitRecordOp::ReplaceContentRange { .. } => {
                return Err(PactError::unsupported(
                    "this compatibility commit operation has no v0.1 mutation mapping",
                ));
            }
        }
    }
    let outcome = resolver.resolve(
        operations,
        &commit.branch_id,
        crate::trace::BoundaryKind::Adhoc,
    )?;
    if !outcome.rejected.is_empty() {
        return Err(PactError::resolver(
            outcome
                .rejected
                .iter()
                .map(|rejection| rejection.reason.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    resolver.seal_staged(&commit.branch_id, crate::trace::BoundaryKind::Adhoc)?;
    Ok(super::validate_file(path)?.manifest)
}

struct FastAppendContext {
    manifest: PactFileManifest,
    manifest_meta: Value,
    manifest_counts_known: bool,
    footer: PactFileFooter,
    file_len: u64,
    next_page_seq: u64,
    latest_head: Option<HeadRecord>,
}

fn try_file_append_commit_record_fast(
    path: &Path,
    commit: &CommitRecord,
    nodes: &[Value],
) -> PactResult<Option<PactFileManifest>> {
    let mut file = fs::File::open(path)
        .map_err(|err| PactError::storage(format!("failed to open .pact file: {err}")))?;
    let file_len = file
        .metadata()
        .map_err(|err| PactError::storage(format!("failed to stat .pact file: {err}")))?
        .len();
    let mut bytes_read = 0_u64;
    let Some(footer) = read_footer_from_end(&mut file, file_len, &mut bytes_read)? else {
        return Ok(None);
    };
    let symbols = read_symbol_table_prefix_from_file(&mut file, &mut bytes_read)?;
    let summary = scan_file_page_summary_from_path(path)?;
    if summary.manifest.last_valid_offset <= FILE_HEADER_LEN && !summary.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot append to invalid .pact file: {}",
            summary.errors.join("; ")
        )));
    }

    let manifest_records =
        read_page_records_from_file(&mut file, footer.manifest_offset, &mut bytes_read, &symbols)?;
    let mut manifest = summary.manifest;
    let manifest_meta = manifest_records
        .iter()
        .rev()
        .find(|record| record.kind == RecordKind::Meta.as_str())
        .map(|record| record.payload.clone())
        .unwrap_or_else(|| json!({ "schema": PACT_FILE_SCHEMA, "version": FILE_VERSION }));
    let manifest_counts_known = manifest_meta.get("record_count").is_some()
        && manifest_meta.get("node_count").is_some()
        && manifest_meta.get("commit_count").is_some();
    if manifest_counts_known {
        apply_manifest_counts_from_meta(&mut manifest, &manifest_meta);
    }
    let branch_records = read_page_records_matching_from_file(
        &mut file,
        footer.branch_head_offset,
        &mut bytes_read,
        &symbols,
        &[RecordKind::Head],
    )?;
    let latest_head = branch_records
        .iter()
        .rev()
        .find_map(|record| serde_json::from_value::<HeadRecord>(record.payload.clone()).ok());
    let context = FastAppendContext {
        next_page_seq: manifest
            .pages
            .iter()
            .map(|page| page.page_seq)
            .max()
            .unwrap_or(0)
            + 1,
        manifest,
        manifest_meta,
        manifest_counts_known,
        footer,
        file_len,
        latest_head,
    };
    append_commit_record_fast(path, context, commit, nodes).map(Some)
}

fn append_commit_record_fast(
    path: &Path,
    context: FastAppendContext,
    commit: &CommitRecord,
    nodes: &[Value],
) -> PactResult<PactFileManifest> {
    let mut next_seq = context.manifest.record_count as u64 + 1;
    let appended_node_sequences = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            node.get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), next_seq + index as u64))
        })
        .collect::<BTreeMap<_, _>>();
    let mut records = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        records.push(EncodedRecord {
            kind: RecordKind::Node,
            seq: next_seq + index as u64,
            payload: encode_record_payload(RecordKind::Node, node)?,
        });
    }
    next_seq += records.len() as u64;
    let commit_seq = next_seq;
    records.push(EncodedRecord {
        kind: RecordKind::Commit,
        seq: commit_seq,
        payload: encode_record_payload(RecordKind::Commit, &commit)?,
    });

    let record_page_offset = context.file_len;
    let head_record = head_record_from_commit(
        commit,
        record_page_offset,
        context
            .latest_head
            .as_ref()
            .and_then(|head| head.root_node_ref.clone())
            .or_else(|| {
                context
                    .manifest_meta
                    .get("root_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        nodes,
        appended_node_sequences,
    );
    records.push(EncodedRecord {
        kind: RecordKind::Head,
        seq: commit_seq + 1,
        payload: encode_record_payload(RecordKind::Head, &head_record)?,
    });
    records.push(EncodedRecord {
        kind: RecordKind::Meta,
        seq: commit_seq + 2,
        payload: encode_record_payload(
            RecordKind::Meta,
            &fast_append_manifest_payload(&context, nodes.len()),
        )?,
    });

    let mut appended = Vec::new();
    let refs = records.iter().collect::<Vec<_>>();
    write_page(
        &mut appended,
        PageKind::Record,
        context.next_page_seq,
        &refs,
    )?;
    let mut next_page_seq = context.next_page_seq + 1;
    let hot_index_offset = append_fast_hot_query_index_update(
        path,
        &context,
        commit,
        nodes,
        &records,
        record_page_offset,
        &mut next_page_seq,
        &mut appended,
    )?
    .unwrap_or(context.footer.hot_index_offset);
    let hot_index_is_current =
        hot_index_offset != 0 && hot_index_offset != context.footer.hot_index_offset;
    write_footer(
        &mut appended,
        record_page_offset,
        record_page_offset,
        hot_index_offset,
        if hot_index_is_current {
            0
        } else {
            record_page_offset
        },
    );
    append_bytes(path, &appended)?;

    let mut output_manifest = scan_file_page_summary_from_path(path)?.manifest;
    output_manifest.file_kind = context.manifest.file_kind;
    output_manifest.content_record_count = context.manifest.content_record_count;
    output_manifest.content_chunk_count = context.manifest.content_chunk_count;
    output_manifest.content_manifest_count = context.manifest.content_manifest_count;
    output_manifest.root_count = context.manifest.root_count;
    output_manifest.branch_count = context.manifest.branch_count;
    output_manifest.node_count = context.manifest.node_count + nodes.len();
    output_manifest.trace_count = context.manifest.trace_count;
    output_manifest.component_count = context.manifest.component_count;
    output_manifest.component_transaction_count = context.manifest.component_transaction_count;
    output_manifest.component_payload_count = context.manifest.component_payload_count;
    output_manifest.view_count = context.manifest.view_count;
    output_manifest.commit_count = context.manifest.commit_count + 1;
    output_manifest.frozen_cycle_count = context.manifest.frozen_cycle_count;
    output_manifest.head_count = context.manifest.head_count + 1;
    Ok(output_manifest)
}

#[allow(clippy::too_many_arguments)]
fn append_fast_hot_query_index_update(
    path: &Path,
    context: &FastAppendContext,
    commit: &CommitRecord,
    nodes: &[Value],
    records: &[EncodedRecord],
    record_page_offset: u64,
    next_page_seq: &mut u64,
    appended: &mut Vec<u8>,
) -> PactResult<Option<u64>> {
    if context.footer.hot_index_offset == 0 || context.footer.last_commit_offset != 0 {
        return Ok(None);
    }
    if !commit_ops_supported_by_hot_index_update(commit) {
        return Ok(None);
    }
    if !nodes.iter().any(stream_node_is_message_turn) {
        return Ok(None);
    }

    let mut file = fs::File::open(path)
        .map_err(|err| PactError::storage(format!("failed to open .pact file: {err}")))?;
    let mut bytes_read = 0_u64;
    let symbols = read_symbol_table_prefix_from_file(&mut file, &mut bytes_read)?;
    let Some(directory) = read_index_record_named_from_file(
        &mut file,
        context.footer.hot_index_offset,
        &mut bytes_read,
        &symbols,
        HOT_QUERY_INDEX_NAME,
    )?
    else {
        return Ok(None);
    };
    let Some(mut directory_entries) = directory
        .payload
        .get("entries")
        .and_then(|entries| entries.get("indexes"))
        .and_then(Value::as_object)
        .cloned()
    else {
        return Ok(None);
    };

    let mut hot_entries = BTreeMap::<(String, Option<String>), BTreeMap<String, Value>>::new();
    let mut next_seq = records.iter().map(|record| record.seq).max().unwrap_or(0) + 1;
    let node_positions = hot_index_node_positions_from_commit(commit);
    for (index, node) in nodes.iter().enumerate() {
        let Some(component_id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(encoded) = records.get(index) else {
            continue;
        };
        if encoded.kind != RecordKind::Node {
            continue;
        }
        let address = PactRecordAddress {
            page_offset: record_page_offset,
            page_seq: context.next_page_seq,
            record_seq: encoded.seq,
            record_kind: RecordKind::Node as u16,
            payload_len: Some(encoded.payload.len() as u32),
        };

        merge_hot_shard_entry(
            &mut file,
            &symbols,
            &mut hot_entries,
            &mut directory_entries,
            "node_by_id",
            &hot_query_shard_key(component_id),
            component_id,
            json!(address.clone()),
            record_page_offset,
            appended,
            next_page_seq,
            &mut next_seq,
        )?;
        merge_hot_shard_entry(
            &mut file,
            &symbols,
            &mut hot_entries,
            &mut directory_entries,
            "node_birth",
            &hot_query_shard_key(component_id),
            component_id,
            json!({
                "component_id": component_id,
                "first_seen": commit.revision,
                "current_surface": true,
                "branch_id": commit.branch_id,
                "address": address.clone(),
            }),
            record_page_offset,
            appended,
            next_page_seq,
            &mut next_seq,
        )?;

        if let Some(parent_id) = node.get("parent_id").and_then(Value::as_str).or_else(|| {
            node_positions
                .get(component_id)
                .map(|(parent_id, _)| parent_id.as_str())
        }) {
            merge_hot_bucket_array_member(
                &mut file,
                &symbols,
                &mut hot_entries,
                &mut directory_entries,
                "parent",
                parent_id,
                component_id,
                node_positions
                    .get(component_id)
                    .and_then(|(_, position)| *position),
                record_page_offset,
                appended,
                next_page_seq,
                &mut next_seq,
            )?;
        }

        if stream_node_is_message_turn(node) {
            merge_hot_bucket_array_member(
                &mut file,
                &symbols,
                &mut hot_entries,
                &mut directory_entries,
                "message_order",
                &commit.branch_id,
                component_id,
                node_positions
                    .get(component_id)
                    .and_then(|(_, position)| *position),
                record_page_offset,
                appended,
                next_page_seq,
                &mut next_seq,
            )?;
        }
        if let Some(kind) = hot_node_kind(node) {
            merge_hot_bucket_array_member(
                &mut file,
                &symbols,
                &mut hot_entries,
                &mut directory_entries,
                "type",
                kind,
                component_id,
                None,
                record_page_offset,
                appended,
                next_page_seq,
                &mut next_seq,
            )?;
            let mut head = serde_json::Map::new();
            head.insert(
                "node_ref".to_string(),
                Value::String(component_id.to_string()),
            );
            head.insert("type".to_string(), Value::String(kind.to_string()));
            head.insert("depth".to_string(), Value::Null);
            head.insert(
                "parent_ref".to_string(),
                node.get("parent_id").cloned().unwrap_or(Value::Null),
            );
            merge_hot_bucket_entry(
                &mut file,
                &symbols,
                &mut hot_entries,
                &mut directory_entries,
                "head_id",
                None,
                component_id,
                Value::Object(head),
                record_page_offset,
                appended,
                next_page_seq,
                &mut next_seq,
            )?;
        }
        for depth_key in hot_node_depth_keys(node) {
            merge_hot_bucket_array_member(
                &mut file,
                &symbols,
                &mut hot_entries,
                &mut directory_entries,
                "depth",
                &depth_key,
                component_id,
                None,
                record_page_offset,
                appended,
                next_page_seq,
                &mut next_seq,
            )?;
        }
    }

    let directory_record = encode_index_record(
        HOT_QUERY_INDEX_NAME,
        BTreeMap::from([("indexes".to_string(), Value::Object(directory_entries))]),
        next_seq,
        &symbols,
    )
    .ok_or_else(|| PactError::codec("failed to encode updated hot query index directory"))?;
    let directory_offset = record_page_offset + appended.len() as u64;
    write_page(
        appended,
        PageKind::Index,
        *next_page_seq,
        &[&directory_record],
    )?;
    *next_page_seq += 1;
    Ok(Some(directory_offset))
}

fn commit_ops_supported_by_hot_index_update(commit: &CommitRecord) -> bool {
    commit.ops.iter().all(|op| {
        matches!(
            op,
            CommitRecordOp::InsertComponent { .. } | CommitRecordOp::AppendComponents { .. }
        )
    })
}

fn hot_index_node_positions_from_commit(
    commit: &CommitRecord,
) -> BTreeMap<String, (String, Option<i64>)> {
    let mut positions = BTreeMap::new();
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent {
                parent_id,
                node_ref,
                position,
            } => {
                positions.insert(node_ref.clone(), (parent_id.clone(), *position));
            }
            CommitRecordOp::AppendComponents {
                parent_id,
                node_refs,
            } => {
                for node_ref in node_refs {
                    positions.insert(node_ref.clone(), (parent_id.clone(), None));
                }
            }
            _ => {}
        }
    }
    positions
}

#[allow(clippy::too_many_arguments)]
fn merge_hot_shard_entry(
    file: &mut fs::File,
    symbols: &SymbolTable,
    cache: &mut BTreeMap<(String, Option<String>), BTreeMap<String, Value>>,
    directory_entries: &mut serde_json::Map<String, Value>,
    bucket: &str,
    shard: &str,
    key: &str,
    value: Value,
    base_offset: u64,
    appended: &mut Vec<u8>,
    next_page_seq: &mut u64,
    next_seq: &mut u64,
) -> PactResult<()> {
    merge_hot_bucket_entry(
        file,
        symbols,
        cache,
        directory_entries,
        bucket,
        Some(shard),
        key,
        value,
        base_offset,
        appended,
        next_page_seq,
        next_seq,
    )
}

#[allow(clippy::too_many_arguments)]
fn merge_hot_bucket_array_member(
    file: &mut fs::File,
    symbols: &SymbolTable,
    cache: &mut BTreeMap<(String, Option<String>), BTreeMap<String, Value>>,
    directory_entries: &mut serde_json::Map<String, Value>,
    bucket: &str,
    key: &str,
    member: &str,
    position: Option<i64>,
    base_offset: u64,
    appended: &mut Vec<u8>,
    next_page_seq: &mut u64,
    next_seq: &mut u64,
) -> PactResult<()> {
    let mut entries =
        read_hot_bucket_entries(file, symbols, cache, directory_entries, bucket, None)?;
    let mut values = entries
        .remove(key)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|value| value.as_str() != Some(member))
        .collect::<Vec<_>>();
    let insert_at = position
        .and_then(|position| usize::try_from(position).ok())
        .map(|position| position.min(values.len()))
        .unwrap_or(values.len());
    values.insert(insert_at, Value::String(member.to_string()));
    entries.insert(key.to_string(), Value::Array(values));
    write_hot_bucket_entries(
        directory_entries,
        cache,
        bucket,
        None,
        entries,
        symbols,
        base_offset,
        appended,
        next_page_seq,
        next_seq,
    )
}

#[allow(clippy::too_many_arguments)]
fn merge_hot_bucket_entry(
    file: &mut fs::File,
    symbols: &SymbolTable,
    cache: &mut BTreeMap<(String, Option<String>), BTreeMap<String, Value>>,
    directory_entries: &mut serde_json::Map<String, Value>,
    bucket: &str,
    shard: Option<&str>,
    key: &str,
    value: Value,
    base_offset: u64,
    appended: &mut Vec<u8>,
    next_page_seq: &mut u64,
    next_seq: &mut u64,
) -> PactResult<()> {
    let mut entries =
        read_hot_bucket_entries(file, symbols, cache, directory_entries, bucket, shard)?;
    entries.insert(key.to_string(), value);
    write_hot_bucket_entries(
        directory_entries,
        cache,
        bucket,
        shard,
        entries,
        symbols,
        base_offset,
        appended,
        next_page_seq,
        next_seq,
    )
}

fn read_hot_bucket_entries(
    file: &mut fs::File,
    symbols: &SymbolTable,
    cache: &mut BTreeMap<(String, Option<String>), BTreeMap<String, Value>>,
    directory_entries: &serde_json::Map<String, Value>,
    bucket: &str,
    shard: Option<&str>,
) -> PactResult<BTreeMap<String, Value>> {
    let cache_key = (bucket.to_string(), shard.map(str::to_string));
    if let Some(entries) = cache.get(&cache_key) {
        return Ok(entries.clone());
    }
    let Some(address) = hot_bucket_address(directory_entries, bucket, shard) else {
        return Ok(BTreeMap::new());
    };
    let mut bytes_read = 0_u64;
    let Some(record) = read_record_from_file(file, &address, &mut bytes_read, symbols)? else {
        return Ok(BTreeMap::new());
    };
    let entries: BTreeMap<String, Value> = record
        .payload
        .get("entries")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    cache.insert(cache_key, entries.clone());
    Ok(entries)
}

fn hot_bucket_address(
    directory_entries: &serde_json::Map<String, Value>,
    bucket: &str,
    shard: Option<&str>,
) -> Option<PactRecordAddress> {
    let address = if let Some(shard) = shard {
        directory_entries.get(bucket)?.get(shard)?
    } else {
        directory_entries.get(bucket)?
    };
    serde_json::from_value(address.clone()).ok()
}

#[allow(clippy::too_many_arguments)]
fn write_hot_bucket_entries(
    directory_entries: &mut serde_json::Map<String, Value>,
    cache: &mut BTreeMap<(String, Option<String>), BTreeMap<String, Value>>,
    bucket: &str,
    shard: Option<&str>,
    entries: BTreeMap<String, Value>,
    symbols: &SymbolTable,
    base_offset: u64,
    appended: &mut Vec<u8>,
    next_page_seq: &mut u64,
    next_seq: &mut u64,
) -> PactResult<()> {
    let index_name = if let Some(shard) = shard {
        format!("{HOT_QUERY_INDEX_NAME}.{bucket}.{shard}")
    } else {
        format!("{HOT_QUERY_INDEX_NAME}.{bucket}")
    };
    let cached_entries = entries.clone();
    let record =
        encode_index_record(&index_name, entries, *next_seq, symbols).ok_or_else(|| {
            PactError::codec(format!(
                "failed to encode updated hot query subindex {index_name}"
            ))
        })?;
    let page_offset = base_offset + appended.len() as u64;
    write_page(appended, PageKind::Index, *next_page_seq, &[&record])?;
    let address = PactRecordAddress {
        page_offset,
        page_seq: *next_page_seq,
        record_seq: record.seq,
        record_kind: RecordKind::Index as u16,
        payload_len: Some(record.payload.len() as u32),
    };
    if let Some(shard) = shard {
        let entry = directory_entries
            .entry(bucket.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(object) = entry.as_object_mut() {
            object.insert(shard.to_string(), json!(address));
        }
    } else {
        directory_entries.insert(bucket.to_string(), json!(address));
    }
    cache.insert(
        (bucket.to_string(), shard.map(str::to_string)),
        cached_entries,
    );
    *next_seq += 1;
    *next_page_seq += 1;
    Ok(())
}

fn fast_append_manifest_payload(context: &FastAppendContext, appended_nodes: usize) -> Value {
    if !context.manifest_counts_known {
        return context.manifest_meta.clone();
    }
    let mut payload = context.manifest_meta.clone();
    let counts = incremental_manifest_payload(&context.manifest, 0, 0, appended_nodes);
    let Some(object) = payload.as_object_mut() else {
        return counts;
    };
    if let Some(counts) = counts.as_object() {
        for (key, value) in counts {
            object.insert(key.clone(), value.clone());
        }
    }
    payload
}

fn apply_manifest_counts_from_meta(manifest: &mut PactFileManifest, meta: &Value) {
    if let Some(file_kind) = meta.get("file_kind").and_then(Value::as_str) {
        manifest.file_kind = file_kind.to_string();
    }
    set_manifest_count(meta, "record_count", &mut manifest.record_count);
    set_manifest_count(
        meta,
        "content_record_count",
        &mut manifest.content_record_count,
    );
    set_manifest_count(
        meta,
        "content_chunk_count",
        &mut manifest.content_chunk_count,
    );
    set_manifest_count(
        meta,
        "content_manifest_count",
        &mut manifest.content_manifest_count,
    );
    set_manifest_count(meta, "root_count", &mut manifest.root_count);
    set_manifest_count(meta, "branch_count", &mut manifest.branch_count);
    set_manifest_count(meta, "node_count", &mut manifest.node_count);
    set_manifest_count(meta, "trace_count", &mut manifest.trace_count);
    set_manifest_count(meta, "component_count", &mut manifest.component_count);
    set_manifest_count(
        meta,
        "component_transaction_count",
        &mut manifest.component_transaction_count,
    );
    set_manifest_count(
        meta,
        "component_payload_count",
        &mut manifest.component_payload_count,
    );
    set_manifest_count(meta, "view_count", &mut manifest.view_count);
    set_manifest_count(meta, "commit_count", &mut manifest.commit_count);
    set_manifest_count(meta, "frozen_cycle_count", &mut manifest.frozen_cycle_count);
    set_manifest_count(meta, "head_count", &mut manifest.head_count);
}

fn set_manifest_count(meta: &Value, key: &str, target: &mut usize) {
    if let Some(value) = meta.get(key).and_then(Value::as_u64) {
        *target = value as usize;
    }
}

fn externalize_large_commit_content_refs(
    value: &mut Value,
    content: &mut ContentStore,
    blob_inline_limit: usize,
) {
    match value {
        Value::Object(object) => {
            if let Some(text) = object.get("content").and_then(Value::as_str)
                && text.chars().count() > blob_inline_limit
            {
                let record = content.put_inline("node.content", text, blob_inline_limit);
                object.remove("content");
                object.insert("content_ref".to_string(), Value::String(record.id));
                object.remove("content_preview");
            }
            for child in object.values_mut() {
                externalize_large_commit_content_refs(child, content, blob_inline_limit);
            }
        }
        Value::Array(items) => {
            for item in items {
                externalize_large_commit_content_refs(item, content, blob_inline_limit);
            }
        }
        _ => {}
    }
}

fn value_node_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn value_children(value: &Value) -> Vec<&Value> {
    value
        .get("children")
        .and_then(Value::as_array)
        .map(|children| children.iter().collect())
        .unwrap_or_default()
}

fn value_without_children(value: &Value) -> Value {
    let mut clone = value.clone();
    if let Some(object) = clone.as_object_mut() {
        object.remove("children");
    }
    clone
}

const DIFF_BLOB_PREVIEW_CHARS: usize = 64;

pub(super) fn diff_value_trees(
    old_node: &Value,
    new_node: &Value,
    ops: &mut Vec<CommitRecordOp>,
    nodes: &mut BTreeMap<String, Value>,
    content: &mut ContentStore,
) -> PactResult<()> {
    let old_id = value_node_id(old_node);
    let new_id = value_node_id(new_node);
    if old_id.is_none() || new_id.is_none() || old_id != new_id {
        let node_ref = new_id.unwrap_or_else(|| format!("node_{:08x}", nodes.len()));
        nodes.insert(node_ref.clone(), new_node.clone());
        if let Some(component_id) = old_id {
            ops.push(CommitRecordOp::ReplaceComponent {
                component_id,
                node_ref,
            });
        }
        return Ok(());
    }

    let Some(component_id) = new_id else {
        return Err(PactError::internal(
            "value-tree diff lost the validated replacement component id",
        ));
    };
    if value_without_children(old_node) != value_without_children(new_node) {
        if !emit_in_place_node_deltas(&component_id, old_node, new_node, ops, content) {
            nodes.insert(component_id.clone(), new_node.clone());
            ops.push(CommitRecordOp::ReplaceComponent {
                component_id: component_id.clone(),
                node_ref: component_id,
            });
            return Ok(());
        }
    }

    let old_children = value_children(old_node);
    let new_children = value_children(new_node);
    let old_by_id = old_children
        .iter()
        .filter_map(|child| value_node_id(child).map(|id| (id, *child)))
        .collect::<BTreeMap<_, _>>();
    let new_ids = new_children
        .iter()
        .filter_map(|child| value_node_id(child))
        .collect::<BTreeSet<_>>();

    for child_id in old_by_id.keys() {
        if !new_ids.contains(child_id) {
            ops.push(CommitRecordOp::DeleteComponent {
                component_id: child_id.clone(),
            });
        }
    }

    for (index, child) in new_children.iter().enumerate() {
        let Some(child_id) = value_node_id(child) else {
            continue;
        };
        if let Some(old_child) = old_by_id.get(&child_id) {
            diff_value_trees(old_child, child, ops, nodes, content)?;
        } else {
            nodes.insert(child_id.clone(), (*child).clone());
            ops.push(CommitRecordOp::InsertComponent {
                parent_id: component_id.clone(),
                node_ref: child_id,
                position: Some(index as i64),
            });
        }
    }
    Ok(())
}

fn emit_in_place_node_deltas(
    component_id: &str,
    old_node: &Value,
    new_node: &Value,
    ops: &mut Vec<CommitRecordOp>,
    content: &mut ContentStore,
) -> bool {
    let (Some(old_obj), Some(new_obj)) = (old_node.as_object(), new_node.as_object()) else {
        return false;
    };

    let is_delta_field = |key: &str| {
        matches!(
            key,
            "aux" | "content" | "content_ref" | "content_preview" | "children"
        )
    };
    for key in old_obj.keys().chain(new_obj.keys()) {
        if is_delta_field(key) {
            continue;
        }
        if old_obj.get(key) != new_obj.get(key) {
            return false;
        }
    }

    let mut staged: Vec<CommitRecordOp> = Vec::new();

    let old_attrs = old_obj.get("aux").and_then(Value::as_object);
    let new_attrs = new_obj.get("aux").and_then(Value::as_object);
    if old_attrs != new_attrs {
        let empty = serde_json::Map::new();
        let oa = old_attrs.unwrap_or(&empty);
        let na = new_attrs.unwrap_or(&empty);
        for (key, value) in na {
            if oa.get(key) != Some(value) {
                let value_ref = content
                    .put_inline(
                        "node.attr",
                        &serde_json::to_string(value).unwrap_or_default(),
                        DIFF_BLOB_PREVIEW_CHARS,
                    )
                    .id;
                staged.push(CommitRecordOp::SetComponentAux {
                    component_id: component_id.to_string(),
                    key_symbol: key.clone(),
                    value_ref,
                });
            }
        }
        for key in oa.keys() {
            if !na.contains_key(key) {
                staged.push(CommitRecordOp::UnsetComponentAux {
                    component_id: component_id.to_string(),
                    key_symbol: key.clone(),
                });
            }
        }
    }

    let content_changed = old_obj.get("content") != new_obj.get("content")
        || old_obj.get("content_ref") != new_obj.get("content_ref");
    if content_changed {
        match content_change_op(component_id, new_obj, content) {
            Some(op) => staged.push(op),
            None => return false,
        }
    }

    ops.extend(staged);
    true
}

fn content_change_op(
    component_id: &str,
    new_obj: &serde_json::Map<String, Value>,
    content: &mut ContentStore,
) -> Option<CommitRecordOp> {
    if let Some(new_text) = new_obj.get("content").and_then(Value::as_str) {
        let blob_id = content
            .put_inline("node.content", new_text, DIFF_BLOB_PREVIEW_CHARS)
            .id;
        return Some(CommitRecordOp::AttachContentRef {
            component_id: component_id.to_string(),
            blob_id,
        });
    }
    if let Some(ref_id) = value_content_ref_id(new_obj.get("content_ref")) {
        if content.records.contains_key(&ref_id) {
            return Some(CommitRecordOp::AttachContentRef {
                component_id: component_id.to_string(),
                blob_id: ref_id,
            });
        }
        return None;
    }
    None
}

fn value_content_ref_id(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    let object = value.as_object()?;
    object
        .get("id")
        .or_else(|| object.get("ref_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn file_reanchor_json(
    source_path: &Path,
    output_path: &Path,
    options: ReanchorOptions,
) -> PactResult<String> {
    let bytes = fs::read(source_path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&bytes) {
        let ledger = crate::pact_file::open_ledger(source_path)?;
        let output_bytes = crate::pact_file::encode_canonical_file(&ledger)?;
        let mode = if output_path == source_path {
            PublishMode::ReplaceExisting
        } else {
            PublishMode::CreateNew
        };
        publish_bytes(output_path, &output_bytes, mode)?;
        invalidate_scan_cache(output_path);
        let output_manifest = super::validate_file(output_path)?.manifest;
        return serde_json::to_string_pretty(&json!({
            "schema": format!("{PACT_FILE_SCHEMA}.reanchor"),
            "source_path": source_path.display().to_string(),
            "output_path": output_path.display().to_string(),
            "anchor_policy": {
                "freeze_cycle_interval": options.freeze_cycle_interval,
                "freeze_commit_bytes": options.freeze_commit_bytes,
            },
            "source_frozen_cycle_count": 0,
            "anchors_written": 0,
            "source_file_bytes": bytes.len(),
            "output_file_bytes": output_bytes.len(),
            "output_manifest": output_manifest,
        }))
        .map_err(|err| PactError::codec(format!("failed to encode .pact reanchor report: {err}")));
    }
    let scanned = scan_file(&bytes, true);
    let source_validation = validation_from_scan(scanned.clone());
    if !source_validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot reanchor invalid .pact file: {}",
            source_validation.errors.join("; ")
        )));
    }

    let records_without_anchors = records_without_frozen_anchors(&scanned.records);
    let anchor_records = build_frozen_anchor_records(&records_without_anchors, &options)?;
    let mut builder = PactFileBuilder {
        records: Vec::new(),
    };
    for record in &records_without_anchors {
        let Some(kind) = RecordKind::from_str(&record.kind) else {
            return Err(PactError::corrupt(format!(
                "cannot reanchor unknown record kind: {}",
                record.kind
            )));
        };
        builder.push(kind, &record.payload)?;
    }
    let anchors_written = anchor_records
        .iter()
        .filter(|(kind, _)| *kind == RecordKind::FrozenCycle)
        .count();
    for (kind, payload) in &anchor_records {
        builder.push(*kind, payload)?;
    }

    let output_bytes = builder.encode()?;
    let mode = if output_path == source_path {
        PublishMode::ReplaceExisting
    } else {
        PublishMode::CreateNew
    };
    publish_bytes(output_path, &output_bytes, mode)?;
    invalidate_scan_cache(output_path);
    let output_bytes = fs::read(output_path).map_err(|err| {
        PactError::storage(format!("failed to read reanchored .pact file: {err}"))
    })?;
    let output_scan = scan_file(&output_bytes, true);
    let output_validation = validation_from_scan(output_scan.clone());
    if !output_validation.ok {
        return Err(PactError::corrupt(format!(
            "reanchored .pact failed validation: {}",
            output_validation.errors.join("; ")
        )));
    }

    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.reanchor"),
        "source_path": source_path.display().to_string(),
        "output_path": output_path.display().to_string(),
        "anchor_policy": {
            "freeze_cycle_interval": options.freeze_cycle_interval,
            "freeze_commit_bytes": options.freeze_commit_bytes,
        },
        "source_frozen_cycle_count": scanned.manifest.frozen_cycle_count,
        "anchors_written": anchors_written,
        "source_file_bytes": bytes.len(),
        "output_file_bytes": output_bytes.len(),
        "output_manifest": output_scan.manifest,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact reanchor report: {err}")))
}

pub fn file_reindex_head_json(
    source_path: &Path,
    output_path: Option<&Path>,
) -> PactResult<String> {
    let bytes = fs::read(source_path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let source_validation = validation_from_scan(scanned.clone());
    if !source_validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot reindex invalid .pact file: {}",
            source_validation.errors.join("; ")
        )));
    }
    let latest_commit = scanned
        .records
        .iter()
        .rev()
        .find(|record| record.kind == "commit")
        .ok_or_else(|| PactError::not_found("cannot reindex head without commit records"))?;
    let latest_commit: CommitRecord = serde_json::from_value(latest_commit.payload.clone())
        .map_err(|err| {
            PactError::codec(format!(
                "invalid latest commit record during head reindex: {err}"
            ))
        })?;

    let target_path = output_path.unwrap_or(source_path);
    if output_path.is_some_and(|output_path| output_path != source_path) {
        publish_bytes(target_path, &bytes, PublishMode::CreateNew)?;
    }

    let target_bytes = fs::read(target_path)
        .map_err(|err| PactError::storage(format!("failed to read target .pact file: {err}")))?;
    let target_scan = scan_file(&target_bytes, true);
    let target_validation = validation_from_scan(target_scan.clone());
    if !target_validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot reindex invalid target .pact file: {}",
            target_validation.errors.join("; ")
        )));
    }
    append_reindexed_head_record(target_path, &target_scan, &latest_commit)?;
    invalidate_scan_cache(target_path);
    let output_bytes = fs::read(target_path)
        .map_err(|err| PactError::storage(format!("failed to read reindexed .pact file: {err}")))?;
    let output_scan = scan_file(&output_bytes, true);
    let output_validation = validation_from_scan(output_scan.clone());
    if !output_validation.ok {
        return Err(PactError::corrupt(format!(
            "reindexed .pact failed validation: {}",
            output_validation.errors.join("; ")
        )));
    }

    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.reindex_head.v1"),
        "source_path": source_path.display().to_string(),
        "output_path": target_path.display().to_string(),
        "source_head_count": scanned.manifest.head_count,
        "output_head_count": output_scan.manifest.head_count,
        "head_revision": latest_commit.revision,
        "head_cycle": latest_commit.cycle,
        "anchors_written": 0,
        "output_manifest": output_scan.manifest,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact head reindex report: {err}")))
}

#[derive(Clone, Copy, Debug)]
pub struct PactRepackOptions {
    pub token_span_strings: bool,
}

impl Default for PactRepackOptions {
    fn default() -> Self {
        Self {
            token_span_strings: true,
        }
    }
}

pub fn file_repack_json(source_path: &Path, output_path: &Path) -> PactResult<String> {
    file_repack_with_options_json(source_path, output_path, PactRepackOptions::default())
}

pub fn file_repack_with_options_json(
    source_path: &Path,
    output_path: &Path,
    options: PactRepackOptions,
) -> PactResult<String> {
    if source_path == output_path {
        return Err(PactError::invalid(
            "file-repack requires a distinct output path; refusing to overwrite source .pact"
                .to_string(),
        ));
    }
    let bytes = fs::read(source_path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&bytes) {
        let ledger = crate::pact_file::open_ledger(source_path)?;
        let output_bytes = crate::pact_file::encode_canonical_file(&ledger)?;
        publish_bytes(output_path, &output_bytes, PublishMode::CreateNew)?;
        invalidate_scan_cache(output_path);
        let source_manifest = super::validate_file(source_path)?.manifest;
        let output_manifest = super::validate_file(output_path)?.manifest;
        return serde_json::to_string_pretty(&json!({
            "schema": format!("{PACT_FILE_SCHEMA}.repack.v1"),
            "source_path": source_path.display().to_string(),
            "output_path": output_path.display().to_string(),
            "source_file_bytes": bytes.len(),
            "output_file_bytes": output_bytes.len(),
            "token_span_strings": options.token_span_strings,
            "source_record_count": source_manifest.record_count,
            "output_record_count": output_manifest.record_count,
            "source_content_chunk_count": source_manifest.content_chunk_count,
            "output_content_chunk_count": output_manifest.content_chunk_count,
            "head_preserved": true,
            "commit_history_preserved": true,
            "output_manifest": output_manifest,
        }))
        .map_err(|err| PactError::codec(format!("failed to encode .pact repack report: {err}")));
    }
    let scanned = scan_file(&bytes, true);
    let source_validation = validation_from_scan(scanned.clone());
    if !source_validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot repack invalid .pact file: {}",
            source_validation.errors.join("; ")
        )));
    }

    let output_bytes = repack_records_preserving_history_with_options(&scanned.records, options)?;
    publish_bytes(output_path, &output_bytes, PublishMode::CreateNew)?;
    invalidate_scan_cache(output_path);

    if let Some(latest_commit_record) = scanned
        .records
        .iter()
        .rev()
        .find(|record| record.kind == "commit")
    {
        let latest_commit: CommitRecord =
            serde_json::from_value(latest_commit_record.payload.clone()).map_err(|err| {
                PactError::codec(format!("invalid latest commit record during repack: {err}"))
            })?;
        let output_scan = scan_file(
            &fs::read(output_path).map_err(|err| {
                PactError::storage(format!("failed to read repacked .pact file: {err}"))
            })?,
            true,
        );
        append_reindexed_head_record(output_path, &output_scan, &latest_commit)?;
        invalidate_scan_cache(output_path);
    }

    let output_bytes = fs::read(output_path)
        .map_err(|err| PactError::storage(format!("failed to read repacked .pact file: {err}")))?;
    let output_scan = scan_file(&output_bytes, true);
    let output_validation = validation_from_scan(output_scan.clone());
    if !output_validation.ok {
        return Err(PactError::corrupt(format!(
            "repacked .pact failed validation: {}",
            output_validation.errors.join("; ")
        )));
    }
    let source_head = file_project_head_json(source_path)?;
    let output_head = file_project_head_json(output_path)?;
    if source_head != output_head {
        return Err(PactError::corrupt(
            "repacked .pact changed provider-visible head".to_string(),
        ));
    }

    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.repack.v1"),
        "source_path": source_path.display().to_string(),
        "output_path": output_path.display().to_string(),
        "source_file_bytes": bytes.len(),
        "output_file_bytes": output_bytes.len(),
        "token_span_strings": options.token_span_strings,
        "source_record_count": scanned.manifest.record_count,
        "output_record_count": output_scan.manifest.record_count,
        "source_content_chunk_count": scanned.manifest.content_chunk_count,
        "output_content_chunk_count": output_scan.manifest.content_chunk_count,
        "head_preserved": true,
        "commit_history_preserved": scanned.manifest.commit_count == output_scan.manifest.commit_count,
        "output_manifest": output_scan.manifest,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact repack report: {err}")))
}

pub fn file_repack_packed_v1_json(_source_path: &Path, _output_path: &Path) -> PactResult<String> {
    Err(PactError::unsupported(
        "the pre-v0.1 packed output format is unsupported",
    ))
}

pub(super) fn packed_v1_to_v0_bytes(_packed_bytes: &[u8]) -> PactResult<Vec<u8>> {
    Err(PactError::unsupported(
        "pre-v0.1 packed Pact files are unsupported",
    ))
}

pub(super) fn repack_records_preserving_history(records: &[ScannedRecord]) -> PactResult<Vec<u8>> {
    repack_records_preserving_history_with_options(records, PactRepackOptions::default())
}

pub(super) fn repack_records_preserving_history_with_options(
    records: &[ScannedRecord],
    options: PactRepackOptions,
) -> PactResult<Vec<u8>> {
    let chunks = content_chunks_from_records(records)?;
    let mut content = ContentCollector::with_compression();
    for record in records.iter().filter(|record| record.kind == "content") {
        let payload: ContentPayload =
            serde_json::from_value(record.payload.clone()).map_err(|err| {
                PactError::codec(format!(
                    "invalid content record payload during repack: {err}"
                ))
            })?;
        let (inline, inline_bytes) = if payload.text.is_some() {
            let bytes = decode_content_payload_bytes(&payload)?;
            if payload.chars.is_some() {
                (
                    Some(String::from_utf8(bytes).map_err(|err| {
                        PactError::corrupt(format!(
                            "content {} payload is not UTF-8: {err}",
                            payload.id
                        ))
                    })?),
                    None,
                )
            } else {
                (None, Some(bytes))
            }
        } else {
            let bytes = chunks
                .get(&payload.id)
                .map(|parts| {
                    Ok::<Vec<u8>, PactError>(
                        parts
                            .iter()
                            .flat_map(|(_, chunk)| chunk.iter().copied())
                            .collect::<Vec<_>>(),
                    )
                })
                .transpose()?;
            if let Some(bytes) = bytes {
                if payload.chars.is_some() {
                    (
                        Some(String::from_utf8(bytes).map_err(|err| {
                            PactError::corrupt(format!(
                                "content {} chunks are not UTF-8: {err}",
                                payload.id
                            ))
                        })?),
                        None,
                    )
                } else {
                    (None, Some(bytes))
                }
            } else {
                (None, None)
            }
        };
        let preview = payload
            .preview_inline
            .clone()
            .unwrap_or_else(|| inline.as_deref().map(inline_preview).unwrap_or_default());
        let storage = if let Some(inline) = inline {
            ContentStorageRef {
                kind: "inline".to_string(),
                inline: Some(inline),
                inline_bytes: None,
                path: None,
                external_ref: None,
            }
        } else if let Some(bytes) = inline_bytes {
            ContentStorageRef {
                kind: "inline_bytes".to_string(),
                inline: None,
                inline_bytes: Some(bytes),
                path: None,
                external_ref: None,
            }
        } else {
            ContentStorageRef {
                kind: "external".to_string(),
                inline: None,
                inline_bytes: None,
                path: None,
                external_ref: Some(crate::components::ExternalRef {
                    kind: "content_storage".to_string(),
                    uri: Some(payload.id.clone()),
                    role: None,
                    data: None,
                }),
            }
        };
        let content_record = ContentRecord {
            id: payload.id.clone(),
            kind: payload.kind,
            hash: payload.hash,
            bytes: payload.bytes,
            chars: payload.chars,
            preview,
            storage,
        };
        content.put_content_record(&content_record)?;
    }

    let mut builder = PactFileBuilder {
        records: Vec::new(),
    };
    for payload in content.by_hash.values() {
        builder.push(RecordKind::Content, payload)?;
    }
    for chunk in &content.chunks {
        builder.push(RecordKind::ContentChunk, chunk)?;
    }
    let mut wrote_file_meta = false;
    for record in records {
        let Some(kind) = RecordKind::from_str(&record.kind) else {
            return Err(PactError::corrupt(format!(
                "cannot repack unknown record kind: {}",
                record.kind
            )));
        };
        if kind == RecordKind::Meta {
            let mut payload = record.payload.clone();
            if !wrote_file_meta && is_file_meta_payload(&payload) {
                apply_repack_codec_metadata(&mut payload, options);
                wrote_file_meta = true;
            }
            builder.push(kind, &payload)?;
            continue;
        }
        if matches!(
            kind,
            RecordKind::Content
                | RecordKind::ContentChunk
                | RecordKind::Symbol
                | RecordKind::Head
                | RecordKind::Index
        ) {
            continue;
        }
        builder.push(kind, &record.payload)?;
    }
    builder.encode_with_options(super::value_codec::PactValueCodecOptions {
        token_span_strings: options.token_span_strings,
    })
}

fn apply_repack_codec_metadata(payload: &mut Value, options: PactRepackOptions) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    if options.token_span_strings {
        object.insert(
            "capabilities".to_string(),
            json!([
                super::layout::TRACE_COMPACT_V2,
                super::layout::TOKEN_SPAN_STRINGS_V1,
                super::layout::ENGLISH_LEXPACK_V1,
                super::layout::SHELL_LEXPACK_V1
            ]),
        );
        object.insert(
            "codec_substrate".to_string(),
            json!({
                "schema": "pact.codec_substrate.v1",
                "tokenizers": [{
                    "tokenizer_id": "tok_0",
                    "family": "openai_tiktoken",
                    "name": "o200k_base",
                    "version": "tiktoken-rs:0.11",
                    "normalizer": "none",
                    "reversible": true,
                }],
                "lexical_packs": [{
                    "pack_id": super::layout::ENGLISH_LEXPACK_ID,
                    "tokenizer_id": "tok_0",
                }, {
                    "pack_id": super::layout::SHELL_LEXPACK_ID,
                    "tokenizer_id": "tok_0",
                }],
            }),
        );
    } else {
        object.insert(
            "capabilities".to_string(),
            Value::Array(vec![Value::String(
                super::layout::TRACE_COMPACT_V2.to_string(),
            )]),
        );
        object.insert("codec_substrate".to_string(), Value::Null);
    }
}

fn is_file_meta_payload(payload: &Value) -> bool {
    payload
        .get("schema")
        .and_then(Value::as_str)
        .is_some_and(|schema| schema == super::layout::PACT_FILE_SCHEMA)
        || (payload.get("root_id").is_some() && payload.get("current_branch_id").is_some())
}

fn inline_preview(text: &str) -> String {
    text.chars().take(160).collect()
}

pub(super) fn records_with_monotonic_commit_cycles(
    records: &[ScannedRecord],
) -> PactResult<Vec<ScannedRecord>> {
    let mut repaired = Vec::with_capacity(records.len());
    let mut next_cycle = 1_u64;
    for record in records {
        let mut record = record.clone();
        if record.kind == "commit" {
            let mut commit: CommitRecord =
                serde_json::from_value(record.payload.clone()).map_err(|err| {
                    PactError::codec(format!(
                        "invalid commit record during logical repair: {err}"
                    ))
                })?;
            commit.cycle = next_cycle;
            commit.revision = next_cycle;
            record.payload = serde_json::to_value(commit).map_err(|err| {
                PactError::codec(format!("failed to encode repaired commit record: {err}"))
            })?;
            next_cycle += 1;
        }
        repaired.push(record);
    }
    Ok(repaired)
}

pub(super) fn append_reindexed_head_record(
    path: &Path,
    scanned: &ScannedFile,
    commit: &CommitRecord,
) -> PactResult<()> {
    let mut bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let mut next_seq = scanned.manifest.record_count as u64 + 1;
    let next_page_seq = scanned
        .manifest
        .pages
        .iter()
        .map(|page| page.page_seq)
        .max()
        .unwrap_or(0)
        + 1;
    let append_offset = bytes.len();
    let record_page_offset = bytes.len() as u64;
    let commit_offset = scanned
        .manifest
        .latest_footer
        .as_ref()
        .map(|footer| footer.last_commit_offset)
        .filter(|offset| *offset > 0)
        .unwrap_or(record_page_offset);
    let head_record = latest_head_record_from_records(&scanned.records)
        .map(|mut head| {
            head.commit_offset = commit_offset;
            head
        })
        .unwrap_or_else(|| {
            head_record_from_commit(
                commit,
                commit_offset,
                root_node_ref_from_records(&scanned.records),
                &[],
                node_record_sequences_from_records(&scanned.records),
            )
        });
    let head = EncodedRecord {
        kind: RecordKind::Head,
        seq: next_seq,
        payload: encode_record_payload(RecordKind::Head, &head_record)?,
    };
    next_seq += 1;
    let meta = EncodedRecord {
        kind: RecordKind::Meta,
        seq: next_seq,
        payload: encode_record_payload(
            RecordKind::Meta,
            &head_reindex_manifest_payload(&scanned.manifest),
        )?,
    };
    let refs = vec![&head, &meta];
    write_page(&mut bytes, PageKind::Record, next_page_seq, &refs)?;
    let footer = scanned.manifest.latest_footer.clone().unwrap_or_default();
    write_footer(
        &mut bytes,
        record_page_offset,
        record_page_offset,
        footer.hot_index_offset,
        commit_offset,
    );
    append_bytes(path, &bytes[append_offset..])?;
    Ok(())
}

pub(super) fn build_head_record_for_commit(
    scanned: &ScannedFile,
    commit: &CommitRecord,
    commit_offset: u64,
    appended_nodes: &[Value],
    appended_node_sequences: &BTreeMap<String, u64>,
    _content_store: Option<&ContentStore>,
) -> PactResult<HeadRecord> {
    let root_node_ref = latest_head_record_from_records(&scanned.records)
        .and_then(|head| head.root_node_ref)
        .or_else(|| root_node_ref_from_records(&scanned.records));
    Ok(head_record_from_commit(
        commit,
        commit_offset,
        root_node_ref,
        appended_nodes,
        appended_node_sequences.clone(),
    ))
}

pub(super) fn latest_head_record_from_records(records: &[ScannedRecord]) -> Option<HeadRecord> {
    records
        .iter()
        .rev()
        .filter(|record| record.kind == "head")
        .find_map(|record| serde_json::from_value::<HeadRecord>(record.payload.clone()).ok())
}

pub(super) fn root_node_ref_from_records(records: &[ScannedRecord]) -> Option<String> {
    records.iter().find_map(|record| {
        if record.kind == "root" {
            record
                .payload
                .get("root_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        }
    })
}

pub(super) fn head_record_from_commit(
    commit: &CommitRecord,
    commit_offset: u64,
    root_node_ref: Option<String>,
    appended_nodes: &[Value],
    record_sequence_ids: BTreeMap<String, u64>,
) -> HeadRecord {
    HeadRecord {
        schema: HEAD_RECORD_SCHEMA.to_string(),
        branch_id: commit.branch_id.clone(),
        head_revision: commit.revision,
        head_cycle: commit.cycle,
        lifecycle_tick: commit.lifecycle_tick,
        commit_offset,
        root_node_ref,
        changed_node_refs: changed_node_refs_from_commit(commit),
        content_ref_ids: content_ref_ids_from_commit_and_nodes(commit, appended_nodes),
        record_sequence_ids,
    }
}

pub(super) fn changed_node_refs_from_commit(commit: &CommitRecord) -> Vec<String> {
    let mut refs = Vec::new();
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent { node_ref, .. } => {
                push_unique(&mut refs, node_ref.clone());
            }
            CommitRecordOp::ReplaceComponent {
                component_id,
                node_ref,
            } => {
                push_unique(&mut refs, component_id.clone());
                push_unique(&mut refs, node_ref.clone());
            }
            CommitRecordOp::DeleteComponent { component_id } => {
                push_unique(&mut refs, component_id.clone());
            }
            CommitRecordOp::AppendComponents {
                parent_id,
                node_refs,
            } => {
                push_unique(&mut refs, parent_id.clone());
                for node_ref in node_refs {
                    push_unique(&mut refs, node_ref.clone());
                }
            }
            CommitRecordOp::SetComponentAux { component_id, .. }
            | CommitRecordOp::UnsetComponentAux { component_id, .. }
            | CommitRecordOp::AttachContentRef { component_id, .. }
            | CommitRecordOp::ReplaceContentRange { component_id, .. } => {
                push_unique(&mut refs, component_id.clone());
            }
        }
    }
    refs
}

pub(super) fn content_ref_ids_from_commit_and_nodes(
    commit: &CommitRecord,
    appended_nodes: &[Value],
) -> Vec<String> {
    let mut refs = Vec::new();
    for op in &commit.ops {
        match op {
            CommitRecordOp::SetComponentAux { value_ref, .. } => {
                push_unique(&mut refs, value_ref.clone());
            }
            CommitRecordOp::AttachContentRef { blob_id, .. }
            | CommitRecordOp::ReplaceContentRange { blob_id, .. } => {
                push_unique(&mut refs, blob_id.clone());
            }
            CommitRecordOp::InsertComponent { .. }
            | CommitRecordOp::ReplaceComponent { .. }
            | CommitRecordOp::DeleteComponent { .. }
            | CommitRecordOp::UnsetComponentAux { .. }
            | CommitRecordOp::AppendComponents { .. } => {}
        }
    }
    for node in appended_nodes {
        let mut node_refs = BTreeSet::new();
        collect_content_ref_ids_from_value(node, &mut node_refs);
        for node_ref in node_refs {
            push_unique(&mut refs, node_ref);
        }
    }
    refs
}

pub(super) fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|item| item == &value) {
        values.push(value);
    }
}

pub(super) fn node_record_sequences_from_records(
    records: &[ScannedRecord],
) -> BTreeMap<String, u64> {
    records
        .iter()
        .filter(|record| record.kind == "node")
        .filter_map(|record| {
            record
                .payload
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), record.sequence))
        })
        .collect()
}

pub(super) fn records_without_frozen_anchors(records: &[ScannedRecord]) -> Vec<ScannedRecord> {
    let mut frozen_content_ids = BTreeSet::new();
    for record in records {
        match record.kind.as_str() {
            "frozen_cycle" => {
                if let Some(root_ref) = record.payload.get("root_ref").and_then(Value::as_str) {
                    frozen_content_ids.insert(root_ref.to_string());
                }
            }
            "content" => {
                if record.payload.get("kind").and_then(Value::as_str) == Some("frozen.root")
                    && let Some(content_id) = record.payload.get("id").and_then(Value::as_str)
                {
                    frozen_content_ids.insert(content_id.to_string());
                }
            }
            _ => {}
        }
    }

    records
        .iter()
        .filter(|record| match record.kind.as_str() {
            "frozen_cycle" => false,
            "content" => {
                let content_id = record.payload.get("id").and_then(Value::as_str);
                record.payload.get("kind").and_then(Value::as_str) != Some("frozen.root")
                    && content_id.is_none_or(|content_id| !frozen_content_ids.contains(content_id))
            }
            "content_chunk" => {
                let content_id = record.payload.get("content_id").and_then(Value::as_str);
                content_id.is_none_or(|content_id| !frozen_content_ids.contains(content_id))
            }
            _ => true,
        })
        .cloned()
        .collect()
}

pub(super) fn build_frozen_anchor_records(
    records: &[ScannedRecord],
    options: &ReanchorOptions,
) -> PactResult<Vec<(RecordKind, Value)>> {
    if options.freeze_cycle_interval == Some(0) && options.freeze_commit_bytes == Some(0) {
        return Ok(Vec::new());
    }

    let mut content = ContentCollector::default();
    let mut frozen_cycles = Vec::new();
    let mut bytes_since_last_freeze = 0_usize;
    let mut last_anchor_cycle = None;
    for record in records.iter().filter(|record| record.kind == "commit") {
        let payload_bytes = serde_json::to_vec(&record.payload).map_err(|err| {
            PactError::codec(format!(
                "failed to encode commit payload for reanchor: {err}"
            ))
        })?;
        bytes_since_last_freeze += payload_bytes.len();
        let commit: CommitRecord =
            serde_json::from_value(record.payload.clone()).map_err(|err| {
                PactError::codec(format!("invalid commit record during reanchor: {err}"))
            })?;
        let cycle_triggered = should_freeze_cycle_interval(
            commit.cycle,
            last_anchor_cycle,
            options.freeze_cycle_interval,
        );
        let bytes_triggered =
            should_freeze_commit_bytes(bytes_since_last_freeze, options.freeze_commit_bytes);
        if !cycle_triggered && !bytes_triggered {
            continue;
        }
        if last_anchor_cycle == Some(commit.cycle) {
            continue;
        }
        let root = project_records_revision_file_wire_value(records, commit.cycle)?;
        let root_json = serde_json::to_string(&root).map_err(|err| {
            PactError::codec(format!("failed to encode frozen cycle root: {err}"))
        })?;
        let root_ref = content.put_text(
            "frozen.root",
            &format!("cycle_{}", commit.cycle),
            &root_json,
        )?;
        frozen_cycles.push(FrozenCycleRecord {
            schema: FROZEN_CYCLE_SCHEMA.to_string(),
            cycle: commit.cycle,
            revision: commit.revision,
            root_ref,
            index_refs: Vec::new(),
            hash: stable_hash("frozen.root", root_json.as_bytes()),
        });
        bytes_since_last_freeze = 0;
        last_anchor_cycle = Some(commit.cycle);
    }

    let mut anchor_records = Vec::new();
    for payload in content.by_hash.values() {
        anchor_records.push((
            RecordKind::Content,
            serde_json::to_value(payload).map_err(|err| {
                PactError::codec(format!("failed to encode frozen content payload: {err}"))
            })?,
        ));
    }
    for chunk in &content.chunks {
        anchor_records.push((
            RecordKind::ContentChunk,
            serde_json::to_value(chunk).map_err(|err| {
                PactError::codec(format!("failed to encode frozen content chunk: {err}"))
            })?,
        ));
    }
    for frozen in frozen_cycles {
        anchor_records.push((
            RecordKind::FrozenCycle,
            serde_json::to_value(frozen).map_err(|err| {
                PactError::codec(format!("failed to encode frozen cycle payload: {err}"))
            })?,
        ));
    }
    Ok(anchor_records)
}

pub(super) fn append_missing_file_level_content_records(
    path: &Path,
    source_records: &[ScannedRecord],
) -> PactResult<usize> {
    let mut bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot preserve content records in invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let existing_content_ids = scanned
        .records
        .iter()
        .filter(|record| record.kind == "content")
        .filter_map(|record| record.payload.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let existing_chunks = scanned
        .records
        .iter()
        .filter(|record| record.kind == "content_chunk")
        .filter_map(|record| {
            let content_id = record.payload.get("content_id").and_then(Value::as_str)?;
            let ordinal = record.payload.get("ordinal").and_then(Value::as_u64)?;
            Some((content_id.to_string(), ordinal))
        })
        .collect::<BTreeSet<_>>();
    let existing_manifest_ids = scanned
        .records
        .iter()
        .filter(|record| record.kind == "content_manifest")
        .filter_map(|record| record.payload.get("manifest_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    let mut missing = Vec::new();
    for record in source_records {
        match record.kind.as_str() {
            "content" => {
                let Some(id) = record.payload.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if !existing_content_ids.contains(id) {
                    missing.push((RecordKind::Content, record.payload.clone()));
                }
            }
            "content_chunk" => {
                let Some(content_id) = record.payload.get("content_id").and_then(Value::as_str)
                else {
                    continue;
                };
                let ordinal = record
                    .payload
                    .get("ordinal")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                if !existing_chunks.contains(&(content_id.to_string(), ordinal)) {
                    missing.push((RecordKind::ContentChunk, record.payload.clone()));
                }
            }
            "content_manifest" => {
                let Some(manifest_id) = record.payload.get("manifest_id").and_then(Value::as_str)
                else {
                    continue;
                };
                if !existing_manifest_ids.contains(manifest_id) {
                    missing.push((RecordKind::ContentManifest, record.payload.clone()));
                }
            }
            _ => {}
        }
    }
    if missing.is_empty() {
        return Ok(0);
    }

    let next_seq = scanned.manifest.record_count as u64 + 1;
    let next_page_seq = scanned
        .manifest
        .pages
        .iter()
        .map(|page| page.page_seq)
        .max()
        .unwrap_or(0)
        + 1;
    let records = missing
        .iter()
        .enumerate()
        .map(|(index, (kind, payload))| {
            Ok(EncodedRecord {
                kind: *kind,
                seq: next_seq + index as u64,
                payload: encode_record_payload(*kind, payload)?,
            })
        })
        .collect::<PactResult<Vec<_>>>()?;
    let page_offset = bytes.len() as u64;
    let refs = records.iter().collect::<Vec<_>>();
    write_page(&mut bytes, PageKind::Content, next_page_seq, &refs)?;
    let footer = scanned.manifest.latest_footer.unwrap_or_default();
    write_footer(
        &mut bytes,
        footer.manifest_offset,
        footer.branch_head_offset,
        footer.hot_index_offset,
        footer.last_commit_offset,
    );
    append_bytes(path, &bytes[page_offset as usize..])?;
    Ok(records.len())
}

pub(super) fn incremental_manifest_payload(
    manifest: &PactFileManifest,
    appended_content: usize,
    appended_content_chunks: usize,
    appended_nodes: usize,
) -> Value {
    json!({
        "schema": PACT_FILE_SCHEMA,
        "version": FILE_VERSION,
        "file_kind": manifest.file_kind,
        "record_count": manifest.record_count + appended_content + appended_content_chunks + appended_nodes + 3,
        "content_record_count": manifest.content_record_count + appended_content,
        "content_chunk_count": manifest.content_chunk_count + appended_content_chunks,
        "node_count": manifest.node_count + appended_nodes,
        "commit_count": manifest.commit_count + 1,
        "head_count": manifest.head_count + 1,
        "frozen_cycle_count": manifest.frozen_cycle_count,
        "root_count": manifest.root_count,
        "branch_count": manifest.branch_count,
    })
}

pub(super) fn head_reindex_manifest_payload(manifest: &PactFileManifest) -> Value {
    json!({
        "schema": PACT_FILE_SCHEMA,
        "version": FILE_VERSION,
        "file_kind": manifest.file_kind,
        "record_count": manifest.record_count + 2,
        "content_record_count": manifest.content_record_count,
        "content_chunk_count": manifest.content_chunk_count,
        "node_count": manifest.node_count,
        "commit_count": manifest.commit_count,
        "head_count": manifest.head_count + 1,
        "frozen_cycle_count": manifest.frozen_cycle_count,
        "root_count": manifest.root_count,
        "branch_count": manifest.branch_count,
    })
}
