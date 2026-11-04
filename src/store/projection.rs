use super::model::{
    BlobRecord, ComponentPath, ComponentViewIndex, ContextTreeView, PactProjection, PactStoreStats,
    ToolPairRecord,
};
use crate::components::{Component, PactContentRef};
use crate::error::{PactError, PactResult};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn build_projection(
    tree: &Value,
    known_first_seen: &BTreeMap<String, u64>,
    blob_inline_limit: usize,
) -> PactResult<PactProjection> {
    let root = Component::from_component_value(tree)
        .map_err(|err| PactError::invalid(format!("invalid PACT component tree: {err}")))?;
    build_projection_from_owned_component(root, known_first_seen.clone(), blob_inline_limit)
}

pub fn build_projection_from_component(
    root: &Component,
    known_first_seen: &BTreeMap<String, u64>,
    blob_inline_limit: usize,
) -> PactResult<PactProjection> {
    build_projection_from_owned_component(root.clone(), known_first_seen.clone(), blob_inline_limit)
}

pub(crate) fn build_projection_from_owned_component(
    mut root: Component,
    known_first_seen: BTreeMap<String, u64>,
    blob_inline_limit: usize,
) -> PactResult<PactProjection> {
    #[cfg(test)]
    crate::tests::instrumentation::PROJECTION_BUILD_COUNT.with(|count| count.set(count.get() + 1));

    let root_id = root.id.clone();
    let mut builder = ProjectionBuilder {
        first_seen: known_first_seen,
        tool_pairs: BTreeMap::new(),
        blobs: BTreeMap::new(),
        blob_ids_by_content: BTreeMap::new(),
        stats: PactStoreStats {
            node_count: 0,
            edge_count: 0,
            blob_count: 0,
            content_chars: 0,
        },
        blob_inline_limit,
    };
    builder.externalize_large_content(&mut root);
    let mut index = ComponentViewIndex::default();
    builder.collect(&root, None, Vec::new(), &mut index);
    builder.stats.blob_count = builder.blobs.len();
    Ok(PactProjection {
        tree: ContextTreeView {
            root_id,
            stats: builder.stats,
            root,
            index,
            first_seen: builder.first_seen,
            tool_pairs: builder.tool_pairs,
        },
        blobs: builder.blobs,
    })
}

struct ProjectionBuilder {
    first_seen: BTreeMap<String, u64>,
    tool_pairs: BTreeMap<String, ToolPairRecord>,
    blobs: BTreeMap<String, BlobRecord>,
    blob_ids_by_content: BTreeMap<String, String>,
    stats: PactStoreStats,
    blob_inline_limit: usize,
}

impl ProjectionBuilder {
    fn externalize_large_content(&mut self, component: &mut Component) {
        if let Some(text) = component.content().map(str::to_string) {
            let chars = text.chars().count();
            if chars > self.blob_inline_limit {
                let blob_id = if let Some(existing_id) = self.blob_ids_by_content.get(&text) {
                    existing_id.clone()
                } else {
                    let blob_id = format!("blob_{:08x}", self.blobs.len());
                    self.blobs.insert(
                        blob_id.clone(),
                        BlobRecord {
                            id: blob_id.clone(),
                            chars,
                            preview: text.chars().take(self.blob_inline_limit).collect(),
                        },
                    );
                    self.blob_ids_by_content
                        .insert(text.clone(), blob_id.clone());
                    blob_id
                };
                component.externalize_content(PactContentRef {
                    id: blob_id,
                    field: Some("$.content".to_string()),
                    chars: Some(chars as u64),
                    preview: Some(text.chars().take(self.blob_inline_limit).collect()),
                    source: None,
                });
            }
        }
        for child in &mut component.children {
            self.externalize_large_content(child);
        }
    }

    fn collect(
        &mut self,
        component: &Component,
        parent_id: Option<String>,
        path: Vec<usize>,
        index: &mut ComponentViewIndex,
    ) {
        self.first_seen.entry(component.id.clone()).or_insert(0);
        self.stats.node_count += 1;
        self.stats.edge_count += component.children.len();
        if let Some(text) = component.content() {
            self.stats.content_chars += text.chars().count();
        }
        if let Some(tool_call_id) = component.tool_call_id() {
            let pair = self
                .tool_pairs
                .entry(tool_call_id.to_string())
                .or_insert_with(|| ToolPairRecord {
                    tool_call_id: tool_call_id.to_string(),
                    call_ids: Vec::new(),
                    result_ids: Vec::new(),
                });
            let lowered = component.component_type().to_ascii_lowercase();
            if lowered.contains("result") {
                pair.result_ids.push(component.id.clone());
            } else if lowered.contains("call") {
                pair.call_ids.push(component.id.clone());
            }
        }

        index.by_id.insert(
            component.id.clone(),
            ComponentPath {
                component_id: component.id.clone(),
                parent_id,
                path: path.clone(),
            },
        );
        index
            .by_type
            .entry(component.component_type().to_string())
            .or_default()
            .push(component.id.clone());
        for key in component.aux.keys() {
            index
                .by_aux_key
                .entry(key.clone())
                .or_default()
                .push(component.id.clone());
        }
        index.child_ids.insert(
            component.id.clone(),
            component
                .children
                .iter()
                .map(|child| child.id.clone())
                .collect(),
        );
        for (child_index, child) in component.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(child_index);
            self.collect(child, Some(component.id.clone()), child_path, index);
        }
    }
}

pub(super) fn content_ref_id_from_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.get("id").and_then(Value::as_str).map(str::to_string))
        .or_else(|| {
            value
                .get("content_ref")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("content_ref")
                .and_then(|ref_value| ref_value.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}
