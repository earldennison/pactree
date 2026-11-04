use super::model::{
    ComponentRecord, ContentStore, ContextTreeLedger, PactBranch, PactStore, ROOT_STORE_SCHEMA,
    STORE_SCHEMA,
};
use super::projection::content_ref_id_from_value;
use crate::components::Component;
use crate::error::{PactError, PactResult};
use crate::file_format::base64_encode;
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};
use crate::tree::{ComponentTree, component_by_id_mut};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ProjectionTreeScope {
    #[default]
    Head,
    Archive,
}

impl ProjectionTreeScope {
    pub fn parse(value: &str) -> PactResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "head" | "current" => Ok(Self::Head),
            "archive" => Ok(Self::Archive),
            other => Err(PactError::invalid(format!(
                "unsupported projection scope {other:?}; expected head or archive"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Archive => "archive",
        }
    }
}

pub(crate) fn reject_removed_patch_history(value: &Value) -> PactResult<()> {
    const REMOVED_FIELDS: [&str; 2] = [concat!("patch", "es"), concat!("legacy_patch", "_imports")];
    fn removed_field_error() -> PactError {
        PactError::invalid(
            "removed patch-shaped mutation history is unsupported; use TransactionTrace",
        )
    }
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for field in REMOVED_FIELDS {
        if object.contains_key(field) {
            return Err(removed_field_error());
        }
    }
    if let Some(branches) = object.get("branches") {
        let branch_values: Vec<&Value> = match branches {
            Value::Object(map) => map.values().collect(),
            Value::Array(items) => items.iter().collect(),
            _ => Vec::new(),
        };
        for branch in branch_values {
            if let Some(branch_object) = branch.as_object() {
                for field in REMOVED_FIELDS {
                    if branch_object.contains_key(field) {
                        return Err(removed_field_error());
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn decode_store(store_json: &str) -> PactResult<PactStore> {
    let value: Value = serde_json::from_str(store_json)
        .map_err(|err| PactError::codec(format!("invalid PACT store JSON: {err}")))?;
    reject_removed_patch_history(&value)?;
    let store: PactStore = serde_json::from_value(value)
        .map_err(|err| PactError::codec(format!("invalid PACT store JSON: {err}")))?;
    if store.schema != STORE_SCHEMA {
        return Err(PactError::invalid(format!(
            "unsupported PACT store schema: {}",
            store.schema
        )));
    }
    Ok(store)
}

pub fn decode_context_tree_ledger(root_store_json: &str) -> PactResult<ContextTreeLedger> {
    let value: Value = serde_json::from_str(root_store_json)
        .map_err(|err| PactError::codec(format!("invalid PACT root store JSON: {err}")))?;
    reject_removed_patch_history(&value)?;
    let mut store: ContextTreeLedger = serde_json::from_value(value)
        .map_err(|err| PactError::codec(format!("invalid PACT root store JSON: {err}")))?;
    if store.schema != ROOT_STORE_SCHEMA {
        return Err(PactError::invalid(format!(
            "unsupported PACT root store schema: {}",
            store.schema
        )));
    }
    store.content.rebuild_hash_index();
    Ok(store)
}

pub(crate) fn project_branch_tree_value(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
) -> PactResult<Value> {
    Ok(project_branch_component_tree(root_store, branch, revision)?.project_value())
}

pub(crate) fn project_branch_archive_tree_value(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
) -> PactResult<(Value, BTreeMap<String, u64>)> {
    let (tree, first_seen) = project_branch_archive_component_tree(root_store, branch, revision)?;
    Ok((archive_tree_value(&tree), first_seen))
}

pub(crate) fn project_branch_archive_component_tree(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
) -> PactResult<(ComponentTree, BTreeMap<String, u64>)> {
    let (mut tree, first_seen) = project_branch_archive_component_tree_inner(
        root_store,
        branch,
        revision,
        &mut BTreeSet::new(),
    )?;
    apply_derived_depth(branch, &mut tree)?;
    Ok((tree, first_seen))
}

fn apply_derived_depth(branch: &PactBranch, tree: &mut ComponentTree) -> PactResult<()> {
    if !branch.derived_depth() {
        return Ok(());
    }
    tree.edit(|root| {
        crate::mutation::derive_turn_depths_component(root);
        Ok(())
    })
}

pub(crate) fn project_branch_current_component_tree(
    branch: &PactBranch,
    revision: Option<u64>,
) -> PactResult<Option<ComponentTree>> {
    let target_revision = revision.unwrap_or(branch.head_revision);
    if target_revision != branch.head_revision
        || target_revision != branch.current.head_revision
        || branch.current.component_count() == 0
    {
        return Ok(None);
    }
    ComponentTree::from_root(branch.current.tree.root.clone()).map(Some)
}

pub(crate) fn branch_has_native_component_projection_at(
    branch: &PactBranch,
    revision: Option<u64>,
) -> bool {
    let target_revision = revision.unwrap_or(branch.head_revision);
    if target_revision == branch.current.head_revision && branch.current.component_count() > 0 {
        return true;
    }
    branch_has_trace_replay_at(branch, target_revision)
        || (branch.current.component_count() > 0
            && branch_has_trace_replay_between(
                branch,
                branch.current.head_revision,
                target_revision,
            ))
}

pub(crate) fn project_branch_component_tree(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
) -> PactResult<ComponentTree> {
    let mut tree =
        project_branch_component_tree_inner(root_store, branch, revision, &mut BTreeSet::new())?;
    apply_derived_depth(branch, &mut tree)?;
    Ok(tree)
}

fn project_branch_component_tree_inner(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
    seen_branches: &mut BTreeSet<String>,
) -> PactResult<ComponentTree> {
    if let Some(tree) = project_branch_current_component_tree(branch, revision)? {
        return Ok(tree);
    }
    let target_revision = revision.unwrap_or(branch.head_revision);
    if !branch_has_native_component_projection_at(branch, revision) {
        if branch.base_ref.content_ref.is_none() {
            return Err(PactError::storage(format!(
                "branch {} has no base content ref; can project only current head revision {}, not revision {}",
                branch.branch_id, branch.current.head_revision, target_revision
            )));
        }
        return Err(PactError::storage(format!(
            "branch {} has no complete typed replay path to revision {target_revision}",
            branch.branch_id
        )));
    }
    project_branch_trace_component_tree(root_store, branch, target_revision, seen_branches)
}

fn archive_tree_value(tree: &ComponentTree) -> Value {
    fn lift_markers(value: &mut Value) {
        let Value::Object(component) = value else {
            return;
        };
        if let Some(Value::Array(children)) = component.get_mut("children") {
            children.iter_mut().for_each(lift_markers);
        }
        let markers = component
            .get("aux")
            .and_then(Value::as_object)
            .map(|aux| {
                [
                    "first_seen",
                    "pact_lifecycle",
                    "visible_in_head",
                    "deleted_at",
                    "last_seen",
                ]
                .into_iter()
                .filter_map(|key| aux.get(key).cloned().map(|value| (key, value)))
                .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(Value::Object(aux)) = component.get_mut("aux") {
            for (key, _) in &markers {
                aux.remove(*key);
            }
            if aux.is_empty() {
                component.remove("aux");
            }
        }
        for (key, value) in markers {
            component.insert(key.to_string(), value);
        }
    }

    let mut value = tree.project_value();
    lift_markers(&mut value);
    value
}

pub(crate) fn project_head_snapshot(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
) -> PactResult<ComponentTree> {
    project_branch_component_tree(root_store, branch, revision)
}

fn project_branch_trace_component_tree(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    target_revision: u64,
    seen_branches: &mut BTreeSet<String>,
) -> PactResult<ComponentTree> {
    #[cfg(test)]
    crate::tests::instrumentation::BRANCH_TRACE_COMPONENT_TREE_REPLAYS
        .with(|count| count.set(count.get() + 1));

    if !seen_branches.insert(branch.branch_id.clone()) {
        return Err(PactError::storage(format!(
            "cyclic fork lineage at branch {}",
            branch.branch_id
        )));
    }

    let covered_from_base = branch_has_trace_replay_at(branch, target_revision);
    let (mut tree, start_revision) = if !covered_from_base
        && branch.current.component_count() > 0
        && branch_has_trace_replay_between(branch, branch.current.head_revision, target_revision)
    {
        (
            ComponentTree::from_root(branch.current.tree.root.clone())?,
            branch.current.head_revision,
        )
    } else if let Some(content_ref) = branch.base_ref.content_ref.as_deref() {
        let record = root_store.content.records.get(content_ref).ok_or_else(|| {
            PactError::not_found(format!("missing branch base content record: {content_ref}"))
        })?;
        let base_json = record.storage.inline.as_deref().ok_or_else(|| {
            PactError::storage(format!(
                "branch base content is not inline yet: {content_ref}"
            ))
        })?;
        let mut base: Value = serde_json::from_str(base_json)
            .map_err(|err| PactError::codec(format!("invalid branch base PACT JSON: {err}")))?;
        hydrate_content_refs_in_value(&mut base, &root_store.content);
        (
            ComponentTree::from_value(&base)?,
            trace_replay_base_revision(branch),
        )
    } else if let Some(fork_ref) = branch.fork_ref.as_ref()
        && let Some(parent_branch) = root_store.branches.get(&fork_ref.branch_id)
    {
        let parent_revision = fork_ref.revision.unwrap_or(parent_branch.head_revision);
        (
            project_branch_component_tree_inner(
                root_store,
                parent_branch,
                Some(parent_revision),
                seen_branches,
            )?,
            0,
        )
    } else if branch.current.component_count() > 0
        && branch_has_trace_replay_between(branch, branch.current.head_revision, target_revision)
    {
        (
            ComponentTree::from_root(branch.current.tree.root.clone())?,
            branch.current.head_revision,
        )
    } else {
        (project_branch_root_skeleton_component_tree(branch)?, 0)
    };

    replay_branch_component_transaction_traces_from(
        &mut tree,
        branch,
        start_revision,
        target_revision,
    )?;
    seen_branches.remove(&branch.branch_id);
    Ok(tree)
}

fn project_branch_root_skeleton_component_tree(branch: &PactBranch) -> PactResult<ComponentTree> {
    let (root_id, nodes) = project_branch_birth_skeleton_records(branch)?;
    ComponentTree::from_value(&project_node_record_tree(&root_id, &nodes)?)
}

fn replay_branch_component_transaction_traces_from(
    tree: &mut ComponentTree,
    branch: &PactBranch,
    start_revision: u64,
    target_revision: u64,
) -> PactResult<()> {
    let mut traces = branch
        .transaction_traces
        .iter()
        .filter(|trace| trace.to_rev > start_revision && trace.to_rev <= target_revision)
        .collect::<Vec<_>>();
    if traces.is_empty() {
        return Ok(());
    }
    traces.sort_by_key(|trace| (trace.to_rev, trace.from_rev));
    tree.edit(|root| {
        for trace in traces {
            trace.validate()?;
            for op in &trace.ops {
                apply_component_trace_op_in(root, op)?;
            }
        }
        Ok(())
    })
}

fn apply_component_trace_op_in(root: &mut Component, op: &TraceOp) -> PactResult<()> {
    let component_id = op.component_id.as_str();
    match &op.kind {
        TraceOpKind::DeleteComponent => detach_component_subtree(root, component_id)
            .map(|_| ())
            .ok_or_else(|| {
                PactError::corrupt(format!("delete: component not found: {component_id}"))
            }),
        TraceOpKind::InsertComponent {
            parent_id,
            index,
            component,
        } => {
            let mut component = component_from_trace_value(component)?;
            component.parent_id = Some(parent_id.clone());
            insert_component_child(root, parent_id, *index, component)
        }
        TraceOpKind::MoveComponent { parent_id, index } => {
            let mut subtree = detach_component_subtree(root, component_id).ok_or_else(|| {
                PactError::corrupt(format!("move: component not found: {component_id}"))
            })?;
            subtree.parent_id = Some(parent_id.clone());
            insert_component_child(root, parent_id, *index, subtree)
        }
        TraceOpKind::SetField { key, value } if key == "child_ids" => {
            let order = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            reorder_component_children(root, component_id, &order)
        }
        TraceOpKind::SetField { key, value } => {
            update_component_in_projected_value(root, component_id, |object| {
                object.insert(key.clone(), value.clone());
            })
        }
        TraceOpKind::UnsetField { key } => {
            update_component_in_projected_value(root, component_id, |object| {
                object.remove(key);
            })
        }
        TraceOpKind::SetAttr { key, value } => {
            let component = component_by_id_mut(root, component_id)
                .ok_or_else(|| PactError::corrupt(format!("node not found: {component_id}")))?;
            component.aux.insert(key.clone(), value.clone());
            Ok(())
        }
        TraceOpKind::UnsetAttr { key } => {
            let component = component_by_id_mut(root, component_id)
                .ok_or_else(|| PactError::corrupt(format!("node not found: {component_id}")))?;
            component.aux.remove(key);
            Ok(())
        }
        TraceOpKind::SetLifecycle { value } => {
            update_component_in_projected_value(root, component_id, |object| {
                object.insert("lifecycle".to_string(), value.clone());
            })
        }
        TraceOpKind::SetContent {
            content,
            content_ref,
            content_preview,
        } => update_component_in_projected_value(root, component_id, |object| {
            match content {
                Some(text) => {
                    object.insert("content".to_string(), Value::String(text.clone()));
                }
                None => {
                    object.remove("content");
                }
            }
            match content_ref {
                Some(value) => {
                    object.insert("content_ref".to_string(), value.clone());
                }
                None => {
                    object.remove("content_ref");
                }
            }
            match content_preview {
                Some(text) => {
                    object.insert("content_preview".to_string(), Value::String(text.clone()));
                }
                None => {
                    object.remove("content_preview");
                }
            }
        }),
        TraceOpKind::ReplaceContentRange {
            start,
            end,
            replacement,
        } => update_component_in_projected_value(root, component_id, |object| {
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
        }),
    }
}

fn update_component_in_projected_value(
    root: &mut Component,
    component_id: &str,
    update: impl FnOnce(&mut Map<String, Value>),
) -> PactResult<()> {
    let component = component_by_id_mut(root, component_id)
        .ok_or_else(|| PactError::corrupt(format!("node not found: {component_id}")))?;

    let children = std::mem::take(&mut component.children);
    let mut value = component.project_component_value();
    let object = value
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt(format!("component is not an object: {component_id}")))?;
    update(object);
    let mut rebuilt = component_from_trace_value(&value)?;
    rebuilt.children = children;
    *component = rebuilt;
    Ok(())
}

fn component_from_trace_value(value: &Value) -> PactResult<Component> {
    Component::from_component_value(value)
        .map_err(|err| PactError::invalid(format!("invalid trace component payload: {err}")))
}

fn detach_component_subtree(root: &mut Component, component_id: &str) -> Option<Component> {
    if let Some(index) = root
        .children
        .iter()
        .position(|child| child.id == component_id)
    {
        return Some(root.children.remove(index));
    }
    for child in &mut root.children {
        if let Some(found) = detach_component_subtree(child, component_id) {
            return Some(found);
        }
    }
    None
}

fn insert_component_child(
    root: &mut Component,
    parent_id: &str,
    index: usize,
    mut component: Component,
) -> PactResult<()> {
    let parent = component_by_id_mut(root, parent_id)
        .ok_or_else(|| PactError::corrupt(format!("insert: parent not found: {parent_id}")))?;
    component.parent_id = Some(parent_id.to_string());
    let at = index.min(parent.children.len());
    parent.children.insert(at, component);
    Ok(())
}

fn reorder_component_children(
    root: &mut Component,
    component_id: &str,
    order: &[String],
) -> PactResult<()> {
    let parent = component_by_id_mut(root, component_id)
        .ok_or_else(|| PactError::corrupt(format!("node not found: {component_id}")))?;
    let mut old_children = std::mem::take(&mut parent.children);
    let mut reordered = Vec::with_capacity(old_children.len());
    for component_id in order {
        if let Some(index) = old_children
            .iter()
            .position(|child| child.id == component_id.as_str())
        {
            reordered.push(old_children.remove(index));
        }
    }
    reordered.extend(old_children);
    parent.children = reordered;
    Ok(())
}

pub(crate) fn project_branch_component_window(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
    component_id: Option<&str>,
    limit: Option<usize>,
) -> PactResult<Vec<Component>> {
    let target_revision = revision.unwrap_or(branch.head_revision);
    if target_revision == branch.current.head_revision && branch.current.component_count() > 0 {
        if let Some(component_id) = component_id {
            return Ok(branch
                .current
                .component(component_id)
                .cloned()
                .into_iter()
                .collect());
        }
        let mut components = Vec::new();
        collect_component_window(&branch.current.tree.root, limit, &mut components);
        return Ok(components);
    }
    let tree = project_branch_component_tree(root_store, branch, revision)?;
    let mut components = Vec::new();
    match component_id {
        Some(component_id) => {
            if let Some(component) = tree.component(component_id) {
                components.push(component.clone());
            }
        }
        None => collect_component_window(tree.root(), limit, &mut components),
    }
    Ok(components)
}

fn collect_component_window(component: &Component, limit: Option<usize>, out: &mut Vec<Component>) {
    if limit.map(|limit| out.len() >= limit).unwrap_or(false) {
        return;
    }
    out.push(component.clone());
    for child in &component.children {
        collect_component_window(child, limit, out);
        if limit.map(|limit| out.len() >= limit).unwrap_or(false) {
            break;
        }
    }
}

fn project_branch_archive_component_tree_inner(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    revision: Option<u64>,
    seen_branches: &mut BTreeSet<String>,
) -> PactResult<(ComponentTree, BTreeMap<String, u64>)> {
    if !seen_branches.insert(branch.branch_id.clone()) {
        return Err(PactError::storage(format!(
            "cyclic fork lineage at branch {}",
            branch.branch_id
        )));
    }

    let target_revision = revision.unwrap_or(branch.head_revision);
    let (mut tree, mut first_seen, replay_start) =
        if branch_has_trace_replay_at(branch, target_revision) {
            archive_component_base_from_transaction_traces(
                root_store,
                branch,
                target_revision,
                seen_branches,
            )?
        } else if target_revision == branch.current.head_revision
            && branch.current.component_count() > 0
        {
            let mut first_seen = branch.current.tree.first_seen.clone();
            collect_first_seen_components(&branch.current.tree.root, 0, &mut first_seen);
            (
                ComponentTree::from_root(branch.current.tree.root.clone())?,
                first_seen,
                branch.current.head_revision,
            )
        } else if let Some(content_ref) = branch.base_ref.content_ref.as_deref() {
            let tree = component_tree_from_base_ref(root_store, content_ref)?;
            let mut first_seen = BTreeMap::new();
            collect_first_seen_components(tree.root(), 0, &mut first_seen);
            (tree, first_seen, trace_replay_base_revision(branch))
        } else if let Some(fork_ref) = branch.fork_ref.as_ref()
            && let Some(parent_branch) = root_store.branches.get(&fork_ref.branch_id)
        {
            let parent_revision = fork_ref.revision.unwrap_or(parent_branch.head_revision);
            let (tree, first_seen) = project_branch_archive_component_tree_inner(
                root_store,
                parent_branch,
                Some(parent_revision),
                seen_branches,
            )?;
            (tree, first_seen, 0)
        } else {
            let tree = project_branch_root_skeleton_component_tree(branch)?;
            let mut first_seen = BTreeMap::new();
            collect_first_seen_components(tree.root(), 0, &mut first_seen);
            (tree, first_seen, 0)
        };

    if branch_has_trace_replay_at(branch, target_revision) {
        replay_branch_archive_component_transaction_traces_from(
            &mut tree,
            branch,
            replay_start,
            target_revision,
            &mut first_seen,
        )?;
    }

    seen_branches.remove(&branch.branch_id);

    let mut visible_in_head = BTreeSet::new();
    let head_tree = project_branch_component_tree(root_store, branch, Some(target_revision))?;
    collect_component_ids(head_tree.root(), &mut visible_in_head);
    tree.edit(|root| {
        annotate_archive_components(root, &first_seen, &visible_in_head, target_revision)
    })?;
    Ok((tree, first_seen))
}

fn component_tree_from_base_ref(
    root_store: &ContextTreeLedger,
    content_ref: &str,
) -> PactResult<ComponentTree> {
    let record = root_store.content.records.get(content_ref).ok_or_else(|| {
        PactError::not_found(format!("missing branch base content record: {content_ref}"))
    })?;
    let base_json = record.storage.inline.as_deref().ok_or_else(|| {
        PactError::storage(format!(
            "branch base content is not inline yet: {content_ref}"
        ))
    })?;
    let mut base: Value = serde_json::from_str(base_json)
        .map_err(|err| PactError::codec(format!("invalid branch base PACT JSON: {err}")))?;
    hydrate_content_refs_in_value(&mut base, &root_store.content);
    ComponentTree::from_value(&base)
}

fn archive_component_base_from_transaction_traces(
    root_store: &ContextTreeLedger,
    branch: &PactBranch,
    target_revision: u64,
    seen_branches: &mut BTreeSet<String>,
) -> PactResult<(ComponentTree, BTreeMap<String, u64>, u64)> {
    if let Some(content_ref) = branch.base_ref.content_ref.as_deref() {
        let tree = component_tree_from_base_ref(root_store, content_ref)?;
        let mut first_seen = BTreeMap::new();
        collect_first_seen_components(tree.root(), 0, &mut first_seen);
        Ok((tree, first_seen, trace_replay_base_revision(branch)))
    } else if let Some(fork_ref) = branch.fork_ref.as_ref()
        && let Some(parent_branch) = root_store.branches.get(&fork_ref.branch_id)
    {
        let parent_revision = fork_ref.revision.unwrap_or(parent_branch.head_revision);
        let (tree, first_seen) = project_branch_archive_component_tree_inner(
            root_store,
            parent_branch,
            Some(parent_revision),
            seen_branches,
        )?;
        Ok((tree, first_seen, 0))
    } else if target_revision == branch.current.head_revision
        && branch.current.component_count() > 0
    {
        let mut first_seen = branch.current.tree.first_seen.clone();
        collect_first_seen_components(&branch.current.tree.root, 0, &mut first_seen);
        Ok((
            ComponentTree::from_root(branch.current.tree.root.clone())?,
            first_seen,
            branch.current.head_revision,
        ))
    } else {
        let tree = project_branch_root_skeleton_component_tree(branch)?;
        let mut first_seen = BTreeMap::new();
        collect_first_seen_components(tree.root(), 0, &mut first_seen);
        Ok((tree, first_seen, 0))
    }
}

fn replay_branch_archive_component_transaction_traces_from(
    tree: &mut ComponentTree,
    branch: &PactBranch,
    start_revision: u64,
    target_revision: u64,
    first_seen: &mut BTreeMap<String, u64>,
) -> PactResult<()> {
    let mut traces = branch
        .transaction_traces
        .iter()
        .filter(|trace| trace.to_rev > start_revision && trace.to_rev <= target_revision)
        .collect::<Vec<_>>();
    if traces.is_empty() {
        return Ok(());
    }
    traces.sort_by_key(|trace| (trace.to_rev, trace.from_rev));
    tree.edit(|root| {
        for trace in traces {
            for op in &trace.ops {
                apply_archive_component_trace_op_in(root, trace, op)?;
                collect_first_seen_components(root, trace.to_rev, first_seen);
            }
        }
        Ok(())
    })
}

fn apply_archive_component_trace_op_in(
    root: &mut Component,
    trace: &TransactionTrace,
    op: &TraceOp,
) -> PactResult<()> {
    if matches!(op.kind, TraceOpKind::DeleteComponent) {
        return mark_deleted_component_subtree(root, &op.component_id, trace.to_rev)
            .map(|_| ())
            .ok_or_else(|| {
                PactError::corrupt(format!("delete: component not found: {}", op.component_id))
            });
    }
    apply_component_trace_op_in(root, op)
}

fn collect_first_seen_components(
    component: &Component,
    revision: u64,
    first_seen: &mut BTreeMap<String, u64>,
) {
    first_seen.entry(component.id.clone()).or_insert(revision);
    for child in &component.children {
        collect_first_seen_components(child, revision, first_seen);
    }
}

fn collect_component_ids(component: &Component, ids: &mut BTreeSet<String>) {
    ids.insert(component.id.clone());
    for child in &component.children {
        collect_component_ids(child, ids);
    }
}

fn mark_deleted_component_subtree(
    component: &mut Component,
    component_id: &str,
    revision: u64,
) -> Option<()> {
    if component.id == component_id {
        mark_component_deleted(component, revision);
        return Some(());
    }
    for child in &mut component.children {
        if mark_deleted_component_subtree(child, component_id, revision).is_some() {
            return Some(());
        }
    }
    None
}

fn mark_component_deleted(component: &mut Component, revision: u64) {
    component.aux.insert(
        "pact_lifecycle".to_string(),
        Value::String("deleted".to_string()),
    );
    component
        .aux
        .insert("visible_in_head".to_string(), Value::Bool(false));
    component
        .aux
        .insert("deleted_at".to_string(), Value::Number(revision.into()));
    component
        .aux
        .insert("last_seen".to_string(), Value::Number(revision.into()));
    for child in &mut component.children {
        mark_component_deleted(child, revision);
    }
}

fn annotate_archive_components(
    component: &mut Component,
    first_seen: &BTreeMap<String, u64>,
    visible_in_head: &BTreeSet<String>,
    target_revision: u64,
) -> PactResult<()> {
    let first_seen_value = first_seen.get(&component.id).copied().unwrap_or_default();
    component.aux.insert(
        "first_seen".to_string(),
        Value::Number(first_seen_value.into()),
    );
    if visible_in_head.contains(&component.id) {
        component.aux.insert(
            "pact_lifecycle".to_string(),
            Value::String("active".to_string()),
        );
        component
            .aux
            .insert("visible_in_head".to_string(), Value::Bool(true));
        component.aux.insert(
            "last_seen".to_string(),
            Value::Number(target_revision.into()),
        );
        component.aux.remove("deleted_at");
    } else {
        component
            .aux
            .entry("pact_lifecycle".to_string())
            .or_insert_with(|| Value::String("deleted".to_string()));
        component
            .aux
            .entry("visible_in_head".to_string())
            .or_insert(Value::Bool(false));
        component
            .aux
            .entry("last_seen".to_string())
            .or_insert_with(|| Value::Number(target_revision.into()));
    }
    for child in &mut component.children {
        annotate_archive_components(child, first_seen, visible_in_head, target_revision)?;
    }
    Ok(())
}

fn branch_has_trace_replay_at(branch: &PactBranch, target_revision: u64) -> bool {
    branch_has_trace_replay_between(branch, trace_replay_base_revision(branch), target_revision)
}

fn trace_replay_base_revision(branch: &PactBranch) -> u64 {
    if branch.base_ref.content_ref.is_some() {
        branch.base_ref.revision.unwrap_or(0)
    } else {
        0
    }
}

fn branch_has_trace_replay_between(
    branch: &PactBranch,
    start_revision: u64,
    target_revision: u64,
) -> bool {
    if target_revision == start_revision {
        return true;
    }
    if target_revision < start_revision {
        return false;
    }
    if target_revision == 0 {
        return false;
    }
    let mut traces = branch
        .transaction_traces
        .iter()
        .filter(|trace| trace.to_rev <= target_revision)
        .collect::<Vec<_>>();
    traces.sort_by_key(|trace| (trace.to_rev, trace.from_rev));
    let mut covered_rev = start_revision;
    for trace in traces {
        if trace.to_rev <= covered_rev {
            continue;
        }
        if trace.from_rev != covered_rev {
            return false;
        }
        covered_rev = trace.to_rev;
        if covered_rev >= target_revision {
            return true;
        }
    }
    false
}

fn snapshot_component_records(branch: &PactBranch) -> BTreeMap<String, ComponentRecord> {
    let mut nodes = branch.current.component_records();
    for node in nodes.values_mut() {
        if let Some(policy) = node.content_capture
            && !policy.persists_content()
        {
            node.content = None;
            node.content_ref = None;
            node.content_preview = None;
        }
    }
    nodes
}

fn project_branch_birth_skeleton_records(
    branch: &PactBranch,
) -> PactResult<(String, BTreeMap<String, ComponentRecord>)> {
    let mut nodes = snapshot_component_records(branch);
    if nodes.is_empty() {
        return Err(PactError::storage(format!(
            "branch {} has no base content ref and no current component tree",
            branch.branch_id
        )));
    }
    let root_id = if nodes.contains_key(&branch.current.tree.root_id) {
        branch.current.tree.root_id.clone()
    } else {
        nodes
            .values()
            .find(|node| node.parent_id.is_none())
            .map(|node| node.id.clone())
            .ok_or_else(|| {
                PactError::storage(format!(
                    "branch {} has no base content ref and no root node record",
                    branch.branch_id
                ))
            })?
    };
    let trace_created = trace_created_component_ids(branch);
    nodes.retain(|component_id, _| {
        component_id == &root_id || !trace_created.contains(component_id)
    });

    loop {
        let survivors = nodes.keys().cloned().collect::<BTreeSet<_>>();
        let before = nodes.len();
        nodes.retain(|component_id, node| {
            component_id == &root_id
                || node
                    .parent_id
                    .as_ref()
                    .is_none_or(|parent_id| survivors.contains(parent_id))
        });
        if nodes.len() == before {
            break;
        }
    }

    let survivors = nodes.keys().cloned().collect::<BTreeSet<_>>();
    for node in nodes.values_mut() {
        node.child_ids
            .retain(|child_id| survivors.contains(child_id));
    }
    Ok((root_id, nodes))
}

fn trace_created_component_ids(branch: &PactBranch) -> BTreeSet<String> {
    branch
        .transaction_traces
        .iter()
        .flat_map(|trace| trace.ops.iter())
        .filter_map(|op| match op.kind {
            TraceOpKind::InsertComponent { .. } => Some(op.component_id.clone()),
            TraceOpKind::DeleteComponent
            | TraceOpKind::MoveComponent { .. }
            | TraceOpKind::SetField { .. }
            | TraceOpKind::UnsetField { .. }
            | TraceOpKind::SetAttr { .. }
            | TraceOpKind::UnsetAttr { .. }
            | TraceOpKind::SetLifecycle { .. }
            | TraceOpKind::SetContent { .. }
            | TraceOpKind::ReplaceContentRange { .. } => None,
        })
        .collect()
}

fn project_node_record_tree(
    component_id: &str,
    nodes: &BTreeMap<String, ComponentRecord>,
) -> PactResult<Value> {
    let node = nodes
        .get(component_id)
        .ok_or_else(|| PactError::not_found(format!("missing node record: {component_id}")))?;
    let mut value = serde_json::to_value(node)
        .map_err(|err| PactError::codec(format!("failed to encode node {component_id}: {err}")))?;
    let object = value.as_object_mut().ok_or_else(|| {
        PactError::storage(format!(
            "node record did not encode as object: {component_id}"
        ))
    })?;
    object.remove("parent_id");
    object.remove("child_ids");
    let children = node
        .child_ids
        .iter()
        .map(|child_id| project_node_record_tree(child_id, nodes))
        .collect::<Result<Vec<_>, _>>()?;
    if !children.is_empty() {
        object.insert("children".to_string(), Value::Array(children));
    }
    Ok(value)
}

pub(super) fn hydrate_content_refs_in_value(value: &mut Value, content: &ContentStore) {
    match value {
        Value::Object(object) => {
            hydrate_text_ref_slot(object, "content_ref", "content", content);
            hydrate_json_ref_slot(object, "parameters_ref", "parameters", content);
            hydrate_json_ref_slot(object, "result_ref", "result", content);
            hydrate_text_ref_slot(object, "error_ref", "error_message", content);
            hydrate_media_ref_slot(object, content);
            for child in object.values_mut() {
                hydrate_content_refs_in_value(child, content);
            }
        }
        Value::Array(items) => {
            for item in items {
                hydrate_content_refs_in_value(item, content);
            }
        }
        _ => {}
    }
}

fn hydrate_media_ref_slot(object: &mut Map<String, Value>, content: &ContentStore) {
    let Some(aux) = object.get_mut("aux").and_then(Value::as_object_mut) else {
        return;
    };
    if aux.get("media_content").is_some() {
        return;
    }
    if let Some(bytes) = aux
        .get("media_content_ref")
        .and_then(content_ref_id_from_value)
        .and_then(|content_id| content_bytes(content, &content_id))
    {
        aux.insert(
            "media_content".to_string(),
            serde_json::json!({
                "data": base64_encode(&bytes),
                "encoding": "base64",
            }),
        );
        aux.remove("media_content_ref");
        aux.remove("media_data_ref");
    }
}

fn content_bytes(content: &ContentStore, content_id: &str) -> Option<Vec<u8>> {
    let record = content.records.get(content_id)?;
    record.storage.inline_bytes.clone().or_else(|| {
        record
            .storage
            .inline
            .as_ref()
            .map(|text| text.as_bytes().to_vec())
    })
}

fn hydrate_text_ref_slot(
    object: &mut Map<String, Value>,
    ref_key: &str,
    target_key: &str,
    content: &ContentStore,
) {
    if let Some(text) = object
        .get(ref_key)
        .and_then(content_ref_id_from_value)
        .and_then(|content_id| content_text(content, &content_id).map(str::to_string))
    {
        object.insert(target_key.to_string(), Value::String(text));
        object.remove(ref_key);
        if ref_key == "content_ref" {
            object.remove("content_preview");
        }
    }
}

fn hydrate_json_ref_slot(
    object: &mut Map<String, Value>,
    ref_key: &str,
    target_key: &str,
    content: &ContentStore,
) {
    let Some(text) = object
        .get(ref_key)
        .and_then(content_ref_id_from_value)
        .and_then(|content_id| content_text(content, &content_id).map(str::to_string))
    else {
        return;
    };
    let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    object.insert(target_key.to_string(), value);
    object.remove(ref_key);
}

fn content_text<'a>(content: &'a ContentStore, content_id: &str) -> Option<&'a str> {
    content
        .records
        .get(content_id)
        .and_then(|record| record.storage.inline.as_deref())
}

pub(super) fn parse_aux(aux_json: Option<&str>) -> PactResult<BTreeMap<String, Value>> {
    let Some(aux_json) = aux_json else {
        return Ok(BTreeMap::new());
    };
    if aux_json.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(aux_json)
        .map_err(|err| PactError::codec(format!("invalid branch aux JSON: {err}")))
}

pub(super) fn branch_inspection_value(branch: &PactBranch) -> Value {
    serde_json::json!({
        "branch_id": &branch.branch_id,
        "root_id": &branch.root_id,
        "parent_branch_id": &branch.parent_branch_id,
        "fork_ref": &branch.fork_ref,
        "head_revision": branch.head_revision,
        "lifecycle_tick": branch.lifecycle_tick,
        "message_count": branch.current.message_order.len(),
        "tool_pair_count": branch.current.tree.tool_pairs.len(),
        "node_count": branch.current.component_count(),
        "content_ref_count": branch.current.content_refs.len(),
        "stats": &branch.current.tree.stats,
        "aux": &branch.aux,
    })
}

pub(crate) fn next_branch_id(
    root_store: &ContextTreeLedger,
    from_branch_id: &str,
    revision: u64,
) -> String {
    let mut index = 1_u64;
    loop {
        let candidate = format!("{from_branch_id}_fork_{revision}_{index}");
        if !root_store.branches.contains_key(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

pub(super) fn encode_store(store: &PactStore) -> PactResult<String> {
    serde_json::to_string(store)
        .map_err(|err| PactError::codec(format!("failed to encode PACT store: {err}")))
}

pub(super) fn encode_root_store(store: &ContextTreeLedger) -> PactResult<String> {
    serde_json::to_string(store)
        .map_err(|err| PactError::codec(format!("failed to encode PACT root store: {err}")))
}
