use super::TransactionTrace;
use super::trace_op::TraceOpKind;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn node_type_of(node: &Value) -> Option<String> {
    node.get("componentType")
        .or_else(|| node.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransactionTraceIndex {
    pub touched_by_revision: BTreeMap<u64, BTreeSet<String>>,

    pub revisions_by_node: BTreeMap<String, Vec<u64>>,

    pub inserted_at: BTreeMap<String, u64>,

    pub deleted_at: BTreeMap<String, u64>,

    pub lifecycle: BTreeMap<String, Vec<(u64, bool)>>,

    pub nodes_by_type: BTreeMap<String, BTreeSet<String>>,

    pub children_by_parent: BTreeMap<String, BTreeSet<String>>,
}

impl TransactionTraceIndex {
    pub fn build(traces: &[TransactionTrace]) -> Self {
        let mut index = TransactionTraceIndex::default();

        let mut parent_of: BTreeMap<String, String> = BTreeMap::new();
        let mut type_of: BTreeMap<String, String> = BTreeMap::new();

        for trace in traces {
            let rev = trace.to_rev;
            for op in &trace.ops {
                let component_id = &op.component_id;
                index
                    .touched_by_revision
                    .entry(rev)
                    .or_default()
                    .insert(component_id.clone());
                let revs = index
                    .revisions_by_node
                    .entry(component_id.clone())
                    .or_default();
                if revs.last() != Some(&rev) {
                    revs.push(rev);
                }
                match &op.kind {
                    TraceOpKind::InsertComponent {
                        parent_id,
                        component,
                        ..
                    } => {
                        index.inserted_at.entry(component_id.clone()).or_insert(rev);

                        index.deleted_at.remove(component_id);
                        index
                            .lifecycle
                            .entry(component_id.clone())
                            .or_default()
                            .push((rev, true));
                        reparent(&mut index, &mut parent_of, component_id, parent_id);
                        if let Some(ty) = node_type_of(component) {
                            set_type(&mut index, &mut type_of, component_id, ty);
                        }
                    }
                    TraceOpKind::MoveComponent { parent_id, .. } => {
                        reparent(&mut index, &mut parent_of, component_id, parent_id);
                    }
                    TraceOpKind::SetField { key, value }
                        if key == "componentType" || key == "type" =>
                    {
                        if let Some(ty) = value.as_str() {
                            set_type(&mut index, &mut type_of, component_id, ty.to_string());
                        }
                    }
                    TraceOpKind::DeleteComponent => {
                        unindex_node(&mut index, &mut parent_of, &mut type_of, component_id, rev);
                    }
                    _ => {}
                }
            }
        }
        index
    }

    pub fn nodes_of_type(&self, node_type: &str) -> BTreeSet<String> {
        self.nodes_by_type
            .get(node_type)
            .cloned()
            .unwrap_or_default()
    }

    pub fn children_of(&self, parent_id: &str) -> BTreeSet<String> {
        self.children_by_parent
            .get(parent_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn touched_in_window(&self, from: u64, to: u64) -> BTreeSet<String> {
        self.touched_by_revision
            .range((
                core::ops::Bound::Excluded(from),
                core::ops::Bound::Included(to),
            ))
            .flat_map(|(_, ids)| ids.iter().cloned())
            .collect()
    }

    pub fn is_live_at(&self, component_id: &str, rev: u64) -> bool {
        match self.lifecycle.get(component_id) {
            Some(events) => {
                let mut alive = false;
                for &(at, is_alive) in events {
                    if at <= rev {
                        alive = is_alive;
                    } else {
                        break;
                    }
                }
                alive
            }
            None => false,
        }
    }
}

fn reparent(
    index: &mut TransactionTraceIndex,
    parent_of: &mut BTreeMap<String, String>,
    component_id: &str,
    new_parent: &str,
) {
    if let Some(old_parent) = parent_of.get(component_id)
        && old_parent != new_parent
        && let Some(children) = index.children_by_parent.get_mut(old_parent)
    {
        children.remove(component_id);
    }
    parent_of.insert(component_id.to_string(), new_parent.to_string());
    index
        .children_by_parent
        .entry(new_parent.to_string())
        .or_default()
        .insert(component_id.to_string());
}

fn set_type(
    index: &mut TransactionTraceIndex,
    type_of: &mut BTreeMap<String, String>,
    component_id: &str,
    ty: String,
) {
    if let Some(old) = type_of.get(component_id)
        && old != &ty
        && let Some(set) = index.nodes_by_type.get_mut(old)
    {
        set.remove(component_id);
    }
    type_of.insert(component_id.to_string(), ty.clone());
    index
        .nodes_by_type
        .entry(ty)
        .or_default()
        .insert(component_id.to_string());
}

fn unindex_node(
    index: &mut TransactionTraceIndex,
    parent_of: &mut BTreeMap<String, String>,
    type_of: &mut BTreeMap<String, String>,
    component_id: &str,
    rev: u64,
) {
    let mut subtree = Vec::new();
    let mut stack = vec![component_id.to_string()];
    while let Some(current) = stack.pop() {
        if let Some(children) = index.children_by_parent.get(&current) {
            stack.extend(children.iter().cloned());
        }
        subtree.push(current);
    }
    for node in subtree {
        index.deleted_at.entry(node.clone()).or_insert(rev);
        index
            .lifecycle
            .entry(node.clone())
            .or_default()
            .push((rev, false));
        if let Some(parent) = parent_of.remove(&node)
            && let Some(children) = index.children_by_parent.get_mut(&parent)
        {
            children.remove(&node);
        }
        if let Some(ty) = type_of.remove(&node)
            && let Some(set) = index.nodes_by_type.get_mut(&ty)
        {
            set.remove(&node);
        }
        index.children_by_parent.remove(&node);
    }
}

#[cfg(test)]
mod tests;
