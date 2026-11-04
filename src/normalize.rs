use crate::components::Component;
use crate::error::{PactError, PactResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

pub const NORMALIZED_SCHEMA: &str = "pact.normalized.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NormalizedNode {
    pub id: String,
    #[serde(rename = "componentType")]
    pub node_type: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub offset: i32,
    pub depth: i32,
    pub priority: i32,
    pub transaction_index: u64,
    pub created_at_ns: u64,
    pub creation_index: u64,
    pub child_count: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NormalizedNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactStats {
    #[serde(rename = "components")]
    pub nodes: usize,
    pub leaves: usize,
    pub max_depth: i32,
    pub content_bytes: usize,
}

pub fn normalize_tree(root: Component) -> NormalizedNode {
    normalize_at_depth(root, 0)
}

pub fn normalize_pact_json(pact_json: &str, blob_inline_limit: usize) -> PactResult<String> {
    let value: Value = serde_json::from_str(pact_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))?;
    reject_removed_vocabulary(&value)?;
    let mut fallback_id = 0usize;
    let mut normalizer = PactNormalizer::new(blob_inline_limit, pact_json.len());
    let root_id = normalizer.normalize_node(&value, None, &mut fallback_id);
    serde_json::to_string(&normalizer.finish(root_id))
        .map_err(|err| PactError::codec(format!("failed to encode normalized PACT: {err}")))
}

pub fn inspect(root: &NormalizedNode) -> PactStats {
    let mut stats = PactStats {
        nodes: 0,
        leaves: 0,
        max_depth: root.depth,
        content_bytes: 0,
    };
    inspect_into(root, &mut stats);
    stats
}

fn reject_removed_vocabulary(value: &Value) -> PactResult<()> {
    fn walk(value: &Value, path: &str) -> PactResult<()> {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if key == "cycle" {
                        return Err(PactError::invalid(format!(
                            "removed PACT field `{path}.cycle`; use transaction_index"
                        )));
                    }
                    if key == "snapshot" || key == "snapshots" || key.ends_with("_snapshot_id") {
                        return Err(PactError::invalid(format!(
                            "removed PACT snapshot field `{path}.{key}`; use transaction traces and refs"
                        )));
                    }
                    let next_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(child, &next_path)?;
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}[{index}]"))?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    walk(value, "$")
}

fn normalize_at_depth(mut node: Component, depth: i32) -> NormalizedNode {
    node.children.sort_by(|left, right| {
        (
            left.offset,
            left.priority,
            left.created_at_ns,
            left.creation_index,
            left.id.as_str(),
        )
            .cmp(&(
                right.offset,
                right.priority,
                right.created_at_ns,
                right.creation_index,
                right.id.as_str(),
            ))
    });

    let content = node.content().map(str::to_string).unwrap_or_default();
    let component_type = node.component_type().to_string();

    let attrs = node.xml_attrs().cloned().unwrap_or_default();
    let children: Vec<NormalizedNode> = node
        .children
        .into_iter()
        .map(|child| normalize_at_depth(child, depth + 1))
        .collect();

    NormalizedNode {
        id: node.id,
        node_type: component_type,
        kind: node.subtype,
        role: node.role,
        offset: node.offset,
        depth,
        priority: node.priority,
        transaction_index: node.transaction_index,
        created_at_ns: node.created_at_ns,
        creation_index: node.creation_index,
        child_count: children.len(),
        attrs,
        content,
        children,
    }
}

struct PactNormalizer {
    nodes: Map<String, Value>,
    blobs: Map<String, Value>,
    blob_ids: HashMap<String, String>,
    blob_inline_limit: usize,
    node_count: usize,
    edge_count: usize,
    content_chars: usize,
    original_bytes: usize,
}

impl PactNormalizer {
    fn new(blob_inline_limit: usize, original_bytes: usize) -> Self {
        Self {
            nodes: Map::new(),
            blobs: Map::new(),
            blob_ids: HashMap::new(),
            blob_inline_limit,
            node_count: 0,
            edge_count: 0,
            content_chars: 0,
            original_bytes,
        }
    }

    fn normalize_node(
        &mut self,
        value: &Value,
        parent_id: Option<&str>,
        fallback_id: &mut usize,
    ) -> String {
        let object = match value.as_object() {
            Some(object) => object,
            None => {
                let id = format!("literal_{:08x}", *fallback_id);
                *fallback_id += 1;
                let mut node = Map::new();
                node.insert("id".to_string(), Value::String(id.clone()));
                node.insert(
                    "componentType".to_string(),
                    Value::String("literal".to_string()),
                );
                node.insert("type".to_string(), Value::String("literal".to_string()));
                node.insert(
                    "parent_id".to_string(),
                    parent_id
                        .map(|value| Value::String(value.to_string()))
                        .unwrap_or(Value::Null),
                );
                node.insert("child_ids".to_string(), Value::Array(vec![]));
                node.insert("value".to_string(), value.clone());
                self.nodes.insert(id.clone(), Value::Object(node));
                self.node_count += 1;
                return id;
            }
        };

        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let generated = format!("node_{:08x}", *fallback_id);
                *fallback_id += 1;
                generated
            });

        let children = object
            .get("children")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut child_ids = Vec::with_capacity(children.len());
        for child in children {
            let child_id = self.normalize_node(&child, Some(&id), fallback_id);
            child_ids.push(Value::String(child_id));
            self.edge_count += 1;
        }

        let mut attrs = Map::new();
        for (key, attr_value) in object {
            if matches!(key.as_str(), "children" | "content" | "org") {
                continue;
            }
            if let Some(scalar) = scalar_attr_value(attr_value) {
                attrs.insert(key.clone(), scalar);
            }
        }

        let mut node = Map::new();
        node.insert("id".to_string(), Value::String(id.clone()));
        node.insert(
            "componentType".to_string(),
            Value::String(
                object
                    .get("componentType")
                    .and_then(Value::as_str)
                    .or_else(|| object.get("type").and_then(Value::as_str))
                    .unwrap_or("node")
                    .to_string(),
            ),
        );
        if let Some(kind) = object.get("type").and_then(Value::as_str) {
            node.insert("type".to_string(), Value::String(kind.to_string()));
        }
        node.insert(
            "parent_id".to_string(),
            parent_id
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
        );
        node.insert("child_ids".to_string(), Value::Array(child_ids));
        node.insert("attrs".to_string(), Value::Object(attrs));

        for text_key in ["content", "org"] {
            if let Some(text) = object.get(text_key).and_then(Value::as_str) {
                self.add_text_field(&mut node, text_key, text);
            }
        }

        self.nodes.insert(id.clone(), Value::Object(node));
        self.node_count += 1;
        id
    }

    fn add_text_field(&mut self, node: &mut Map<String, Value>, key: &str, text: &str) {
        let chars = text.chars().count();
        self.content_chars += chars;
        node.insert(format!("{key}_chars"), Value::Number(chars.into()));
        if chars <= self.blob_inline_limit {
            node.insert(key.to_string(), Value::String(text.to_string()));
        } else {
            let blob_id = self.intern_blob(text);
            node.insert(format!("{key}_ref"), Value::String(blob_id));
            node.insert(
                format!("{key}_preview"),
                Value::String(preview_text(text, self.blob_inline_limit.min(512))),
            );
        }
    }

    fn intern_blob(&mut self, text: &str) -> String {
        if let Some(existing) = self.blob_ids.get(text) {
            return existing.clone();
        }
        let id = format!("blob_{:08x}", self.blob_ids.len());
        self.blob_ids.insert(text.to_string(), id.clone());

        let mut blob = Map::new();
        blob.insert("id".to_string(), Value::String(id.clone()));
        blob.insert(
            "chars".to_string(),
            Value::Number(text.chars().count().into()),
        );
        blob.insert(
            "preview".to_string(),
            Value::String(preview_text(text, self.blob_inline_limit.min(512))),
        );
        self.blobs.insert(id.clone(), Value::Object(blob));
        id
    }

    fn finish(self, root_id: String) -> Value {
        let normalized_bytes = serde_json::to_vec(&Value::Object({
            let mut root = Map::new();
            root.insert("components".to_string(), Value::Object(self.nodes.clone()));
            root.insert("blobs".to_string(), Value::Object(self.blobs.clone()));
            root
        }))
        .map(|bytes| bytes.len())
        .unwrap_or(0);

        let mut stats = Map::new();
        stats.insert(
            "component_count".to_string(),
            Value::Number(self.node_count.into()),
        );
        stats.insert(
            "edge_count".to_string(),
            Value::Number(self.edge_count.into()),
        );
        stats.insert(
            "blob_count".to_string(),
            Value::Number(self.blobs.len().into()),
        );
        stats.insert(
            "content_chars".to_string(),
            Value::Number(self.content_chars.into()),
        );
        stats.insert(
            "original_bytes".to_string(),
            Value::Number(self.original_bytes.into()),
        );
        stats.insert(
            "normalized_bytes".to_string(),
            Value::Number(normalized_bytes.into()),
        );

        let mut out = Map::new();
        out.insert(
            "schema".to_string(),
            Value::String(NORMALIZED_SCHEMA.to_string()),
        );
        out.insert("root_id".to_string(), Value::String(root_id));
        out.insert("components".to_string(), Value::Object(self.nodes));
        out.insert("blobs".to_string(), Value::Object(self.blobs));
        out.insert("stats".to_string(), Value::Object(stats));
        Value::Object(out)
    }
}

fn scalar_attr_value(value: &Value) -> Option<Value> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Some(value.clone()),
        _ => None,
    }
}

fn preview_text(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    value.chars().take(limit).collect()
}

fn inspect_into(node: &NormalizedNode, stats: &mut PactStats) {
    stats.nodes += 1;
    stats.max_depth = stats.max_depth.max(node.depth);
    stats.content_bytes += node.content.len();
    if node.children.is_empty() {
        stats.leaves += 1;
    }
    for child in &node.children {
        inspect_into(child, stats);
    }
}

#[cfg(test)]
mod tests;
