use super::parent::{ParentResolverConfig, VerifiedParent, resolve_parent};
use super::{decode_context_tree_ledger, file_create_from_ledger_json, file_open_ledger};
use crate::error::{PactError, PactResult};
use crate::store::{
    ContextTreeLedger, PactBaseMode, PactBaseProvenance, PactBranch, PactRef,
    project_branch_component_tree,
};
use crate::tree::ComponentTree;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) fn write_delta_overlay(
    source_path: &Path,
    output_path: &Path,
    base_mode: PactBaseMode,
    branch_id: Option<&str>,
    revision: Option<u64>,
    locator_hints: Vec<crate::store::PactParentLocator>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let source_ledger = file_open_ledger(source_path)?;
    let source_branch_id = branch_id
        .map(str::to_string)
        .unwrap_or_else(|| source_ledger.current_branch_id.clone());
    let source_branch = source_ledger
        .branches
        .get(&source_branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {source_branch_id}")))?;
    let fork_revision = revision.unwrap_or(source_branch.head_revision);
    if fork_revision > source_branch.head_revision {
        return Err(PactError::invalid(format!(
            "requested revision {fork_revision} is after branch {source_branch_id} head {}",
            source_branch.head_revision
        )));
    }

    let created_from_head = PactRef {
        root_id: source_ledger.root_id.clone(),
        branch_id: source_branch_id.clone(),
        revision: Some(fork_revision),
        transaction_id: None,
        component_id: None,
        content_ref: None,
    };

    let base_ref = PactRef {
        revision: match base_mode {
            PactBaseMode::Pinned => Some(fork_revision),
            PactBaseMode::Following => None,
        },
        ..created_from_head.clone()
    };

    let child_root_id =
        super::mint_child_root_id(&source_ledger.root_id, &source_branch_id, fork_revision);
    let mut empty_tree =
        ComponentTree::from_value(&crate::store::empty_context_root_value(&child_root_id))?;
    let new_root_id = child_root_id.clone();
    empty_tree.edit(|root| {
        root.id = new_root_id.clone();
        Ok(())
    })?;
    let child_tree_json = serde_json::to_string(&empty_tree.project_value())
        .map_err(|err| PactError::codec(format!("failed to encode child bootstrap tree: {err}")))?;
    let child_ledger_json = crate::store::new_ledger_json(
        &child_tree_json,
        Some(&source_branch_id),
        blob_inline_limit,
    )?;
    let mut child_ledger = decode_context_tree_ledger(&child_ledger_json)?;
    if child_ledger.root_id != child_root_id {
        return Err(PactError::internal(format!(
            "child ledger root_id {} did not match minted child root id {child_root_id}",
            child_ledger.root_id
        )));
    }

    if let Some(branch) = child_ledger.branches.get_mut(&source_branch_id) {
        branch.transaction_traces.clear();
    }
    child_ledger.base = Some(PactBaseProvenance {
        base_ref: base_ref.clone(),
        mode: base_mode.clone(),
        created_from_head: created_from_head.clone(),
        locator_hints,
        aux: BTreeMap::new(),
    });

    let child_ledger_json = serde_json::to_string(&child_ledger)
        .map_err(|err| PactError::codec(format!("failed to encode child ledger: {err}")))?;
    let manifest_json =
        file_create_from_ledger_json(output_path, &child_ledger_json, blob_inline_limit)?;
    let manifest: Value = serde_json::from_str(&manifest_json)
        .map_err(|err| PactError::codec(format!("failed to decode child manifest: {err}")))?;

    serde_json::to_string_pretty(&json!({
        "schema": format!("{}.fork.delta.v1", super::PACT_FILE_SCHEMA),
        "mode": "delta",
        "base_mode": base_mode_str(&base_mode),
        "source_path": source_path.display().to_string(),
        "child_path": output_path.display().to_string(),
        "source_branch_id": source_branch_id,
        "revision": fork_revision,
        "base_ref": base_ref,
        "created_from_head": created_from_head,
        "child_root_id": child_root_id,
        "child_branch_id": source_branch_id,
        "manifest": manifest,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode delta fork report: {err}")))
}

fn base_mode_str(mode: &PactBaseMode) -> &'static str {
    match mode {
        PactBaseMode::Pinned => "pinned",
        PactBaseMode::Following => "following",
    }
}

#[derive(Clone, Debug)]
pub struct DeltaProjection {
    pub tree: Value,

    pub base_mode: PactBaseMode,

    pub base_revision: u64,

    pub resolved_base: VerifiedParent,
}

pub fn project_delta_overlay(
    child_path: Option<&Path>,
    child_ledger: &ContextTreeLedger,
    branch_id: Option<&str>,
    parent_config: &ParentResolverConfig,
) -> PactResult<DeltaProjection> {
    let provenance = child_ledger.base.as_ref().ok_or_else(|| {
        PactError::invalid("cannot project delta overlay: ledger carries no base provenance")
    })?;

    let resolved_base =
        resolve_parent(child_path, &as_parent_provenance(provenance), parent_config)
            .map_err(map_base_resolution_error)?;
    let base_ledger = &resolved_base.ledger;

    let base_branch_id = &provenance.base_ref.branch_id;
    let base_branch = base_ledger.branches.get(base_branch_id).ok_or_else(|| {
        PactError::unresolved_parent(format!(
            "verified base has no branch {base_branch_id:?} for delta overlay"
        ))
    })?;
    let base_revision = match provenance.mode {
        PactBaseMode::Pinned => provenance
            .created_from_head
            .revision
            .unwrap_or(base_branch.head_revision),

        PactBaseMode::Following => base_branch.head_revision,
    };
    let base_tree = project_base_tree_value(base_ledger, base_branch, base_revision)?;

    let composed = replay_child_local_traces(base_tree, child_ledger, branch_id)?;

    Ok(DeltaProjection {
        tree: composed,
        base_mode: provenance.mode.clone(),
        base_revision,
        resolved_base,
    })
}

fn project_base_tree_value(
    base_ledger: &ContextTreeLedger,
    base_branch: &PactBranch,
    revision: u64,
) -> PactResult<Value> {
    let tree = project_branch_component_tree(base_ledger, base_branch, Some(revision))?;
    Ok(tree.project_value())
}

fn replay_child_local_traces(
    base_tree: Value,
    child_ledger: &ContextTreeLedger,
    branch_id: Option<&str>,
) -> PactResult<Value> {
    let mut tree = ComponentTree::from_value(&base_tree)?;
    let child_branch_id = branch_id
        .map(str::to_string)
        .unwrap_or_else(|| child_ledger.current_branch_id.clone());
    let Some(child_branch) = child_ledger.branches.get(&child_branch_id) else {
        return Ok(tree.project_value());
    };

    let mut traces = child_branch.transaction_traces.clone();
    traces.sort_by_key(|trace| (trace.to_rev, trace.from_rev));
    for trace in &traces {
        trace.validate()?;

        tree.edit(|root| {
            let before = root.project_component_value();
            let after = trace.replay(&before)?;
            let replayed_root = crate::components::Component::from_component_value(&after)
                .map_err(|err| {
                    PactError::invalid(format!(
                        "delta overlay: replayed child trace produced invalid component tree: {err}"
                    ))
                })?;
            *root = replayed_root;
            Ok(())
        })?;
    }

    Ok(tree.project_value())
}

fn as_parent_provenance(provenance: &PactBaseProvenance) -> crate::store::PactParentProvenance {
    crate::store::PactParentProvenance {
        parent_ref: provenance.base_ref.clone(),
        locator_hints: provenance.locator_hints.clone(),
        aux: provenance.aux.clone(),
    }
}

fn map_base_resolution_error(err: PactError) -> PactError {
    match err {
        PactError::UnresolvedParent(msg) => {
            PactError::unresolved_parent(format!("unresolved delta-overlay base: {msg}"))
        }
        PactError::ParentMismatch(msg) => {
            PactError::parent_mismatch(format!("delta-overlay base mismatch: {msg}"))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests;
