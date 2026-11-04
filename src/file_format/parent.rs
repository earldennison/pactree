use crate::error::{PactError, PactResult};
use crate::store::{ContextTreeLedger, PactParentProvenance, PactRef};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParentResolverConfig {
    pub base_dir: Option<PathBuf>,

    pub search_paths: Vec<PathBuf>,

    pub explicit_parents: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedParent {
    pub path: PathBuf,
    pub(crate) ledger: ContextTreeLedger,
}

pub(crate) fn resolve_parent(
    child_path: Option<&Path>,
    parent: &PactParentProvenance,
    config: &ParentResolverConfig,
) -> PactResult<VerifiedParent> {
    let parent_ref = &parent.parent_ref;
    let mut tried: Vec<String> = Vec::new();
    let mut mismatch: Option<PactError> = None;

    for candidate in candidate_paths(child_path, parent, config) {
        tried.push(candidate.display().to_string());
        match verify_candidate(&candidate, parent_ref) {
            Ok(Some(verified)) => return Ok(verified),

            Ok(None) => {}

            Err(err @ PactError::ParentMismatch(_)) => {
                if mismatch.is_none() {
                    mismatch = Some(err);
                }
            }
            Err(other) => return Err(other),
        }
    }

    if let Some(mismatch) = mismatch {
        return Err(mismatch);
    }
    Err(PactError::unresolved_parent(format!(
        "could not resolve parent .pact for root_id {root:?} branch {branch:?}; tried: [{tried}]",
        root = parent_ref.root_id,
        branch = parent_ref.branch_id,
        tried = tried.join(", "),
    )))
}

fn candidate_paths(
    child_path: Option<&Path>,
    parent: &PactParentProvenance,
    config: &ParentResolverConfig,
) -> Vec<PathBuf> {
    let root_id = &parent.parent_ref.root_id;
    let file_name = format!("{root_id}.pact");
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(path) = config.explicit_parents.get(root_id) {
        candidates.push(path.clone());
    }

    for hint in &parent.locator_hints {
        for path in hint_candidate_paths(hint, config, child_path) {
            candidates.push(path);
        }
    }

    if let Some(base_dir) = &config.base_dir {
        candidates.push(base_dir.join(&file_name));
    }
    for dir in &config.search_paths {
        candidates.push(dir.join(&file_name));
    }

    if let Some(child_path) = child_path
        && let Some(dir) = child_path.parent()
    {
        candidates.push(dir.join(&file_name));
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&file_name));
    }

    dedup_preserving_order(candidates)
}

fn hint_candidate_paths(
    hint: &crate::store::PactParentLocator,
    config: &ParentResolverConfig,
    child_path: Option<&Path>,
) -> Vec<PathBuf> {
    match hint.kind.as_str() {
        "path" => vec![PathBuf::from(&hint.value)],
        "relative_path" => {
            let mut out = Vec::new();
            if let Some(base_dir) = &config.base_dir {
                out.push(base_dir.join(&hint.value));
            }
            if let Some(child_path) = child_path
                && let Some(dir) = child_path.parent()
            {
                out.push(dir.join(&hint.value));
            }
            out.push(PathBuf::from(&hint.value));
            out
        }
        _ => Vec::new(),
    }
}

fn dedup_preserving_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

fn verify_candidate(candidate: &Path, parent_ref: &PactRef) -> PactResult<Option<VerifiedParent>> {
    if !candidate.is_file() {
        return Ok(None);
    }
    let ledger = super::file_open_ledger(candidate).map_err(|error| {
        PactError::parent_mismatch(format!(
            "candidate parent {candidate:?} failed validation: {error}"
        ))
    })?;

    if ledger.root_id != parent_ref.root_id {
        return Err(PactError::parent_mismatch(format!(
            "candidate parent {candidate:?} root_id {found:?} does not match expected {expected:?}",
            found = ledger.root_id,
            expected = parent_ref.root_id,
        )));
    }

    verify_ref_in_ledger(candidate, &ledger, parent_ref)?;
    Ok(Some(VerifiedParent {
        path: candidate.to_path_buf(),
        ledger,
    }))
}

fn verify_ref_in_ledger(
    candidate: &Path,
    ledger: &ContextTreeLedger,
    parent_ref: &PactRef,
) -> PactResult<()> {
    let branch = ledger.branches.get(&parent_ref.branch_id).ok_or_else(|| {
        PactError::parent_mismatch(format!(
            "candidate parent {candidate:?} has no branch {branch:?}",
            branch = parent_ref.branch_id,
        ))
    })?;

    if let Some(revision) = parent_ref.revision
        && revision > branch.head_revision
    {
        return Err(PactError::parent_mismatch(format!(
            "candidate parent {candidate:?} branch {branch:?} head {head} is before requested revision {revision}",
            branch = parent_ref.branch_id,
            head = branch.head_revision,
        )));
    }

    if let Some(transaction_id) = &parent_ref.transaction_id
        && !ledger
            .event_index
            .transaction_traces
            .contains_key(transaction_id)
    {
        return Err(PactError::parent_mismatch(format!(
            "candidate parent {candidate:?} has no transaction {transaction_id:?}",
        )));
    }

    if let Some(component_id) = &parent_ref.component_id
        && !branch.current.tree.first_seen.contains_key(component_id)
        && !branch.current.tree.index.by_id.contains_key(component_id)
    {
        return Err(PactError::parent_mismatch(format!(
            "candidate parent {candidate:?} branch {branch:?} has no component {component_id:?}",
            branch = parent_ref.branch_id,
        )));
    }

    Ok(())
}
