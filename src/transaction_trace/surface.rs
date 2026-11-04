use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct NodeLocation<'a> {
    pub node: &'a Value,

    pub parent_id: Option<String>,

    pub index: usize,

    pub depth: usize,
}

pub(crate) fn index_surface(tree: &Value) -> BTreeMap<String, NodeLocation<'_>> {
    let mut out = BTreeMap::new();
    index_node(tree, None, 0, 0, &mut out);
    out
}

fn index_node<'a>(
    node: &'a Value,
    parent_id: Option<String>,
    index: usize,
    depth: usize,
    out: &mut BTreeMap<String, NodeLocation<'a>>,
) {
    let Some(id) = component_id(node) else {
        return;
    };
    out.insert(
        id.clone(),
        NodeLocation {
            node,
            parent_id,
            index,
            depth,
        },
    );
    for (child_index, child) in children(node).iter().enumerate() {
        index_node(child, Some(id.clone()), child_index, depth + 1, out);
    }
}

pub(crate) fn component_id(node: &Value) -> Option<String> {
    node.get(super::trace_op::ID_KEY)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(crate) fn children(node: &Value) -> &[Value] {
    node.get(super::trace_op::CHILDREN_KEY)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub(crate) fn node_without_children(node: &Value) -> Value {
    let mut clone = node.clone();
    if let Some(object) = clone.as_object_mut()
        && object.contains_key(super::trace_op::CHILDREN_KEY)
    {
        object.insert(
            super::trace_op::CHILDREN_KEY.to_string(),
            Value::Array(Vec::new()),
        );
    }
    clone
}
