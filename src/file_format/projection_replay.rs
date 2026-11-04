use super::*;
use crate::error::{PactError, PactResult};
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

pub fn file_project_revision_json(path: &Path, target_revision: u64) -> PactResult<String> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_pack::is_packed_v1(&raw) {
        let container = crate::pact_pack::PackedV1::read(&raw)?;
        let projected = container.project_revision(target_revision)?;
        return serde_json::to_string(&projected).map_err(|err| {
            PactError::codec(format!(
                "failed to encode packed-v1 revision projection: {err}"
            ))
        });
    }
    let session = super::scan::PactReadSession::open(path)?;
    let validation = session.validation();
    if validation.manifest.last_valid_offset <= FILE_HEADER_LEN && !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot project invalid .pact file revision: {}",
            session.errors().join("; ")
        )));
    }
    if validation.ok {
        let projected = project_records_revision_value(session.records(), target_revision)?;
        return serde_json::to_string(&projected).map_err(|err| {
            PactError::codec(format!("failed to encode projected revision: {err}"))
        });
    }

    let projected = project_records_revision_file_wire_value(session.records(), target_revision)?;
    serde_json::to_string(&projected)
        .map_err(|err| PactError::codec(format!("failed to encode projected revision: {err}")))
}

pub fn file_project_head_json(path: &Path) -> PactResult<String> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_pack::is_packed_v1(&raw) {
        let container = crate::pact_pack::PackedV1::read(&raw)?;
        let projected = container.project_head()?;
        return serde_json::to_string(&projected).map_err(|err| {
            PactError::codec(format!("failed to encode packed-v1 head projection: {err}"))
        });
    }
    let session = super::scan::PactReadSession::open(path)?;
    let validation = session.validation();
    if validation.manifest.last_valid_offset <= FILE_HEADER_LEN && !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot project invalid .pact file head: {}",
            session.errors().join("; ")
        )));
    }
    let records = session.records();
    if let Ok(root_store) = ledger_from_records(records) {
        let target_branch = root_store.current_branch_id.clone();
        if root_store.branches.contains_key(&target_branch)
            && let Ok(root_store_json) = serde_json::to_string(&root_store)
            && let Ok(projected) = crate::store::ledger_project_branch_tree_json_with_scope(
                &root_store_json,
                &target_branch,
                None,
                ProjectionTreeScope::Head,
            )
        {
            return Ok(projected);
        }
    }
    let target_cycle = latest_projected_cycle(records);
    if validation.ok {
        let projected = project_records_revision_value(records, target_cycle)?;
        return serde_json::to_string(&projected)
            .map_err(|err| PactError::codec(format!("failed to encode projected head: {err}")));
    }

    let projected = project_records_revision_file_wire_value(records, target_cycle)?;
    serde_json::to_string(&projected)
        .map_err(|err| PactError::codec(format!("failed to encode projected head: {err}")))
}

pub(super) fn project_records_revision_value(
    records: &[ScannedRecord],
    target_cycle: u64,
) -> PactResult<Value> {
    if let Some(projected) = project_records_revision_native(records, target_cycle)? {
        return Ok(projected);
    }
    project_records_revision_file_wire_value(records, target_cycle)
}

pub(super) fn project_records_revision_file_wire_value(
    records: &[ScannedRecord],
    target_cycle: u64,
) -> PactResult<Value> {
    project_file_wire_revision_from_records(records, target_cycle)
}

fn project_records_revision_native(
    records: &[ScannedRecord],
    target_cycle: u64,
) -> PactResult<Option<Value>> {
    if has_frozen_anchor_at_or_before(records, target_cycle)? {
        return Ok(None);
    }
    let Ok(root_store) = ledger_from_records(records) else {
        return Ok(None);
    };
    let Some(branch) = root_store.branches.get(&root_store.current_branch_id) else {
        return Ok(None);
    };
    if !crate::store::branch_has_native_component_projection_at(branch, Some(target_cycle)) {
        return Ok(None);
    }
    crate::store::project_branch_component_tree(&root_store, branch, Some(target_cycle))
        .map(|tree| Some(tree.project_value()))
}

pub(super) fn has_frozen_anchor_at_or_before(
    records: &[ScannedRecord],
    target_cycle: u64,
) -> PactResult<bool> {
    for record in records
        .iter()
        .filter(|record| record.kind == "frozen_cycle")
    {
        let frozen: FrozenCycleRecord = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid frozen cycle record: {err}")))?;
        if frozen.cycle <= target_cycle {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn file_fast_open_summary_json(path: &Path) -> PactResult<String> {
    let mut file = fs::File::open(path)
        .map_err(|err| PactError::storage(format!("failed to open .pact file: {err}")))?;
    let file_len = file
        .metadata()
        .map_err(|err| PactError::storage(format!("failed to stat .pact file: {err}")))?
        .len();
    let mut bytes_read = 0_u64;
    let mut footer_bytes = vec![0_u8; FILE_FOOTER_LEN];
    let footer = if file_len >= FILE_FOOTER_LEN as u64 {
        file.seek(SeekFrom::End(-(FILE_FOOTER_LEN as i64)))
            .map_err(|err| PactError::storage(format!("failed to seek .pact footer: {err}")))?;
        file.read_exact(&mut footer_bytes)
            .map_err(|err| PactError::storage(format!("failed to read .pact footer: {err}")))?;
        bytes_read += FILE_FOOTER_LEN as u64;
        read_footer_at(&footer_bytes, 0)
    } else {
        None
    };
    let (fallback_used, footer) = match footer {
        Some(mut footer) => {
            footer.offset = file_len - FILE_FOOTER_LEN as u64;
            (false, Some(footer))
        }
        None => {
            let raw = fs::read(path)
                .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
            bytes_read += raw.len() as u64;

            let bytes = if crate::pact_pack::is_packed_v1(&raw) {
                packed_v1_to_v0_bytes(&raw)?
            } else {
                raw
            };
            (true, latest_valid_footer(&bytes))
        }
    };
    serde_json::to_string_pretty(&json!({
        "schema": "pact.fast_open.v1",
        "file_bytes": file_len,
        "bytes_read": bytes_read,
        "fallback_used": fallback_used,
        "footer": footer,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode fast open summary: {err}")))
}

pub fn file_open_head_fast_json(path: &Path) -> PactResult<String> {
    let mut file = fs::File::open(path)
        .map_err(|err| PactError::storage(format!("failed to open .pact file: {err}")))?;
    let file_len = file
        .metadata()
        .map_err(|err| PactError::storage(format!("failed to stat .pact file: {err}")))?
        .len();
    let mut bytes_read = 0_u64;
    let footer = read_footer_from_end(&mut file, file_len, &mut bytes_read)?;
    let Some(footer) = footer else {
        let raw = fs::read(path)
            .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;

        let bytes = if crate::pact_pack::is_packed_v1(&raw) {
            packed_v1_to_v0_bytes(&raw)?
        } else {
            raw
        };
        let scanned = scan_file(&bytes, true);
        let manifest = scanned
            .records
            .iter()
            .find(|record| record.kind == "meta")
            .map(|record| record.payload.clone());
        let current_branch_id = manifest
            .as_ref()
            .and_then(|payload| payload.get("current_branch_id"))
            .and_then(Value::as_str);
        let branch_head = scanned
            .records
            .iter()
            .rev()
            .find(|record| {
                record.kind == "branch"
                    && current_branch_id
                        .map(|branch_id| {
                            record.payload.get("branch_id").and_then(Value::as_str)
                                == Some(branch_id)
                        })
                        .unwrap_or(true)
            })
            .map(|record| record.payload.clone());
        let mut head = scanned
            .records
            .iter()
            .rev()
            .find(|record| {
                record.kind == "head"
                    && current_branch_id
                        .map(|branch_id| {
                            record.payload.get("branch_id").and_then(Value::as_str)
                                == Some(branch_id)
                        })
                        .unwrap_or(true)
            })
            .map(|record| record.payload.clone());
        if head.is_none() {
            head = branch_head.clone();
        }
        return serde_json::to_string_pretty(&json!({
            "schema": "pact.fast_head.v1",
            "fallback_used": true,
            "bytes_read": bytes_read + bytes.len() as u64,
            "manifest": manifest,
            "branch_head": branch_head,
            "head": head,
        }))
        .map_err(|err| PactError::codec(format!("failed to encode fast head fallback: {err}")));
    };
    let symbols = read_symbol_table_prefix_from_file(&mut file, &mut bytes_read)?;
    let manifest_records =
        read_page_records_from_file(&mut file, footer.manifest_offset, &mut bytes_read, &symbols)?;
    let branch_records = read_page_records_matching_from_file(
        &mut file,
        footer.branch_head_offset,
        &mut bytes_read,
        &symbols,
        &[RecordKind::Branch, RecordKind::Head, RecordKind::Commit],
    )?;
    let manifest = manifest_records
        .iter()
        .find(|record| record.kind == "meta")
        .map(|record| record.payload.clone());
    let current_branch_id = manifest
        .as_ref()
        .and_then(|payload| payload.get("current_branch_id"))
        .and_then(Value::as_str);
    let latest_commit = branch_records
        .iter()
        .rev()
        .find(|record| record.kind == "commit")
        .map(|record| record.payload.clone());
    let branch = branch_records
        .iter()
        .rev()
        .find(|record| {
            record.kind == "branch"
                && current_branch_id
                    .map(|branch_id| {
                        record.payload.get("branch_id").and_then(Value::as_str) == Some(branch_id)
                    })
                    .unwrap_or(true)
        })
        .map(|record| record.payload.clone());
    let head = branch_records
        .iter()
        .rev()
        .find(|record| {
            record.kind == "head"
                && current_branch_id
                    .map(|branch_id| {
                        record.payload.get("branch_id").and_then(Value::as_str) == Some(branch_id)
                    })
                    .unwrap_or(true)
        })
        .map(|record| record.payload.clone());
    let effective_head = head.or_else(|| branch.clone());
    let branch_head_source = effective_head.as_ref();
    let branch_head = branch_head_source.map(|head| {
        json!({
            "branch_id": head.get("branch_id").and_then(Value::as_str),
            "head_revision": head.get("head_revision").and_then(Value::as_u64),
            "head_cycle": head.get("head_cycle").and_then(Value::as_u64),
        })
    });
    serde_json::to_string_pretty(&json!({
        "schema": "pact.fast_head.v1",
        "fallback_used": false,
        "bytes_read": bytes_read,
        "footer": footer,
        "manifest": manifest,
        "branch_head": branch_head,
        "head": effective_head,
        "latest_commit": latest_commit,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode fast head: {err}")))
}

pub fn file_batch_open_head_fast_json<P: AsRef<Path>>(paths: &[P]) -> PactResult<String> {
    let entries = paths
        .iter()
        .map(|path| {
            let path = path.as_ref();
            let path_display = path.display().to_string();
            match file_open_head_fast_json(path) {
                Ok(head_json) => match serde_json::from_str::<Value>(&head_json) {
                    Ok(head) => json!({
                        "ok": true,
                        "path": path_display,
                        "head": head,
                    }),
                    Err(err) => json!({
                        "ok": false,
                        "path": path_display,
                        "error": format!("failed to decode fast head JSON: {err}"),
                    }),
                },
                Err(err) => json!({
                    "ok": false,
                    "path": path_display,
                    "error": err.to_string(),
                }),
            }
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&entries)
        .map_err(|err| PactError::codec(format!("failed to encode fast-head batch: {err}")))
}

pub(super) fn latest_projected_cycle(records: &[ScannedRecord]) -> u64 {
    records
        .iter()
        .filter_map(|record| match record.kind.as_str() {
            "commit" => serde_json::from_value::<CommitRecord>(record.payload.clone())
                .ok()
                .map(|commit| commit.cycle),
            "frozen_cycle" => serde_json::from_value::<FrozenCycleRecord>(record.payload.clone())
                .ok()
                .map(|frozen| frozen.cycle),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn project_file_wire_revision_from_records(
    records: &[ScannedRecord],
    target_cycle: u64,
) -> PactResult<Value> {
    let content = content_texts_from_records(records);
    let mut frozen: Option<FrozenCycleRecord> = None;
    for record in records
        .iter()
        .filter(|record| record.kind == "frozen_cycle")
    {
        let candidate: FrozenCycleRecord = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid frozen cycle record: {err}")))?;
        if candidate.cycle <= target_cycle
            && frozen
                .as_ref()
                .is_none_or(|existing| candidate.cycle > existing.cycle)
        {
            frozen = Some(candidate);
        }
    }

    let (mut tree, frozen_cycle) = if let Some(frozen) = frozen {
        let root_json = content.get(&frozen.root_ref).ok_or_else(|| {
            PactError::not_found(format!("missing frozen root blob: {}", frozen.root_ref))
        })?;
        let tree: Value = serde_json::from_str(root_json)
            .map_err(|err| PactError::codec(format!("invalid frozen root JSON: {err}")))?;
        (tree, frozen.cycle)
    } else {
        let root_store = ledger_from_records_with_commits(records, false)?;
        let branch = root_store
            .branches
            .get(&root_store.current_branch_id)
            .ok_or_else(|| {
                PactError::not_found(format!(
                    "missing current branch: {}",
                    root_store.current_branch_id
                ))
            })?;
        let tree = if branch.base_ref.content_ref.is_none() && branch.current.component_count() > 0
        {
            crate::store::project_branch_component_tree(&root_store, branch, None)?.project_value()
        } else {
            project_branch_file_wire_base_value(&root_store, branch)?
        };
        (tree, 0)
    };

    let nodes = node_records_by_id(records);
    let mut commits = Vec::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let commit: CommitRecord = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid commit record: {err}")))?;
        if commit.cycle > frozen_cycle && commit.cycle <= target_cycle {
            commits.push(commit);
        }
    }
    commits.sort_by_key(|commit| (commit.cycle, commit.revision));
    for commit in commits {
        apply_commit_record(&mut tree, &commit, &nodes, &content)?;
    }
    canonical_component_projection_value(tree)
}

fn project_file_wire_branch_cycle_value(
    records: &[ScannedRecord],
    branch_id: &str,
    target_cycle: u64,
) -> PactResult<Value> {
    let content = content_texts_from_records(records);
    let nodes = node_records_by_id(records);
    let root_store = ledger_from_records_with_commits(records, false)?;
    let mut visited = BTreeSet::new();
    let mut tree =
        project_branch_base_for_cycle(records, &root_store, branch_id, target_cycle, &mut visited)?;

    let mut commits = branch_commits_to_cycle(records, branch_id, target_cycle)?;
    commits.sort_by_key(|commit| (commit.cycle, commit.revision));
    for commit in commits {
        apply_commit_record(&mut tree, &commit, &nodes, &content)?;
    }
    canonical_component_projection_value(tree)
}

fn canonical_component_projection_value(value: Value) -> PactResult<Value> {
    crate::tree::ComponentTree::from_value(&value).map(|tree| tree.project_value())
}

fn project_branch_base_for_cycle(
    records: &[ScannedRecord],
    root_store: &ContextTreeLedger,
    branch_id: &str,
    target_cycle: u64,
    visited: &mut BTreeSet<String>,
) -> PactResult<Value> {
    if !visited.insert(branch_id.to_string()) {
        return Err(PactError::corrupt(format!(
            "branch lineage cycle while projecting {branch_id:?}"
        )));
    }

    let branch = root_store.branches.get(branch_id);
    let base = if let Some(branch) = branch {
        if let Some(parent_branch_id) = branch.parent_branch_id.as_deref() {
            let fork_cycle = branch
                .fork_ref
                .as_ref()
                .and_then(|fork_ref| fork_ref.revision)
                .unwrap_or(target_cycle)
                .min(target_cycle);
            let mut parent_tree = project_branch_base_for_cycle(
                records,
                root_store,
                parent_branch_id,
                fork_cycle,
                visited,
            )?;
            let content = content_texts_from_records(records);
            let nodes = node_records_by_id(records);
            let mut parent_commits =
                branch_commits_to_cycle(records, parent_branch_id, fork_cycle)?;
            parent_commits.sort_by_key(|commit| (commit.cycle, commit.revision));
            for commit in parent_commits {
                apply_commit_record(&mut parent_tree, &commit, &nodes, &content)?;
            }
            parent_tree
        } else {
            project_branch_file_wire_base_value(root_store, branch)?
        }
    } else {
        let fallback_branch = root_store
            .branches
            .get(&root_store.current_branch_id)
            .ok_or_else(|| {
                PactError::not_found(format!(
                    "missing current branch: {}",
                    root_store.current_branch_id
                ))
            })?;
        project_branch_file_wire_base_value(root_store, fallback_branch)?
    };

    visited.remove(branch_id);
    Ok(base)
}

fn project_branch_file_wire_base_value(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
) -> PactResult<Value> {
    if branch.base_ref.content_ref.is_none()
        && branch.current.component_count() > 0
        && crate::store::branch_has_native_component_projection_at(branch, Some(0))
    {
        return Ok(
            crate::store::project_branch_component_tree(root_store, branch, Some(0))?
                .project_value(),
        );
    }
    crate::store::project_branch_tree_value(root_store, branch, Some(0))
}

fn branch_commits_to_cycle(
    records: &[ScannedRecord],
    branch_id: &str,
    target_cycle: u64,
) -> PactResult<Vec<CommitRecord>> {
    let mut commits = Vec::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let commit: CommitRecord = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid commit record: {err}")))?;
        if commit.branch_id == branch_id && commit.cycle <= target_cycle {
            commits.push(commit);
        }
    }
    Ok(commits)
}

pub(super) fn transaction_traces_from_commit_records_from(
    records: &[ScannedRecord],
    branch_id: &str,
    start_cycle: u64,
    target_cycle: u64,
) -> PactResult<Vec<TransactionTrace>> {
    let mut cycles = records
        .iter()
        .filter(|record| record.kind == "commit")
        .filter_map(|record| serde_json::from_value::<CommitRecord>(record.payload.clone()).ok())
        .filter(|commit| {
            commit.branch_id == branch_id
                && commit.cycle > start_cycle
                && commit.cycle <= target_cycle
        })
        .map(|commit| commit.cycle)
        .collect::<Vec<_>>();
    cycles.sort_unstable();
    cycles.dedup();

    let mut traces = Vec::new();
    let mut from_cycle = start_cycle;
    let mut before = commit_trace_base_from_records(records, branch_id, from_cycle)?;
    let nodes = node_records_by_id(records);
    let content = content_texts_from_records(records);
    for cycle in cycles {
        if cycle <= from_cycle {
            continue;
        }
        let mut ops = Vec::new();
        for commit in branch_commits_at_cycle(records, branch_id, cycle)? {
            append_commit_record_trace_ops(&before, &commit, &nodes, &content, &mut ops)?;
        }
        let trace = TransactionTrace {
            from_rev: from_cycle,
            to_rev: cycle,
            ops,
        };
        if !trace.is_empty() {
            before = trace.replay_owned(before)?;
            traces.push(trace);
        } else {
            before = project_file_wire_branch_cycle_value(records, branch_id, cycle)?;
        }
        from_cycle = cycle;
    }
    Ok(traces)
}

pub(super) fn commit_trace_base_from_records(
    records: &[ScannedRecord],
    branch_id: &str,
    start_cycle: u64,
) -> PactResult<Value> {
    let root_store = ledger_from_records_with_commits(records, false)?;
    if let Some(branch) = root_store.branches.get(branch_id)
        && branch.base_ref.content_ref.is_none()
        && branch.current.component_count() > 0
        && (start_cycle == 0 || start_cycle == branch.head_revision)
    {
        return Ok(
            crate::store::project_branch_component_tree(&root_store, branch, None)?.project_value(),
        );
    }
    project_file_wire_branch_cycle_value(records, branch_id, start_cycle)
}

fn branch_commits_at_cycle(
    records: &[ScannedRecord],
    branch_id: &str,
    cycle: u64,
) -> PactResult<Vec<CommitRecord>> {
    let mut commits = Vec::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let commit: CommitRecord = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid commit record: {err}")))?;
        if commit.branch_id == branch_id && commit.cycle == cycle {
            commits.push(commit);
        }
    }
    commits.sort_by_key(|commit| commit.revision);
    Ok(commits)
}

fn append_commit_record_trace_ops(
    before: &Value,
    commit: &CommitRecord,
    nodes: &BTreeMap<String, Value>,
    content: &BTreeMap<String, String>,
    ops: &mut Vec<TraceOp>,
) -> PactResult<()> {
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent {
                parent_id,
                node_ref,
                position,
            } => {
                let mut node = nodes.get(node_ref).cloned().ok_or_else(|| {
                    PactError::not_found(format!("missing node ref for insert: {node_ref}"))
                })?;
                if let Some(position) = position
                    && let Some(object) = node.as_object_mut()
                {
                    object.insert("offset".to_string(), json!(position));
                }
                append_insert_trace_ops_from_value(
                    parent_id,
                    position_to_index(*position),
                    node,
                    ops,
                )?;
            }
            CommitRecordOp::ReplaceComponent {
                component_id,
                node_ref,
            } => {
                let node = nodes.get(node_ref).cloned().ok_or_else(|| {
                    PactError::not_found(format!("missing node ref for replace: {node_ref}"))
                })?;
                if trace_value_node_id(&node).as_deref() == Some(component_id.as_str()) {
                    append_update_trace_ops_from_value(component_id, node, ops)?;
                    continue;
                }
                let (parent_id, index) =
                    value_parent_and_index(before, component_id).ok_or_else(|| {
                        PactError::not_found(format!(
                            "missing existing node for replace trace conversion: {component_id}"
                        ))
                    })?;
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::DeleteComponent,
                ));
                append_insert_trace_ops_from_value(&parent_id, index, node, ops)?;
            }
            CommitRecordOp::DeleteComponent { component_id } => {
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::DeleteComponent,
                ));
            }
            CommitRecordOp::AppendComponents {
                parent_id,
                node_refs,
            } => {
                for node_ref in node_refs {
                    let node = nodes.get(node_ref).cloned().ok_or_else(|| {
                        PactError::not_found(format!("missing node ref for append: {node_ref}"))
                    })?;
                    append_insert_trace_ops_from_value(parent_id, usize::MAX, node, ops)?;
                }
            }
            CommitRecordOp::SetComponentAux {
                component_id,
                key_symbol,
                value_ref,
            } => {
                let value_json = content.get(value_ref).ok_or_else(|| {
                    PactError::not_found(format!("missing attr value blob: {value_ref}"))
                })?;
                let value: Value =
                    serde_json::from_str(value_json).unwrap_or_else(|_| json!(value_json));
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::SetAttr {
                        key: key_symbol.clone(),
                        value,
                    },
                ));
            }
            CommitRecordOp::UnsetComponentAux {
                component_id,
                key_symbol,
            } => {
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::UnsetAttr {
                        key: key_symbol.clone(),
                    },
                ));
            }
            CommitRecordOp::AttachContentRef {
                component_id,
                blob_id,
            } => {
                let text = content.get(blob_id).ok_or_else(|| {
                    PactError::not_found(format!("missing content blob: {blob_id}"))
                })?;
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::SetContent {
                        content: Some(text.clone()),
                        content_ref: None,
                        content_preview: None,
                    },
                ));
            }
            CommitRecordOp::ReplaceContentRange {
                component_id,
                range,
                blob_id,
            } => {
                let replacement = content.get(blob_id).ok_or_else(|| {
                    PactError::not_found(format!("missing range replacement blob: {blob_id}"))
                })?;
                let start = usize::try_from(range.start).map_err(|_| {
                    PactError::invalid(format!("content range start too large: {}", range.start))
                })?;
                let end = usize::try_from(range.end).map_err(|_| {
                    PactError::invalid(format!("content range end too large: {}", range.end))
                })?;
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::ReplaceContentRange {
                        start,
                        end,
                        replacement: replacement.clone(),
                    },
                ));
            }
        }
    }
    Ok(())
}

fn append_update_trace_ops_from_value(
    component_id: &str,
    node: Value,
    ops: &mut Vec<TraceOp>,
) -> PactResult<()> {
    let object = node
        .as_object()
        .ok_or_else(|| PactError::invalid("replace trace conversion requires object node"))?;
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    let content_ref = object.get("content_ref").cloned();
    let content_preview = object
        .get("content_preview")
        .and_then(Value::as_str)
        .map(str::to_string);
    if content.is_some() || content_ref.is_some() || content_preview.is_some() {
        ops.push(TraceOp::new(
            component_id.to_string(),
            TraceOpKind::SetContent {
                content,
                content_ref,
                content_preview,
            },
        ));
    }
    for (key, value) in object {
        if matches!(
            key.as_str(),
            "id" | "children" | "content" | "content_ref" | "content_preview"
        ) {
            continue;
        }
        ops.push(TraceOp::new(
            component_id.to_string(),
            TraceOpKind::SetField {
                key: key.clone(),
                value: value.clone(),
            },
        ));
    }
    Ok(())
}

fn append_insert_trace_ops_from_value(
    parent_id: &str,
    index: usize,
    node: Value,
    ops: &mut Vec<TraceOp>,
) -> PactResult<()> {
    let component_id = trace_value_node_id(&node)
        .ok_or_else(|| PactError::invalid("insert trace conversion requires node id"))?;
    let children = node
        .get("children")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut payload = node;
    if let Some(object) = payload.as_object_mut() {
        object.insert("children".to_string(), Value::Array(Vec::new()));
    }
    ops.push(TraceOp::new(
        component_id.clone(),
        TraceOpKind::InsertComponent {
            parent_id: parent_id.to_string(),
            index,
            component: payload,
        },
    ));
    for (child_index, child) in children.into_iter().enumerate() {
        append_insert_trace_ops_from_value(&component_id, child_index, child, ops)?;
    }
    Ok(())
}

fn position_to_index(position: Option<i64>) -> usize {
    position
        .and_then(|position| usize::try_from(position).ok())
        .unwrap_or(usize::MAX)
}

fn value_parent_and_index(tree: &Value, component_id: &str) -> Option<(String, usize)> {
    let children = tree.get("children").and_then(Value::as_array)?;
    for (index, child) in children.iter().enumerate() {
        if child.get("id").and_then(Value::as_str) == Some(component_id) {
            return trace_value_node_id(tree).map(|parent_id| (parent_id, index));
        }
        if let Some(found) = value_parent_and_index(child, component_id) {
            return Some(found);
        }
    }
    None
}

fn trace_value_node_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(super) fn apply_commit_record(
    tree: &mut Value,
    commit: &CommitRecord,
    nodes: &BTreeMap<String, Value>,
    content: &BTreeMap<String, String>,
) -> PactResult<()> {
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent {
                parent_id,
                node_ref,
                position,
            } => {
                let mut node = nodes.get(node_ref).cloned().ok_or_else(|| {
                    PactError::not_found(format!("missing node ref for insert: {node_ref}"))
                })?;
                hydrate_value_node_content_refs(&mut node, content);
                if let Some(position) = position
                    && let Some(object) = node.as_object_mut()
                {
                    object.insert("offset".to_string(), json!(position));
                }
                insert_commit_node(tree, parent_id, node)?;
            }
            CommitRecordOp::ReplaceComponent {
                component_id,
                node_ref,
            } => {
                let mut node = nodes.get(node_ref).cloned().ok_or_else(|| {
                    PactError::not_found(format!("missing node ref for replace: {node_ref}"))
                })?;
                hydrate_value_node_content_refs(&mut node, content);
                replace_or_insert_commit_node(tree, component_id, node)?;
            }
            CommitRecordOp::DeleteComponent { component_id } => {
                remove_commit_node(tree, component_id)?;
            }
            CommitRecordOp::AppendComponents {
                parent_id,
                node_refs,
            } => {
                for node_ref in node_refs {
                    let mut node = nodes.get(node_ref).cloned().ok_or_else(|| {
                        PactError::not_found(format!("missing node ref for append: {node_ref}"))
                    })?;
                    hydrate_value_node_content_refs(&mut node, content);
                    insert_commit_node(tree, parent_id, node)?;
                }
            }
            CommitRecordOp::SetComponentAux {
                component_id,
                key_symbol,
                value_ref,
            } => {
                let value_json = content.get(value_ref).ok_or_else(|| {
                    PactError::not_found(format!("missing attr value blob: {value_ref}"))
                })?;
                let value: Value =
                    serde_json::from_str(value_json).unwrap_or_else(|_| json!(value_json));
                set_component_aux(tree, component_id, key_symbol, value)?;
            }
            CommitRecordOp::UnsetComponentAux {
                component_id,
                key_symbol,
            } => {
                let _ = unset_component_aux(tree, component_id, key_symbol);
            }
            CommitRecordOp::AttachContentRef {
                component_id,
                blob_id,
            } => {
                let text = content
                    .get(blob_id)
                    .ok_or_else(|| {
                        PactError::not_found(format!("missing content blob: {blob_id}"))
                    })?
                    .clone();
                set_node_content(tree, component_id, text)?;
            }
            CommitRecordOp::ReplaceContentRange {
                component_id,
                range,
                blob_id,
            } => {
                let replacement = content.get(blob_id).ok_or_else(|| {
                    PactError::not_found(format!("missing range replacement blob: {blob_id}"))
                })?;
                replace_content_range(tree, component_id, range, replacement)?;
            }
        }
    }
    Ok(())
}

fn insert_commit_node(tree: &mut Value, parent_id: &str, mut node: Value) -> PactResult<()> {
    let component_id = node.get("id").and_then(Value::as_str).map(str::to_string);
    if let Some(component_id) = component_id.as_deref()
        && !parent_has_direct_child(tree, parent_id, component_id)
    {
        remove_commit_node_if_present(tree, component_id);
    }

    if let Some(object) = node.as_object_mut() {
        object
            .entry("parent_id".to_string())
            .or_insert_with(|| Value::String(parent_id.to_string()));
    }

    let parent = find_node_mut(tree, parent_id)
        .ok_or_else(|| PactError::not_found(format!("parent node not found: {parent_id}")))?;
    let parent_object = parent
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt(format!("parent node {parent_id} is not an object")))?;
    let children = parent_object
        .entry("children".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            PactError::corrupt(format!("parent node {parent_id} children is not an array"))
        })?;

    if let Some(component_id) = component_id.as_deref()
        && let Some(existing) = children
            .iter()
            .position(|child| child.get("id").and_then(Value::as_str) == Some(component_id))
    {
        children[existing] = node;
        sort_commit_children(children);
        return Ok(());
    }

    children.push(node);
    sort_commit_children(children);
    Ok(())
}

fn replace_or_insert_commit_node(
    tree: &mut Value,
    component_id: &str,
    replacement: Value,
) -> PactResult<()> {
    match replace_commit_node(tree, component_id, replacement.clone()) {
        Ok(()) => Ok(()),
        Err(PactError::NotFound(message)) => {
            let parent_id = replacement
                .get("parent_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or(PactError::NotFound(message))?;
            insert_commit_node(tree, &parent_id, replacement)
        }
        Err(err) => Err(err),
    }
}

fn replace_commit_node(
    tree: &mut Value,
    component_id: &str,
    mut replacement: Value,
) -> PactResult<()> {
    let node = find_node_mut(tree, component_id)
        .ok_or_else(|| PactError::not_found(format!("node not found: {component_id}")))?;
    let object = node
        .as_object()
        .ok_or_else(|| PactError::corrupt(format!("node {component_id} is not an object")))?;
    let prior_parent_id = object.get("parent_id").cloned();
    let prior_offset = object.get("offset").cloned();
    let prior_depth = object.get("pact_depth").cloned();
    let prior_depth_alias = object.get("depth").cloned();
    let prior_children = object.get("children").cloned();

    let replacement_object = replacement.as_object_mut().ok_or_else(|| {
        PactError::corrupt(format!("replacement node {component_id} is not an object"))
    })?;
    if let Some(parent_id) = prior_parent_id {
        replacement_object.insert("parent_id".to_string(), parent_id);
    }
    if let Some(offset) = prior_offset {
        replacement_object.insert("offset".to_string(), offset);
    }
    if let Some(depth) = prior_depth {
        replacement_object.insert("pact_depth".to_string(), depth);
    }
    if let Some(depth) = prior_depth_alias {
        replacement_object
            .entry("depth".to_string())
            .or_insert(depth);
    }
    if let Some(children) = prior_children {
        replacement_object
            .entry("children".to_string())
            .or_insert(children);
    }
    *node = replacement;
    Ok(())
}

fn set_node_content(tree: &mut Value, component_id: &str, content: String) -> PactResult<()> {
    let node = find_node_mut(tree, component_id)
        .ok_or_else(|| PactError::not_found(format!("node not found: {component_id}")))?;
    let object = node
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt(format!("node {component_id} is not an object")))?;
    object.insert("content".to_string(), Value::String(content));
    object.remove("content_ref");
    object.remove("content_preview");
    Ok(())
}

fn parent_has_direct_child(tree: &Value, parent_id: &str, child_id: &str) -> bool {
    let Some(parent) = find_node(tree, parent_id) else {
        return false;
    };
    parent
        .get("children")
        .and_then(Value::as_array)
        .is_some_and(|children| {
            children
                .iter()
                .any(|child| child.get("id").and_then(Value::as_str) == Some(child_id))
        })
}

fn find_node<'a>(node: &'a Value, component_id: &str) -> Option<&'a Value> {
    if node.get("id").and_then(Value::as_str) == Some(component_id) {
        return Some(node);
    }
    node.get("children")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|child| find_node(child, component_id))
}

fn find_node_mut<'a>(node: &'a mut Value, component_id: &str) -> Option<&'a mut Value> {
    if node.get("id").and_then(Value::as_str) == Some(component_id) {
        return Some(node);
    }
    let children = node.get_mut("children")?.as_array_mut()?;
    for child in children {
        if let Some(found) = find_node_mut(child, component_id) {
            return Some(found);
        }
    }
    None
}

fn remove_commit_node(tree: &mut Value, component_id: &str) -> PactResult<()> {
    let paired_tool_call_id = find_node(tree, component_id).and_then(tool_call_id_from_node);
    let removed = if let Some(tool_call_id) = paired_tool_call_id.as_deref() {
        remove_nodes_with_tool_call_id(tree, tool_call_id)
    } else {
        remove_commit_node_if_present(tree, component_id)
    };
    if removed {
        Ok(())
    } else {
        Err(PactError::not_found(format!(
            "node not found: {component_id}"
        )))
    }
}

fn remove_nodes_with_tool_call_id(node: &mut Value, tool_call_id: &str) -> bool {
    let mut removed = false;
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        let before = children.len();
        children.retain(|child| tool_call_id_from_node(child).as_deref() != Some(tool_call_id));
        removed |= children.len() != before;
        for child in children {
            removed |= remove_nodes_with_tool_call_id(child, tool_call_id);
        }
    }
    removed
}

fn tool_call_id_from_node(node: &Value) -> Option<String> {
    node.get("tool_call_id")
        .or_else(|| node.get("aux").and_then(|aux| aux.get("tool_call_id")))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn remove_commit_node_if_present(node: &mut Value, component_id: &str) -> bool {
    let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) else {
        return false;
    };
    if let Some(index) = children
        .iter()
        .position(|child| child.get("id").and_then(Value::as_str) == Some(component_id))
    {
        children.remove(index);
        return true;
    }
    children
        .iter_mut()
        .any(|child| remove_commit_node_if_present(child, component_id))
}

fn sort_commit_children(children: &mut [Value]) {
    children.sort_by(|left, right| {
        (
            numeric_field(left, "offset"),
            numeric_field(left, "created_at_ns"),
            numeric_field(left, "creation_index"),
            string_field(left, "id"),
        )
            .cmp(&(
                numeric_field(right, "offset"),
                numeric_field(right, "created_at_ns"),
                numeric_field(right, "creation_index"),
                string_field(right, "id"),
            ))
    });
}

fn numeric_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn string_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn hydrate_value_node_content_refs(value: &mut Value, content: &BTreeMap<String, String>) {
    match value {
        Value::Object(object) => {
            if let Some(content_ref) = ref_id_from_content_ref_value(object.get("content_ref"))
                && let Some(text) = content.get(&content_ref)
            {
                object.insert("content".to_string(), Value::String(text.clone()));
                object.remove("content_ref");
                object.remove("content_preview");
            }
            if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
                for child in children {
                    hydrate_value_node_content_refs(child, content);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                hydrate_value_node_content_refs(item, content);
            }
        }
        _ => {}
    }
}

fn set_component_aux(
    tree: &mut Value,
    component_id: &str,
    key: &str,
    value: Value,
) -> PactResult<()> {
    if tree.get("id").and_then(Value::as_str) == Some(component_id) {
        let object = tree
            .as_object_mut()
            .ok_or_else(|| PactError::corrupt(format!("node {component_id} is not an object")))?;
        let aux = object.entry("aux").or_insert_with(|| json!({}));
        let aux = aux.as_object_mut().ok_or_else(|| {
            PactError::corrupt(format!("node {component_id} aux is not an object"))
        })?;
        aux.insert(key.to_string(), value);
        return Ok(());
    }
    if let Some(children) = tree.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            if set_component_aux(child, component_id, key, value.clone()).is_ok() {
                return Ok(());
            }
        }
    }
    Err(PactError::not_found(format!(
        "component not found for aux update: {component_id}"
    )))
}

fn unset_component_aux(tree: &mut Value, component_id: &str, key: &str) -> PactResult<()> {
    if tree.get("id").and_then(Value::as_str) == Some(component_id) {
        let object = tree
            .as_object_mut()
            .ok_or_else(|| PactError::corrupt(format!("node {component_id} is not an object")))?;
        if let Some(aux) = object.get_mut("aux").and_then(Value::as_object_mut) {
            aux.remove(key);
        }
        return Ok(());
    }
    if let Some(children) = tree.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            if unset_component_aux(child, component_id, key).is_ok() {
                return Ok(());
            }
        }
    }
    Err(PactError::not_found(format!(
        "component not found for aux unset: {component_id}"
    )))
}

fn replace_content_range(
    tree: &mut Value,
    component_id: &str,
    range: &PactFileRange,
    replacement: &str,
) -> PactResult<()> {
    if tree.get("id").and_then(Value::as_str) == Some(component_id) {
        let object = tree
            .as_object_mut()
            .ok_or_else(|| PactError::corrupt(format!("node {component_id} is not an object")))?;
        let content = object
            .entry("content")
            .or_insert_with(|| Value::String(String::new()));
        let text = content.as_str().ok_or_else(|| {
            PactError::corrupt(format!("node {component_id} content is not a string"))
        })?;
        let start = range.start as usize;
        let end = range.end as usize;
        if start > end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            return Err(PactError::invalid(format!(
                "invalid content range {start}..{end} for node {component_id}"
            )));
        }
        let mut updated = String::new();
        updated.push_str(&text[..start]);
        updated.push_str(replacement);
        updated.push_str(&text[end..]);
        *content = Value::String(updated);
        return Ok(());
    }
    if let Some(children) = tree.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            if replace_content_range(child, component_id, range, replacement).is_ok() {
                return Ok(());
            }
        }
    }
    Err(PactError::not_found(format!(
        "node not found for content range update: {component_id}"
    )))
}
