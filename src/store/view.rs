use super::model::{
    ComponentRecord, ContentRecordSummary, ContextTreeView, PactProjection, PactView,
};
use crate::components::Component;
use serde_json::Value;
use std::collections::BTreeMap;

impl PactView {
    pub fn from_projection(
        projection: PactProjection,
        branch_id: String,
        head_revision: u64,
    ) -> Self {
        Self::from_projection_with_lifecycle_seen(
            projection,
            branch_id,
            head_revision,
            BTreeMap::new(),
            0,
        )
    }

    pub fn from_projection_with_lifecycle_seen(
        projection: PactProjection,
        branch_id: String,
        head_revision: u64,
        mut lifecycle_seen: BTreeMap<String, u64>,
        birth_tick: u64,
    ) -> Self {
        let mut message_rows = projection
            .tree
            .index
            .by_type
            .get("message_turn")
            .into_iter()
            .flatten()
            .filter_map(|component_id| {
                projection
                    .component(component_id)
                    .map(|component| (pact_message_order_key(component), component.id.clone()))
            })
            .collect::<Vec<_>>();
        message_rows.sort_by(|left, right| left.0.cmp(&right.0));

        let content_refs = projection
            .blobs
            .iter()
            .map(|(id, blob)| {
                (
                    id.clone(),
                    ContentRecordSummary {
                        id: id.clone(),
                        kind: "text".to_string(),
                        hash: id.clone(),
                        bytes: blob.preview.len(),
                        chars: Some(blob.chars),
                        preview: blob.preview.clone(),
                    },
                )
            })
            .collect();

        for component_id in projection.tree.index.by_id.keys() {
            lifecycle_seen
                .entry(component_id.clone())
                .or_insert(birth_tick);
        }

        Self {
            tree: projection.tree,
            branch_id,
            head_revision,
            lifecycle_seen,
            message_order: message_rows.into_iter().map(|(_, id)| id).collect(),
            branch_refs: Vec::new(),
            content_refs,
        }
    }

    pub fn component(&self, component_id: &str) -> Option<&Component> {
        self.tree.component(component_id)
    }

    pub fn component_mut(&mut self, component_id: &str) -> Option<&mut Component> {
        self.tree.component_mut(component_id)
    }

    pub fn components_by_id(&self) -> BTreeMap<String, &Component> {
        self.tree
            .index
            .by_id
            .keys()
            .filter_map(|component_id| {
                self.component(component_id)
                    .map(|component| (component_id.clone(), component))
            })
            .collect()
    }

    pub fn component_values(&self) -> Vec<&Component> {
        self.tree
            .index
            .by_id
            .keys()
            .filter_map(|component_id| self.component(component_id))
            .collect()
    }

    pub fn component_ids(&self) -> impl Iterator<Item = &String> {
        self.tree.index.by_id.keys()
    }

    pub fn child_ids(&self, component_id: &str) -> &[String] {
        self.tree
            .index
            .child_ids
            .get(component_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn component_value_map(&self) -> BTreeMap<String, Value> {
        self.components_by_id()
            .into_iter()
            .map(|(id, component)| (id, component.project_component_value()))
            .collect()
    }

    pub fn component_count(&self) -> usize {
        self.tree.index.by_id.len()
    }

    pub(crate) fn component_record(&self, component_id: &str) -> Option<ComponentRecord> {
        self.tree.component_records().remove(component_id)
    }

    pub(crate) fn component_records(&self) -> BTreeMap<String, ComponentRecord> {
        self.tree.component_records()
    }
}

impl PactProjection {
    pub fn component(&self, component_id: &str) -> Option<&Component> {
        self.tree.component(component_id)
    }

    pub fn component_ids(&self) -> impl Iterator<Item = &String> {
        self.tree.index.by_id.keys()
    }

    #[allow(dead_code)]
    pub(crate) fn component_record(&self, component_id: &str) -> Option<ComponentRecord> {
        self.tree.component_records().remove(component_id)
    }

    #[allow(dead_code)]
    pub(crate) fn component_records(&self) -> BTreeMap<String, ComponentRecord> {
        self.tree.component_records()
    }
}

impl ContextTreeView {
    pub fn component(&self, component_id: &str) -> Option<&Component> {
        let path = self.index.by_id.get(component_id)?;
        component_at_path(&self.root, &path.path)
    }

    pub fn component_mut(&mut self, component_id: &str) -> Option<&mut Component> {
        let path = self.index.by_id.get(component_id)?.path.clone();
        component_at_path_mut(&mut self.root, &path)
    }

    pub fn component_ids(&self) -> impl Iterator<Item = &String> {
        self.index.by_id.keys()
    }

    pub(crate) fn component_records(&self) -> BTreeMap<String, ComponentRecord> {
        self.index
            .by_id
            .iter()
            .filter_map(|(component_id, path)| {
                self.component(component_id).map(|component| {
                    (
                        component_id.clone(),
                        ComponentRecord::from_component_with_parent(
                            component,
                            path.parent_id.clone(),
                        ),
                    )
                })
            })
            .collect()
    }
}

fn component_at_path<'a>(root: &'a Component, path: &[usize]) -> Option<&'a Component> {
    let mut current = root;
    for index in path {
        current = current.children.get(*index)?;
    }
    Some(current)
}

fn component_at_path_mut<'a>(root: &'a mut Component, path: &[usize]) -> Option<&'a mut Component> {
    let mut current = root;
    for index in path {
        current = current.children.get_mut(*index)?;
    }
    Some(current)
}

fn pact_message_order_key(component: &Component) -> (i32, i32, u64, u64, String) {
    let depth = component.pact_depth.unwrap_or(component.offset);
    let depth_rank = if depth < 0 {
        -1
    } else if depth == 0 {
        i32::MAX
    } else {
        i32::MAX - depth
    };
    (
        depth_rank,
        component.offset,
        component.created_at_ns,
        component.creation_index,
        component.id.clone(),
    )
}
