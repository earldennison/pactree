use crate::error::{PactError, PactResult};
use crate::file_format::{CommitRecord, CommitRecordOp};
use crate::mutation::{MutationKind, MutationRequest, OperationReport};
use crate::store::{ContentStore, PactRef};
use crate::trace::{BoundaryKind, RejectedOperation};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const DELTA_BLOB_PREVIEW_CHARS: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct IngestOutcome {
    pub accepted: usize,
    pub deduped: usize,
    pub rejected: Vec<RejectedOperation>,
}

pub(super) fn boundary_label(b: BoundaryKind) -> &'static str {
    match b {
        BoundaryKind::Authored => "authored",
        BoundaryKind::PreRequest => "Preflight",
        BoundaryKind::PostRequestAlteration => "post_request_alteration",
        BoundaryKind::Request => "request",
        BoundaryKind::Response => "response",
        BoundaryKind::PostResponseAlteration => "post_response_alteration",
        BoundaryKind::PostResponse => "PostResponse",
        BoundaryKind::Adhoc => "adhoc",
    }
}

pub(super) fn monotonic_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

pub(super) fn operation_report_refs(
    report: &OperationReport,
    root_id: &str,
    branch_id: &str,
) -> Vec<PactRef> {
    let mut out = Vec::new();
    for id in &report.inserted_ids {
        out.push(PactRef {
            root_id: root_id.to_string(),
            branch_id: branch_id.to_string(),
            revision: None,
            transaction_id: None,
            component_id: Some(id.clone()),
            content_ref: None,
        });
    }
    for id in &report.replaced_ids {
        out.push(PactRef {
            root_id: root_id.to_string(),
            branch_id: branch_id.to_string(),
            revision: None,
            transaction_id: None,
            component_id: Some(id.clone()),
            content_ref: None,
        });
    }
    out
}

pub(super) fn commit_record_from_mutations(
    branch_id: &str,
    revision: u64,
    mutations: &[MutationRequest],
    committed_tree: &Value,
    content: &mut ContentStore,
) -> PactResult<(CommitRecord, Vec<Value>)> {
    let mut ops = Vec::new();
    let mut nodes = Vec::new();
    for request in mutations {
        let is_append_update =
            request.kind == MutationKind::Update && request.mode.as_deref() == Some("append");
        let is_node_replace =
            request.kind == MutationKind::Update && !is_append_update && request.node.is_some();
        if request.kind == MutationKind::Insert || is_append_update {
            let node = request.node.as_ref().ok_or_else(|| {
                PactError::invalid("commit record insert requires a node payload".to_string())
            })?;
            let parent_id = if is_append_update {
                request
                    .target
                    .component_id
                    .clone()
                    .or_else(|| request.target.parent_id.clone())
                    .or_else(|| node.parent_id.clone())
            } else {
                request
                    .target
                    .parent_id
                    .clone()
                    .or_else(|| node.parent_id.clone())
            }
            .ok_or_else(|| {
                PactError::invalid("commit record insert requires a parent id".to_string())
            })?;
            let node_value = serde_json::to_value(node).map_err(|err| {
                PactError::codec(format!("failed to encode inserted node: {err}"))
            })?;
            nodes.push(node_value);
            ops.push(CommitRecordOp::InsertComponent {
                parent_id,
                node_ref: node.id.clone(),
                position: Some(node.offset as i64),
            });
        } else if is_node_replace {
            let target_id = request.target.component_id.clone().ok_or_else(|| {
                PactError::invalid("commit record replace requires a target node id".to_string())
            })?;

            let node_value = match find_node_value(committed_tree, &target_id) {
                Some(found) => found.clone(),
                None => {
                    let node = request.node.as_ref().ok_or_else(|| {
                        PactError::invalid(
                            "commit record replace requires a node payload".to_string(),
                        )
                    })?;
                    let mut value = serde_json::to_value(node).map_err(|err| {
                        PactError::codec(format!("failed to encode replacement node: {err}"))
                    })?;
                    if let Some(object) = value.as_object_mut() {
                        object.insert("id".to_string(), Value::String(target_id.clone()));
                    }
                    value
                }
            };
            nodes.push(node_value);
            ops.push(CommitRecordOp::ReplaceComponent {
                component_id: target_id.clone(),
                node_ref: target_id,
            });
        } else if request.kind == MutationKind::Delete {
            let component_id = request.target.component_id.clone().ok_or_else(|| {
                PactError::invalid("commit record delete requires a target node id".to_string())
            })?;
            ops.push(CommitRecordOp::DeleteComponent { component_id });
        } else if request.kind == MutationKind::Update {
            let component_id = request.target.component_id.clone().ok_or_else(|| {
                PactError::invalid("commit record content update requires a target node id")
            })?;

            if let Some(delta_ops) = update_deltas_from_request(&component_id, request, content) {
                ops.extend(delta_ops);
            } else {
                let node = find_node_value(committed_tree, &component_id).ok_or_else(|| {
                    PactError::not_found(format!(
                        "updated node not found in committed tree: {component_id}"
                    ))
                })?;
                nodes.push(node.clone());
                ops.push(CommitRecordOp::ReplaceComponent {
                    component_id: component_id.clone(),
                    node_ref: component_id,
                });
            }
        } else {
            return Err(PactError::invalid(
                "move mutation is not implemented for commit records".to_string(),
            ));
        }
    }
    Ok((
        CommitRecord {
            schema: "pact.commit_record.v1".to_string(),
            branch_id: branch_id.to_string(),
            cycle: revision,
            revision,
            lifecycle_tick: None,
            ops,
        },
        nodes,
    ))
}

fn update_deltas_from_request(
    component_id: &str,
    request: &MutationRequest,
    content: &mut ContentStore,
) -> Option<Vec<CommitRecordOp>> {
    if !request.fields.is_empty() {
        return None;
    }

    let mut ops = Vec::new();
    if let Some(text) = &request.content {
        let blob_id = content
            .put_inline("node.content", text, DELTA_BLOB_PREVIEW_CHARS)
            .id;
        ops.push(CommitRecordOp::AttachContentRef {
            component_id: component_id.to_string(),
            blob_id,
        });
    } else if let Some(content_ref) = &request.content_ref {
        if content_ref.id.is_empty() || !content.records.contains_key(&content_ref.id) {
            return None;
        }
        ops.push(CommitRecordOp::AttachContentRef {
            component_id: component_id.to_string(),
            blob_id: content_ref.id.clone(),
        });
    }

    for (key, value) in &request.attrs {
        let value_ref = content
            .put_inline(
                "node.attr",
                &serde_json::to_string(value).unwrap_or_default(),
                DELTA_BLOB_PREVIEW_CHARS,
            )
            .id;
        ops.push(CommitRecordOp::SetComponentAux {
            component_id: component_id.to_string(),
            key_symbol: key.clone(),
            value_ref,
        });
    }

    if ops.is_empty() {
        return None;
    }
    Some(ops)
}

pub(super) fn find_node_value<'a>(tree: &'a Value, component_id: &str) -> Option<&'a Value> {
    if tree.get("id").and_then(Value::as_str) == Some(component_id) {
        return Some(tree);
    }
    for child in tree
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = find_node_value(child, component_id) {
            return Some(found);
        }
    }
    None
}
