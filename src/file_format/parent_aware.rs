use super::parent::{ParentResolverConfig, VerifiedParent, resolve_parent};
use super::{
    PactFileSelectorQuery, PactFileSelectorResult, ledger_from_records, scan_file_cached,
    select_records, selector_reaches_prefork_history,
};
use crate::error::{PactError, PactResult};
use crate::store::{ContextTreeLedger, project_branch_component_tree};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentAwareScope {
    ChildHead,

    CrossFork,
}

#[derive(Clone, Debug)]
pub struct ParentAwareProjection {
    pub tree: Value,

    pub scope: ParentAwareScope,

    pub resolved_parent: Option<VerifiedParent>,
}

const CROSS_FORK_HISTORY_BLOCKER: &str = "parent-crossing history is not yet wired: it requires the streaming \
     projection primitive (streaming-projection-replaces-materialization) to \
     stitch the verified parent's pre-fork record stream onto the child; the \
     parent resolver and child fast path are complete";

pub fn project_with_parents(
    child_path: Option<&Path>,
    ledger: &ContextTreeLedger,
    branch_id: &str,
    revision: Option<u64>,
    scope: ParentAwareScope,
    parent_config: &ParentResolverConfig,
) -> PactResult<ParentAwareProjection> {
    let branch = ledger
        .branches
        .get(branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    let tree = project_branch_component_tree(ledger, branch, revision)?.project_value();

    match scope {
        ParentAwareScope::ChildHead => Ok(ParentAwareProjection {
            tree,
            scope,
            resolved_parent: None,
        }),
        ParentAwareScope::CrossFork => {
            let Some(provenance) = ledger.parent.as_ref() else {
                return Ok(ParentAwareProjection {
                    tree,
                    scope: ParentAwareScope::ChildHead,
                    resolved_parent: None,
                });
            };

            let verified = resolve_parent(child_path, provenance, parent_config)?;
            let _ = verified;
            Err(PactError::unsupported(CROSS_FORK_HISTORY_BLOCKER))
        }
    }
}

pub fn file_select_with_parents(
    child_path: &Path,
    query: PactFileSelectorQuery,
    parent_config: &ParentResolverConfig,
) -> PactResult<PactFileSelectorResult> {
    let scanned = scan_file_cached(child_path, true)?;
    if !scanned.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot run selector on invalid .pact file: {}",
            scanned.errors.join("; ")
        )));
    }

    if !selector_reaches_prefork_history(&query.selector)? {
        return select_records(&scanned.records, query);
    }

    let ledger = ledger_from_records(&scanned.records)?;
    let Some(provenance) = ledger.parent.as_ref() else {
        return select_records(&scanned.records, query);
    };

    let verified = resolve_parent(Some(child_path), provenance, parent_config)?;
    let _ = verified;
    Err(PactError::unsupported(CROSS_FORK_HISTORY_BLOCKER))
}
