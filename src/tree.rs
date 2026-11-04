use crate::components::{Component, LifecycleState};
use crate::error::{PactError, PactResult};
use crate::store::ComponentPath;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentTree {
    root: Component,
    index: ComponentTreeIndex,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ComponentTreeIndex {
    by_id: BTreeMap<String, ComponentPath>,
    parent_by_id: BTreeMap<String, String>,
    children_by_id: BTreeMap<String, Vec<String>>,
    by_type: BTreeMap<String, Vec<String>>,
    by_role: BTreeMap<String, Vec<String>>,
    by_key: BTreeMap<String, Vec<String>>,
    by_tag: BTreeMap<String, Vec<String>>,
    by_lifecycle_state: BTreeMap<LifecycleState, Vec<String>>,
    ttl_candidates: Vec<String>,
    cad_candidates: Vec<String>,
    tool_calls_by_call_id: BTreeMap<String, Vec<String>>,
    tool_results_by_call_id: BTreeMap<String, Vec<String>>,
}

impl ComponentTreeIndex {
    fn try_derive(root: &Component) -> PactResult<Self> {
        let mut index = ComponentTreeIndex::default();
        index_component(root, None, Vec::new(), &mut index)?;
        Ok(index)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToolPairIds {
    pub(crate) call_ids: Vec<String>,
    pub(crate) result_ids: Vec<String>,
}

impl ComponentTree {
    pub fn from_root(root: Component) -> PactResult<Self> {
        let mut tree = Self {
            root,
            index: ComponentTreeIndex::default(),
        };
        tree.rebuild_index()?;
        Ok(tree)
    }

    pub fn from_value(value: &Value) -> PactResult<Self> {
        let root = Component::from_component_value(value)
            .map_err(|err| PactError::invalid(format!("invalid component tree: {err}")))?;
        Self::from_root(root)
    }

    pub fn project_value(&self) -> Value {
        self.root.project_component_value()
    }

    pub fn root(&self) -> &Component {
        &self.root
    }

    pub fn root_id(&self) -> &str {
        &self.root.id
    }

    pub(crate) fn edit<T>(
        &mut self,
        f: impl FnOnce(&mut Component) -> PactResult<T>,
    ) -> PactResult<T> {
        #[cfg(test)]
        crate::tests::instrumentation::TREE_EDIT_CALLS.with(|calls| calls.set(calls.get() + 1));

        let mut candidate_root = self.root.clone();
        let out = f(&mut candidate_root)?;
        let candidate_index = ComponentTreeIndex::try_derive(&candidate_root)?;
        self.root = candidate_root;
        self.index = candidate_index;
        #[cfg(debug_assertions)]
        self.debug_assert_index_coherent();
        Ok(out)
    }

    #[cfg(debug_assertions)]
    fn debug_assert_index_coherent(&self) {
        let derived = ComponentTreeIndex::try_derive(&self.root)
            .expect("live ComponentTree root must derive a valid index");
        debug_assert!(
            self.index == derived,
            "ComponentTree index is incoherent with its tree"
        );
    }

    pub fn contains(&self, component_id: &str) -> bool {
        self.index.by_id.contains_key(component_id)
    }

    pub fn component(&self, component_id: &str) -> Option<&Component> {
        let path = self.index.by_id.get(component_id)?;
        component_at_path(&self.root, &path.path)
    }

    pub(crate) fn component_path(&self, component_id: &str) -> Option<&ComponentPath> {
        self.index.by_id.get(component_id)
    }

    pub(crate) fn component_ids(&self) -> impl Iterator<Item = &str> {
        self.index.by_id.keys().map(String::as_str)
    }

    pub fn parent_id(&self, component_id: &str) -> Option<&str> {
        self.index
            .parent_by_id
            .get(component_id)
            .map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn child_ids(&self, component_id: &str) -> &[String] {
        self.index
            .children_by_id
            .get(component_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn ids_by_type(&self, component_type: &str) -> &[String] {
        self.index
            .by_type
            .get(component_type)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn ids_by_role(&self, role: &str) -> &[String] {
        self.index
            .by_role
            .get(role)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[allow(dead_code)]
    pub(crate) fn id_by_key(&self, key: &str) -> Option<&str> {
        self.index
            .by_key
            .get(key)
            .and_then(|ids| ids.first())
            .map(String::as_str)
    }

    pub(crate) fn ids_by_key(&self, key: &str) -> &[String] {
        self.index.by_key.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    #[allow(dead_code)]
    pub(crate) fn ids_by_tag(&self, tag: &str) -> &[String] {
        self.index.by_tag.get(tag).map(Vec::as_slice).unwrap_or(&[])
    }

    #[allow(dead_code)]
    pub(crate) fn ids_by_lifecycle_state(&self, state: LifecycleState) -> &[String] {
        self.index
            .by_lifecycle_state
            .get(&state)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn ttl_candidate_ids(&self) -> &[String] {
        &self.index.ttl_candidates
    }

    #[allow(dead_code)]
    pub(crate) fn cad_candidate_ids(&self) -> &[String] {
        &self.index.cad_candidates
    }

    pub(crate) fn tool_pair_ids(&self, tool_call_id: &str) -> ToolPairIds {
        ToolPairIds {
            call_ids: self
                .index
                .tool_calls_by_call_id
                .get(tool_call_id)
                .cloned()
                .unwrap_or_default(),
            result_ids: self
                .index
                .tool_results_by_call_id
                .get(tool_call_id)
                .cloned()
                .unwrap_or_default(),
        }
    }

    pub(crate) fn tool_call_ids(&self) -> impl Iterator<Item = &str> {
        self.index.tool_calls_by_call_id.keys().map(String::as_str)
    }

    pub(crate) fn tool_result_call_ids(&self) -> impl Iterator<Item = &str> {
        self.index
            .tool_results_by_call_id
            .keys()
            .map(String::as_str)
    }

    pub(crate) fn indexed_unpaired_tool_call_ids(&self) -> Vec<String> {
        self.tool_call_ids()
            .filter(|tool_call_id| {
                !self
                    .index
                    .tool_results_by_call_id
                    .contains_key(*tool_call_id)
            })
            .map(str::to_string)
            .collect()
    }

    fn rebuild_index(&mut self) -> PactResult<()> {
        self.index = ComponentTreeIndex::try_derive(&self.root)?;
        Ok(())
    }

    #[allow(dead_code)]
    fn index_subtree_at_path(
        &mut self,
        parent_id: Option<String>,
        path: Vec<usize>,
    ) -> PactResult<()> {
        let component = component_at_path(&self.root, &path)
            .ok_or_else(|| PactError::not_found("component path not found for index update"))?;
        let mut subtree_index = ComponentTreeIndex::default();
        index_component(component, parent_id.as_deref(), path, &mut subtree_index)?;
        self.index.by_id.extend(subtree_index.by_id);
        self.index.parent_by_id.extend(subtree_index.parent_by_id);
        self.index
            .children_by_id
            .extend(subtree_index.children_by_id);
        extend_index_map(&mut self.index.by_type, subtree_index.by_type);
        extend_index_map(&mut self.index.by_role, subtree_index.by_role);
        extend_index_map(&mut self.index.by_key, subtree_index.by_key);
        extend_index_map(&mut self.index.by_tag, subtree_index.by_tag);
        extend_index_map(
            &mut self.index.by_lifecycle_state,
            subtree_index.by_lifecycle_state,
        );
        self.index
            .ttl_candidates
            .extend(subtree_index.ttl_candidates);
        self.index
            .cad_candidates
            .extend(subtree_index.cad_candidates);
        extend_index_map(
            &mut self.index.tool_calls_by_call_id,
            subtree_index.tool_calls_by_call_id,
        );
        extend_index_map(
            &mut self.index.tool_results_by_call_id,
            subtree_index.tool_results_by_call_id,
        );
        Ok(())
    }

    #[allow(dead_code)]
    fn sort_children_recursive(&mut self) {
        self.root.sort_children_recursive();
    }
}

fn extend_index_map<K: Ord>(
    target: &mut BTreeMap<K, Vec<String>>,
    source: BTreeMap<K, Vec<String>>,
) {
    for (key, mut ids) in source {
        target.entry(key).or_default().append(&mut ids);
    }
}

fn index_component(
    component: &Component,
    parent_id: Option<&str>,
    path: Vec<usize>,
    index: &mut ComponentTreeIndex,
) -> PactResult<()> {
    if index.by_id.contains_key(&component.id) {
        return Err(PactError::invalid(format!(
            "duplicate component id `{}` in component tree",
            component.id
        )));
    }
    index.by_id.insert(
        component.id.clone(),
        ComponentPath {
            component_id: component.id.clone(),
            parent_id: parent_id.map(str::to_string),
            path: path.clone(),
        },
    );
    if let Some(parent_id) = parent_id {
        index
            .parent_by_id
            .insert(component.id.clone(), parent_id.to_string());
    }
    index.children_by_id.insert(
        component.id.clone(),
        component
            .children
            .iter()
            .map(|child| child.id.clone())
            .collect(),
    );
    index
        .by_type
        .entry(component.component_type().to_string())
        .or_default()
        .push(component.id.clone());
    if let Some(role) = component.role.as_ref().filter(|role| !role.is_empty()) {
        index
            .by_role
            .entry(role.clone())
            .or_default()
            .push(component.id.clone());
    }
    if let Some(key) = component.key.as_ref().filter(|key| !key.is_empty()) {
        index
            .by_key
            .entry(key.clone())
            .or_default()
            .push(component.id.clone());
    }
    for tag in &component.tags {
        if !tag.is_empty() {
            index
                .by_tag
                .entry(tag.clone())
                .or_default()
                .push(component.id.clone());
        }
    }
    index
        .by_lifecycle_state
        .entry(component.lifecycle.state)
        .or_default()
        .push(component.id.clone());
    if component.lifecycle.policy.ttl.is_some() {
        index.ttl_candidates.push(component.id.clone());
    }
    if component.lifecycle.policy.cad.is_some() {
        index.cad_candidates.push(component.id.clone());
    }
    if let Some(tool_call_id) = component.tool_call_id() {
        let lowered = component.component_type().to_ascii_lowercase();
        if lowered.contains("result") {
            index
                .tool_results_by_call_id
                .entry(tool_call_id.to_string())
                .or_default()
                .push(component.id.clone());
        } else if lowered.contains("call") {
            index
                .tool_calls_by_call_id
                .entry(tool_call_id.to_string())
                .or_default()
                .push(component.id.clone());
        }
    }
    for (child_index, child) in component.children.iter().enumerate() {
        let mut child_path = path.clone();
        child_path.push(child_index);
        index_component(child, Some(&component.id), child_path, index)?;
    }
    Ok(())
}

fn component_at_path<'a>(component: &'a Component, path: &[usize]) -> Option<&'a Component> {
    let mut current = component;
    for index in path {
        current = current.children.get(*index)?;
    }
    Some(current)
}

pub(crate) fn component_by_id_mut<'a>(
    root: &'a mut Component,
    component_id: &str,
) -> Option<&'a mut Component> {
    if root.id == component_id {
        return Some(root);
    }
    for child in &mut root.children {
        if let Some(found) = component_by_id_mut(child, component_id) {
            return Some(found);
        }
    }
    None
}

pub(crate) fn children_by_id_mut<'a>(
    root: &'a mut Component,
    component_id: &str,
) -> Option<&'a mut Vec<Component>> {
    component_by_id_mut(root, component_id).map(|component| &mut component.children)
}

#[cfg(test)]
mod tests;
