mod content_op;
pub(crate) mod encode;
mod index;
mod pack;
pub(crate) mod stream_codec;
mod surface;
mod trace;
mod trace_op;

#[cfg(test)]
mod tests;

pub use encode::{
    decode_transaction_trace, decode_transaction_trace_with, encode_transaction_trace,
};
pub use index::TransactionTraceIndex;
#[cfg(feature = "compression-zstd")]
pub use pack::{
    compress_pack, compress_pack_with_dict, decompress_pack, decompress_pack_with_dict,
};
pub use pack::{pack_transaction_traces, unpack_transaction_traces};
pub use trace::{RevisionTrace, derive_revision_trace, derive_trace};
pub use trace_op::{ComponentChange, TraceOp, TraceOpKind};

use crate::error::{PactError, PactResult};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use surface::{NodeLocation, children, component_id, index_surface, node_without_children};
use trace_op::{
    ATTRS_KEY, CHILDREN_KEY, CONTENT_KEY, CONTENT_PREVIEW_KEY, CONTENT_REF_KEY, ID_KEY,
    LIFECYCLE_KEY,
};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransactionTrace {
    pub from_rev: u64,
    pub to_rev: u64,
    #[serde(default, alias = "node_deltas")]
    pub ops: Vec<TraceOp>,
}

impl TransactionTrace {
    pub fn diff(from_rev: u64, to_rev: u64, before: &Value, after: &Value) -> Self {
        Self {
            from_rev,
            to_rev,
            ops: diff_surface(before, after),
        }
    }

    pub fn replay(&self, before: &Value) -> PactResult<Value> {
        self.validate()?;
        replay(before, &self.ops)
    }

    pub(crate) fn replay_owned(&self, tree: Value) -> PactResult<Value> {
        self.validate()?;
        replay_owned(tree, &self.ops)
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn validate(&self) -> PactResult<()> {
        if self.to_rev < self.from_rev {
            return Err(PactError::invalid(format!(
                "transaction trace to_rev {} is before from_rev {}",
                self.to_rev, self.from_rev
            )));
        }
        for op in &self.ops {
            op.validate()?;
        }
        Ok(())
    }
}

pub fn normalize_for_transaction_trace(tree: &Value) -> Value {
    fn walk(value: &mut Value) {
        if let Some(object) = value.as_object_mut() {
            for key in [
                "depth",
                "pact_depth",
                "child_ids",
                "visible_in_head",
                "first_seen",
                "last_seen",
                "deleted_at",
                "pact_lifecycle",
            ] {
                object.remove(key);
            }
            if let Some(attrs) = object.get_mut(ATTRS_KEY).and_then(Value::as_object_mut) {
                for key in [
                    "visible_in_head",
                    "first_seen",
                    "last_seen",
                    "deleted_at",
                    "pact_lifecycle",
                ] {
                    attrs.remove(key);
                }
            }
            if let Some(children) = object
                .get_mut(trace_op::CHILDREN_KEY)
                .and_then(Value::as_array_mut)
            {
                for child in children {
                    walk(child);
                }
            }
        }
    }

    let mut normalized = tree.clone();
    walk(&mut normalized);
    normalized
}

pub fn diff_surface(before: &Value, after: &Value) -> Vec<TraceOp> {
    let before_idx = index_surface(before);
    let after_idx = index_surface(after);

    let mut ops = Vec::new();
    let root = component_id(after);

    if let Some(root_id) = &root {
        for id in after_idx.keys() {
            let Some(before_loc) = before_idx.get(id) else {
                continue;
            };
            let rescued = before_loc
                .parent_id
                .as_ref()
                .is_some_and(|before_parent| !after_idx.contains_key(before_parent));

            if rescued && before_loc.parent_id.as_deref() != Some(root_id.as_str()) {
                ops.push(TraceOp::new(
                    id.clone(),
                    TraceOpKind::MoveComponent {
                        parent_id: root_id.clone(),
                        index: 0,
                    },
                ));
            }
        }
    }

    for (id, loc) in &before_idx {
        if after_idx.contains_key(id) {
            continue;
        }
        let parent_still_present = match &loc.parent_id {
            Some(parent) => after_idx.contains_key(parent),
            None => true,
        };
        if parent_still_present {
            ops.push(TraceOp::new(id.clone(), TraceOpKind::DeleteComponent));
        }
    }

    for (id, after_loc) in &after_idx {
        let Some(before_loc) = before_idx.get(id) else {
            continue;
        };
        surface_field_ops(id, before_loc.node, after_loc.node, &mut ops);
    }

    let mut structural: Vec<(usize, usize, TraceOp)> = Vec::new();
    for (id, after_loc) in &after_idx {
        let Some(parent_id) = &after_loc.parent_id else {
            continue;
        };
        match before_idx.get(id) {
            None => {
                structural.push((
                    after_loc.depth,
                    after_loc.index,
                    TraceOp::new(
                        id.clone(),
                        TraceOpKind::InsertComponent {
                            parent_id: parent_id.clone(),
                            index: after_loc.index,
                            component: node_without_children(after_loc.node),
                        },
                    ),
                ));
            }
            Some(before_loc) => {
                if needs_move(parent_id, before_loc, &before_idx, &after_idx) {
                    structural.push((
                        after_loc.depth,
                        after_loc.index,
                        TraceOp::new(
                            id.clone(),
                            TraceOpKind::MoveComponent {
                                parent_id: parent_id.clone(),
                                index: after_loc.index,
                            },
                        ),
                    ));
                }
            }
        }
    }
    structural.sort_by_key(|(depth, index, _)| (*depth, *index));
    ops.extend(structural.into_iter().map(|(_, _, op)| op));

    ops
}

fn needs_move(
    after_parent: &str,
    before_loc: &NodeLocation<'_>,
    before_idx: &BTreeMap<String, NodeLocation<'_>>,
    after_idx: &BTreeMap<String, NodeLocation<'_>>,
) -> bool {
    if before_loc.parent_id.as_deref() != Some(after_parent) {
        return true;
    }
    parent_survivors_reordered(after_parent, before_idx, after_idx)
}

fn parent_survivors_reordered(
    parent: &str,
    before_idx: &BTreeMap<String, NodeLocation<'_>>,
    after_idx: &BTreeMap<String, NodeLocation<'_>>,
) -> bool {
    let survives_here = |id: &str| {
        before_idx.get(id).and_then(|l| l.parent_id.as_deref()) == Some(parent)
            && after_idx.get(id).and_then(|l| l.parent_id.as_deref()) == Some(parent)
    };
    let after_order: Vec<String> = after_idx
        .get(parent)
        .map(|loc| child_ids(loc.node))
        .unwrap_or_default()
        .into_iter()
        .filter(|id| survives_here(id))
        .collect();
    let before_order: Vec<String> = before_idx
        .get(parent)
        .map(|loc| child_ids(loc.node))
        .unwrap_or_default()
        .into_iter()
        .filter(|id| survives_here(id))
        .collect();
    after_order != before_order
}

fn child_ids(node: &Value) -> Vec<String> {
    children(node).iter().filter_map(component_id).collect()
}

fn surface_field_ops(component_id: &str, before: &Value, after: &Value, out: &mut Vec<TraceOp>) {
    let empty = Map::new();
    let before_obj = before.as_object().unwrap_or(&empty);
    let after_obj = after.as_object().unwrap_or(&empty);

    for (key, after_val) in after_obj {
        if trace_op::is_reserved_field(key) {
            continue;
        }
        if before_obj.get(key) != Some(after_val) {
            out.push(TraceOp::new(
                component_id,
                TraceOpKind::SetField {
                    key: key.clone(),
                    value: after_val.clone(),
                },
            ));
        }
    }
    for key in before_obj.keys() {
        if trace_op::is_reserved_field(key) {
            continue;
        }
        if !after_obj.contains_key(key) {
            out.push(TraceOp::new(
                component_id,
                TraceOpKind::UnsetField { key: key.clone() },
            ));
        }
    }

    let before_attrs = before_obj.get(ATTRS_KEY).and_then(Value::as_object);
    let after_attrs = after_obj.get(ATTRS_KEY).and_then(Value::as_object);
    if before_attrs != after_attrs {
        let empty_attrs = Map::new();
        let ba = before_attrs.unwrap_or(&empty_attrs);
        let aa = after_attrs.unwrap_or(&empty_attrs);
        for (key, val) in aa {
            if ba.get(key) != Some(val) {
                out.push(TraceOp::new(
                    component_id,
                    TraceOpKind::SetAttr {
                        key: key.clone(),
                        value: val.clone(),
                    },
                ));
            }
        }
        for key in ba.keys() {
            if !aa.contains_key(key) {
                out.push(TraceOp::new(
                    component_id,
                    TraceOpKind::UnsetAttr { key: key.clone() },
                ));
            }
        }
    }

    if before_obj.get(LIFECYCLE_KEY) != after_obj.get(LIFECYCLE_KEY) {
        if let Some(value) = after_obj.get(LIFECYCLE_KEY) {
            out.push(TraceOp::new(
                component_id,
                TraceOpKind::SetLifecycle {
                    value: value.clone(),
                },
            ));
        } else {
            out.push(TraceOp::new(
                component_id,
                TraceOpKind::UnsetField {
                    key: LIFECYCLE_KEY.to_string(),
                },
            ));
        }
    }

    if let Some(op) = content_op::content_op(component_id, before, after) {
        out.push(op);
    }

    let before_children = before_obj.get(trace_op::CHILDREN_KEY);
    let after_children = after_obj.get(trace_op::CHILDREN_KEY);
    match (before_children, after_children) {
        (None, Some(value)) if value.as_array().is_some_and(|a| a.is_empty()) => {
            out.push(TraceOp::new(
                component_id,
                TraceOpKind::SetField {
                    key: trace_op::CHILDREN_KEY.to_string(),
                    value: value.clone(),
                },
            ));
        }
        (Some(_), None) => {
            out.push(TraceOp::new(
                component_id,
                TraceOpKind::UnsetField {
                    key: trace_op::CHILDREN_KEY.to_string(),
                },
            ));
        }
        _ => {}
    }
}

fn replay(before: &Value, ops: &[TraceOp]) -> PactResult<Value> {
    replay_owned(before.clone(), ops)
}

pub(crate) fn replay_owned(tree: Value, ops: &[TraceOp]) -> PactResult<Value> {
    replay_with_index(tree, ops, PathIndex::enabled())
}

fn replay_with_index(mut tree: Value, ops: &[TraceOp], mut index: PathIndex) -> PactResult<Value> {
    for op in ops {
        apply_op(&mut tree, &mut index, op)?;
        if is_structural(op) {
            index.invalidate();
        }
    }
    Ok(tree)
}

struct PathIndex {
    enabled: bool,
    paths: Option<HashMap<String, Vec<usize>>>,
}

impl PathIndex {
    fn enabled() -> Self {
        Self {
            enabled: true,
            paths: None,
        }
    }

    fn invalidate(&mut self) {
        self.paths = None;
    }

    fn locate<'a>(&mut self, tree: &'a mut Value, target: &str) -> Option<&'a mut Value> {
        if !self.enabled {
            return find_node_mut(tree, target);
        }
        if self.paths.is_none() {
            let mut paths = HashMap::new();
            collect_paths(tree, &mut Vec::new(), &mut paths);
            self.paths = Some(paths);
        }
        let path = self.paths.as_ref()?.get(target)?.clone();
        let mut node = tree;
        for index in path {
            node = node.get_mut("children")?.as_array_mut()?.get_mut(index)?;
        }
        (node.get("id").and_then(Value::as_str) == Some(target)).then_some(node)
    }
}

fn collect_paths(node: &Value, path: &mut Vec<usize>, paths: &mut HashMap<String, Vec<usize>>) {
    if let Some(id) = node.get("id").and_then(Value::as_str) {
        paths.entry(id.to_string()).or_insert_with(|| path.clone());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for (index, child) in children.iter().enumerate() {
            path.push(index);
            collect_paths(child, path, paths);
            path.pop();
        }
    }
}

fn is_structural(op: &TraceOp) -> bool {
    match &op.kind {
        TraceOpKind::InsertComponent { .. }
        | TraceOpKind::DeleteComponent
        | TraceOpKind::MoveComponent { .. } => true,
        TraceOpKind::SetField { key, .. } | TraceOpKind::UnsetField { key } => {
            key == "children" || key == "child_ids" || key == "id"
        }
        _ => false,
    }
}

fn apply_op(tree: &mut Value, index: &mut PathIndex, op: &TraceOp) -> PactResult<()> {
    let id = &op.component_id;
    match &op.kind {
        TraceOpKind::DeleteComponent => detach_subtree(tree, id)
            .map(|_| ())
            .ok_or_else(|| PactError::corrupt(format!("delete: node not found: {id}"))),
        TraceOpKind::InsertComponent {
            parent_id,
            index,
            component,
        } => insert_child(tree, parent_id, *index, component.clone()),
        TraceOpKind::MoveComponent { parent_id, index } => {
            let subtree = detach_subtree(tree, id)
                .ok_or_else(|| PactError::corrupt(format!("move: node not found: {id}")))?;
            insert_child(tree, parent_id, *index, subtree)
        }
        TraceOpKind::SetField { key, value } => with_node_object(tree, index, id, |obj| {
            obj.insert(key.clone(), value.clone());
            if key == "child_ids" {
                reorder_children_to_child_ids(obj, value);
            }
        }),
        TraceOpKind::UnsetField { key } => with_node_object(tree, index, id, |obj| {
            obj.remove(key);
        }),
        TraceOpKind::SetAttr { key, value } => with_node_object(tree, index, id, |obj| {
            let attrs = obj
                .entry(ATTRS_KEY.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(map) = attrs.as_object_mut() {
                map.insert(key.clone(), value.clone());
            }
        }),
        TraceOpKind::UnsetAttr { key } => with_node_object(tree, index, id, |obj| {
            if let Some(map) = obj.get_mut(ATTRS_KEY).and_then(Value::as_object_mut) {
                map.remove(key);
            }
        }),
        TraceOpKind::SetLifecycle { value } => with_node_object(tree, index, id, |obj| {
            obj.insert(LIFECYCLE_KEY.to_string(), value.clone());
        }),
        TraceOpKind::SetContent {
            content,
            content_ref,
            content_preview,
        } => with_node_object(tree, index, id, |obj| {
            match content {
                Some(text) => {
                    obj.insert(CONTENT_KEY.to_string(), Value::String(text.clone()));
                }
                None => {
                    obj.remove(CONTENT_KEY);
                }
            }
            match content_ref {
                Some(value) => {
                    obj.insert(CONTENT_REF_KEY.to_string(), value.clone());
                }
                None => {
                    obj.remove(CONTENT_REF_KEY);
                }
            }
            match content_preview {
                Some(text) => {
                    obj.insert(CONTENT_PREVIEW_KEY.to_string(), Value::String(text.clone()));
                }
                None => {
                    obj.remove(CONTENT_PREVIEW_KEY);
                }
            }
        }),
        TraceOpKind::ReplaceContentRange {
            start,
            end,
            replacement,
        } => with_node_object(tree, index, id, |obj| {
            let current = obj
                .get(CONTENT_KEY)
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
                obj.insert(CONTENT_KEY.to_string(), Value::String(updated));
            }
        }),
    }
}

fn with_node_object<F: FnOnce(&mut Map<String, Value>)>(
    tree: &mut Value,
    index: &mut PathIndex,
    component_id: &str,
    f: F,
) -> PactResult<()> {
    let node = index
        .locate(tree, component_id)
        .ok_or_else(|| PactError::corrupt(format!("node not found: {component_id}")))?;
    let obj = node
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt(format!("node is not an object: {component_id}")))?;
    f(obj);
    Ok(())
}

fn reorder_children_to_child_ids(obj: &mut Map<String, Value>, value: &Value) {
    let Some(order) = value.as_array() else {
        return;
    };
    let order = order
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let Some(children) = obj.get_mut(CHILDREN_KEY).and_then(Value::as_array_mut) else {
        return;
    };

    let mut old_children = std::mem::take(children);
    let mut reordered = Vec::with_capacity(old_children.len());
    for component_id in order {
        if let Some(index) = old_children.iter().position(|child| {
            child.get(ID_KEY).and_then(Value::as_str) == Some(component_id.as_str())
        }) {
            reordered.push(old_children.remove(index));
        }
    }
    reordered.extend(old_children);
    *children = reordered;
}

fn find_node_mut<'a>(node: &'a mut Value, target: &str) -> Option<&'a mut Value> {
    if node.get("id").and_then(Value::as_str) == Some(target) {
        return Some(node);
    }
    let children = node.get_mut("children")?.as_array_mut()?;
    for child in children {
        if let Some(found) = find_node_mut(child, target) {
            return Some(found);
        }
    }
    None
}

fn detach_subtree(tree: &mut Value, target: &str) -> Option<Value> {
    let children = tree.get_mut("children")?.as_array_mut()?;
    if let Some(pos) = children
        .iter()
        .position(|child| child.get("id").and_then(Value::as_str) == Some(target))
    {
        return Some(children.remove(pos));
    }
    for child in children.iter_mut() {
        if let Some(found) = detach_subtree(child, target) {
            return Some(found);
        }
    }
    None
}

fn insert_child(tree: &mut Value, parent_id: &str, index: usize, node: Value) -> PactResult<()> {
    let parent = find_node_mut(tree, parent_id)
        .ok_or_else(|| PactError::corrupt(format!("insert: parent not found: {parent_id}")))?;
    let parent_obj = parent
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt(format!("insert: parent not an object: {parent_id}")))?;
    let children = parent_obj
        .entry("children".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            PactError::corrupt(format!("insert: parent children not an array: {parent_id}"))
        })?;
    let at = index.min(children.len());
    children.insert(at, node);
    Ok(())
}
