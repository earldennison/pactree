use std::collections::{BTreeSet, HashSet};

use serde_json::{Value, json};

use super::snapshot::Snapshot;
use crate::error::{PactError, PactResult};
use crate::selector::{
    SearchSelectorIndexSeed, match_node_value, search_selector_index_seed,
    search_selector_requires_tree,
};

#[cfg(test)]
mod tests;

const COMPONENT_PAGE_ROWS: usize = 4096;
const MAX_STRUCTURAL_COMPONENTS: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum BitmapStorage {
    Dense(Vec<u64>),
    Sparse(Vec<u64>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OrdinalBitmap {
    len: usize,
    storage: BitmapStorage,
}

impl OrdinalBitmap {
    fn dense(len: usize) -> Self {
        Self {
            len,
            storage: BitmapStorage::Dense(vec![0; len.div_ceil(64)]),
        }
    }

    fn sparse(len: usize, mut ordinals: Vec<u64>) -> PactResult<Self> {
        ordinals.sort_unstable();
        ordinals.dedup();
        if ordinals
            .last()
            .is_some_and(|ordinal| *ordinal >= len as u64)
        {
            return Err(PactError::corrupt(
                "Pact search selector ordinal exceeds the component surface",
            ));
        }
        Ok(Self {
            len,
            storage: BitmapStorage::Sparse(ordinals),
        })
    }

    fn insert(&mut self, ordinal: usize) -> PactResult<()> {
        if ordinal >= self.len {
            return Err(PactError::corrupt(
                "Pact search selector ordinal exceeds the component surface",
            ));
        }
        match &mut self.storage {
            BitmapStorage::Dense(words) => words[ordinal / 64] |= 1_u64 << (ordinal % 64),
            BitmapStorage::Sparse(ordinals) => ordinals.push(ordinal as u64),
        }
        Ok(())
    }

    pub(crate) fn contains(&self, ordinal: u64) -> bool {
        let Ok(ordinal) = usize::try_from(ordinal) else {
            return false;
        };
        if ordinal >= self.len {
            return false;
        }
        match &self.storage {
            BitmapStorage::Dense(words) => words[ordinal / 64] & (1_u64 << (ordinal % 64)) != 0,
            BitmapStorage::Sparse(ordinals) => ordinals.binary_search(&(ordinal as u64)).is_ok(),
        }
    }

    fn ordinals(&self) -> Vec<u64> {
        match &self.storage {
            BitmapStorage::Sparse(ordinals) => ordinals.clone(),
            BitmapStorage::Dense(words) => words
                .iter()
                .enumerate()
                .flat_map(|(word_index, word)| {
                    let mut remaining = *word;
                    let mut ordinals = Vec::with_capacity(remaining.count_ones() as usize);
                    while remaining != 0 {
                        let bit = remaining.trailing_zeros() as usize;
                        ordinals.push((word_index * 64 + bit) as u64);
                        remaining &= remaining - 1;
                    }
                    ordinals
                })
                .filter(|ordinal| *ordinal < self.len as u64)
                .collect(),
        }
    }

    fn count(&self) -> usize {
        match &self.storage {
            BitmapStorage::Dense(words) => {
                words.iter().map(|word| word.count_ones() as usize).sum()
            }
            BitmapStorage::Sparse(ordinals) => ordinals.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MetadataTraversalPlan {
    pub(crate) eligible: OrdinalBitmap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BoundedStructuralTraversalPlan {
    pub(crate) eligible: OrdinalBitmap,
    pub(crate) components_visited: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchSelectorPlan {
    IndexedBitmap(OrdinalBitmap),
    MetadataTraversal(MetadataTraversalPlan),
    BoundedStructuralTraversal(BoundedStructuralTraversalPlan),
    UnsupportedForSnapshot { reason: String },
}

impl SearchSelectorPlan {
    fn bitmap(&self) -> Option<&OrdinalBitmap> {
        match self {
            Self::IndexedBitmap(bitmap) => Some(bitmap),
            Self::MetadataTraversal(plan) => Some(&plan.eligible),
            Self::BoundedStructuralTraversal(plan) => Some(&plan.eligible),
            Self::UnsupportedForSnapshot { .. } => None,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::IndexedBitmap(_) => "indexed_bitmap",
            Self::MetadataTraversal(_) => "metadata_traversal",
            Self::BoundedStructuralTraversal(_) => "bounded_structural_traversal",
            Self::UnsupportedForSnapshot { .. } => "unsupported_for_snapshot",
        }
    }
}

impl From<&SearchSelectorPlan> for crate::search_contract::PactSearchSelectorEligibility {
    fn from(plan: &SearchSelectorPlan) -> Self {
        let bitmap = plan.bitmap();
        let eligible_ordinals = bitmap.map_or_else(Vec::new, OrdinalBitmap::ordinals);
        debug_assert!(bitmap.is_none_or(|bitmap| {
            eligible_ordinals
                .iter()
                .all(|ordinal| bitmap.contains(*ordinal))
        }));
        Self {
            total_components: bitmap.map_or(0, |bitmap| bitmap.len as u64),
            eligible_components: bitmap.map_or(0, |bitmap| bitmap.count() as u64),
            eligible_ordinals,
            execution_plan: plan.kind().to_string(),
        }
    }
}

pub(crate) fn execute(
    snapshot: &Snapshot,
    branch: &str,
    selector: &str,
    allow_expensive_selector: bool,
) -> PactResult<SearchSelectorPlan> {
    let metadata = snapshot.metadata()?;
    if metadata.composition_branches.contains(branch) {
        return unsupported(
            "composed branches require historical replay and are not snapshot-indexed",
            allow_expensive_selector,
        );
    }
    let parsed = crate::file_format::corpus::parse_snapshot_search_selector(selector)?;
    if !parsed.current_only {
        return unsupported(
            "historical and non-current temporal selectors require the full rebuild oracle",
            allow_expensive_selector,
        );
    }
    let structural = parsed.structural_selector;

    let derived_depth = metadata.derived_depth_branches.contains(branch);
    if parsed.has_result_controls
        || search_selector_requires_tree(&structural)?
        || (derived_depth && crate::selector::selector_reads_depth(&structural)?)
    {
        return execute_structural(
            snapshot,
            branch,
            &metadata.root_id,
            &parsed.structural_selector_with_controls,
            derived_depth,
        );
    }
    if let Some(seed) = search_selector_index_seed(&structural)?
        && seed_is_available(snapshot, &seed)?
    {
        return execute_indexed(snapshot, branch, &metadata.root_id, &structural, seed);
    }
    execute_metadata(snapshot, branch, &metadata.root_id, &structural)
}

fn unsupported(reason: &str, allow_expensive_selector: bool) -> PactResult<SearchSelectorPlan> {
    if !allow_expensive_selector {
        return Err(PactError::unsupported(format!(
            "{reason}; pass allow_expensive_selector=true to surface the explicit fallback plan"
        )));
    }
    Ok(SearchSelectorPlan::UnsupportedForSnapshot {
        reason: reason.to_string(),
    })
}

fn seed_is_available(snapshot: &Snapshot, seed: &SearchSelectorIndexSeed) -> PactResult<bool> {
    if matches!(
        seed,
        SearchSelectorIndexSeed::Attribute { key, .. }
            if matches!(key.as_str(), "tool_call_id" | "tool_name" | "success")
    ) {
        return snapshot.has_search_selector_facet();
    }
    Ok(true)
}

fn execute_indexed(
    snapshot: &Snapshot,
    branch: &str,
    root_id: &str,
    selector: &str,
    seed: SearchSelectorIndexSeed,
) -> PactResult<SearchSelectorPlan> {
    if matches!(&seed, SearchSelectorIndexSeed::Type(value) if matches!(value.as_str(), "b" | "c" | "seg"))
    {
        return execute_metadata(snapshot, branch, root_id, selector);
    }
    let total = total_components(snapshot, branch)?;
    let candidates = match seed {
        SearchSelectorIndexSeed::Id(component_id) => snapshot
            .component_shallow(branch, &component_id)?
            .into_iter()
            .collect(),
        SearchSelectorIndexSeed::Type(component_type) => {
            snapshot
                .components_by_type(branch, &component_type, 0, total)?
                .1
        }
        SearchSelectorIndexSeed::Attribute { key, value } => {
            snapshot
                .components_by_aux(branch, &key, &value, 0, total)?
                .1
        }
        SearchSelectorIndexSeed::Tag(query) => snapshot.components_by_tag_query(branch, &query)?,
    };
    let mut ordinals = Vec::with_capacity(candidates.len());
    if root_matches(root_id, selector)? {
        ordinals.push(0);
    }
    for component in candidates {
        if !match_node_value(&component.value, selector)? {
            continue;
        }
        ordinals.push(component_ordinal(
            snapshot,
            branch,
            component_id(&component.value)?,
        )?);
    }
    let bitmap = OrdinalBitmap::sparse(total, ordinals)?;
    Ok(SearchSelectorPlan::IndexedBitmap(bitmap))
}

fn execute_metadata(
    snapshot: &Snapshot,
    branch: &str,
    root_id: &str,
    selector: &str,
) -> PactResult<SearchSelectorPlan> {
    let total = total_components(snapshot, branch)?;
    let mut bitmap = OrdinalBitmap::dense(total);
    if root_matches(root_id, selector)? {
        bitmap.insert(0)?;
    }
    let mut offset = 0;
    loop {
        let (indexed_total, components) =
            snapshot.components_shallow_window(branch, offset, COMPONENT_PAGE_ROWS)?;
        if indexed_total.saturating_add(1) != total {
            return Err(PactError::corrupt(
                "Pact component index count disagrees with head metadata",
            ));
        }
        if components.is_empty() {
            break;
        }
        for (index, component) in components.iter().enumerate() {
            if match_node_value(&component.value, selector)? {
                bitmap.insert(offset + index + 1)?;
            }
        }
        offset = offset.saturating_add(components.len());
        if offset >= indexed_total {
            break;
        }
    }
    Ok(SearchSelectorPlan::MetadataTraversal(
        MetadataTraversalPlan { eligible: bitmap },
    ))
}

fn execute_structural(
    snapshot: &Snapshot,
    branch: &str,
    root_id: &str,
    selector: &str,
    derived_depth: bool,
) -> PactResult<SearchSelectorPlan> {
    let total = total_components(snapshot, branch)?;
    if total > MAX_STRUCTURAL_COMPONENTS {
        return Err(PactError::unsupported(format!(
            "structural selector visits {total} components, exceeding the bounded metadata ceiling of {MAX_STRUCTURAL_COMPONENTS}"
        )));
    }
    let mut visited = HashSet::with_capacity(total);
    visited.insert(root_id.to_string());
    let mut root = root_value(root_id);
    let child_ids = snapshot.parent_children(branch, root_id)?;
    let mut children = Vec::with_capacity(child_ids.len());
    for child_id in &child_ids {
        children.push(build_metadata_subtree(
            snapshot,
            branch,
            child_id,
            &mut visited,
            total,
        )?);
    }
    root["child_ids"] = Value::Array(child_ids.into_iter().map(Value::String).collect());
    root["children"] = Value::Array(children);
    if visited.len() != total {
        return Err(PactError::corrupt(
            "Pact parent-child facet does not cover the indexed component surface",
        ));
    }
    if derived_depth {
        crate::mutation::derive_turn_depths_value(&mut root);
    }
    let mut component_ids = crate::selector::select_components(&root, selector, true)?
        .into_iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    deduplicate_ids(&mut component_ids);
    let mut ordinals = Vec::with_capacity(component_ids.len());
    for component_id in &component_ids {
        ordinals.push(if component_id == root_id {
            0
        } else {
            component_ordinal(snapshot, branch, component_id)?
        });
    }
    let bitmap = OrdinalBitmap::sparse(total, ordinals)?;
    Ok(SearchSelectorPlan::BoundedStructuralTraversal(
        BoundedStructuralTraversalPlan {
            eligible: bitmap,
            components_visited: visited.len(),
        },
    ))
}

fn build_metadata_subtree(
    snapshot: &Snapshot,
    branch: &str,
    component_id: &str,
    visited: &mut HashSet<String>,
    limit: usize,
) -> PactResult<Value> {
    if visited.len() >= limit || !visited.insert(component_id.to_string()) {
        return Err(PactError::corrupt(
            "Pact parent-child facet exceeds the component count or contains a cycle",
        ));
    }
    let mut component = snapshot
        .component_shallow(branch, component_id)?
        .ok_or_else(|| {
            PactError::corrupt("Pact parent-child facet references a missing component")
        })?
        .value;
    let child_ids = snapshot.parent_children(branch, component_id)?;
    let mut children = Vec::with_capacity(child_ids.len());
    for child_id in &child_ids {
        children.push(build_metadata_subtree(
            snapshot, branch, child_id, visited, limit,
        )?);
    }
    component["child_ids"] = Value::Array(child_ids.into_iter().map(Value::String).collect());
    component["children"] = Value::Array(children);
    Ok(component)
}

fn total_components(snapshot: &Snapshot, branch: &str) -> PactResult<usize> {
    snapshot
        .metadata()?
        .node_counts
        .get(branch)
        .copied()
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch}")))
}

fn component_ordinal(snapshot: &Snapshot, branch: &str, component_id: &str) -> PactResult<u64> {
    let (_, rank) = snapshot.component_count_and_rank(branch, component_id)?;
    Ok(rank as u64 + 1)
}

fn component_id(value: &Value) -> PactResult<&str> {
    value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PactError::corrupt("indexed Pact component has no string id"))
}

fn root_matches(root_id: &str, selector: &str) -> PactResult<bool> {
    match_node_value(&root_value(root_id), selector)
}

fn root_value(root_id: &str) -> Value {
    json!({
        "id": root_id,
        "componentType": "context_root",
        "type": "context_root",
        "offset": 0,
        "priority": 0,
        "created_at_ns": 0,
        "creation_index": 0,
        "children": [],
    })
}

fn deduplicate_ids(component_ids: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    component_ids.retain(|component_id| seen.insert(component_id.clone()));
}
