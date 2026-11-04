use super::branch_projection::{
    ProjectionTreeScope, branch_inspection_value, decode_context_tree_ledger, decode_store,
    encode_root_store, encode_store, hydrate_content_refs_in_value, next_branch_id, parse_aux,
    project_branch_archive_component_tree, project_branch_archive_tree_value,
    project_branch_component_tree, project_head_snapshot,
};
use super::model::{
    BranchEventIndex, CHECKPOINT_SCHEMA, ComponentViewIndex, ContentStore, ContextTreeLedger,
    PactBranch, PactRef, PactStore, PactStoreStats, PactTransactionTrace,
    PactTransactionTraceBuildRequest, PactView, ROOT_STORE_SCHEMA, STORE_SCHEMA,
};
use super::projection::{build_projection, build_projection_from_component};
use crate::error::{PactError, PactResult};
use crate::file_format::base64_decode;
use crate::resolver::PactResolver;
use crate::trace::BoundaryKind;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

impl ContextTreeLedger {
    pub fn project_branch_with_scope(
        &self,
        branch_id: &str,
        revision: Option<u64>,
        blob_inline_limit: usize,
        scope: ProjectionTreeScope,
    ) -> PactResult<PactView> {
        let branch = self
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        let target_revision = revision.unwrap_or(branch.head_revision);
        let projection = match scope {
            ProjectionTreeScope::Head => {
                let first_seen = branch
                    .current
                    .tree
                    .first_seen
                    .iter()
                    .filter(|(_, seen_revision)| **seen_revision <= target_revision)
                    .map(|(component_id, seen_revision)| (component_id.clone(), *seen_revision))
                    .collect::<BTreeMap<_, _>>();
                let tree = project_head_snapshot(self, branch, Some(target_revision))?;
                build_projection_from_component(tree.root(), &first_seen, blob_inline_limit)?
            }
            ProjectionTreeScope::Archive => {
                let (tree, first_seen) =
                    project_branch_archive_component_tree(self, branch, Some(target_revision))?;
                build_projection_from_component(tree.root(), &first_seen, blob_inline_limit)?
            }
        };
        Ok(PactView::from_projection(
            projection,
            branch_id.to_string(),
            target_revision,
        ))
    }

    pub fn project_branch(
        &self,
        branch_id: &str,
        revision: Option<u64>,
        blob_inline_limit: usize,
    ) -> PactResult<PactView> {
        self.project_branch_with_scope(
            branch_id,
            revision,
            blob_inline_limit,
            ProjectionTreeScope::Head,
        )
    }

    pub fn inspect(&self) -> Value {
        let mut branches = Map::new();
        for (branch_id, branch) in &self.branches {
            branches.insert(branch_id.clone(), branch_inspection_value(branch));
        }
        serde_json::json!({
            "schema": self.schema,
            "root_id": self.root_id,
            "current_branch_id": self.current_branch_id,
            "branch_count": self.branches.len(),
            "content_record_count": self.content.records.len(),
            "transaction_trace_count": self.trace_count(),
            "branches": branches,
        })
    }

    pub fn inspect_branch(&self, branch_id: &str) -> PactResult<Value> {
        let branch = self
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        Ok(branch_inspection_value(branch))
    }

    pub fn fork_at_revision(
        mut self,
        from_branch_id: &str,
        revision: Option<u64>,
        new_branch_id: Option<String>,
        aux: BTreeMap<String, Value>,
        blob_inline_limit: usize,
    ) -> PactResult<(Self, String)> {
        let source_branch = self
            .branches
            .get(from_branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {from_branch_id}")))?
            .clone();
        let revision = revision.unwrap_or(source_branch.head_revision);
        let mut base =
            project_head_snapshot(&self, &source_branch, Some(revision))?.project_value();
        externalize_large_content_in_value(&mut base, &mut self.content, blob_inline_limit);
        let projection = build_projection(&base, &BTreeMap::new(), blob_inline_limit)?;
        let branch_id =
            new_branch_id.unwrap_or_else(|| next_branch_id(&self, from_branch_id, revision));
        if self.branches.contains_key(&branch_id) {
            return Err(PactError::invalid(format!(
                "branch already exists: {branch_id}"
            )));
        }

        let fork_ref = PactRef {
            root_id: self.root_id.clone(),
            branch_id: from_branch_id.to_string(),
            revision: Some(revision),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };
        let base_ref = PactRef {
            root_id: self.root_id.clone(),
            branch_id: branch_id.clone(),
            revision: Some(0),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };
        let mut view = PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_id.clone(),
            0,
            source_branch.current.lifecycle_seen.clone(),
            source_branch.lifecycle_tick,
        );
        sync_view_content_refs_from_store(&mut view, &self.content);
        let branch = PactBranch {
            branch_id: branch_id.clone(),
            root_id: self.root_id.clone(),
            parent_branch_id: Some(from_branch_id.to_string()),
            fork_ref: Some(fork_ref),
            base_ref,
            head_revision: 0,
            lifecycle_tick: source_branch.lifecycle_tick,
            transaction_traces: Vec::new(),
            current: view,
            aux,
        };
        self.branches.insert(branch_id.clone(), branch);
        self.current_branch_id = branch_id.clone();
        Ok((self, branch_id))
    }

    pub fn fork_reference_at(
        mut self,
        from_branch_id: &str,
        revision: Option<u64>,
        new_branch_id: Option<String>,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<(Self, String)> {
        let source_branch = self
            .branches
            .get(from_branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {from_branch_id}")))?
            .clone();
        let revision = revision.unwrap_or(source_branch.head_revision);
        let branch_id =
            new_branch_id.unwrap_or_else(|| next_branch_id(&self, from_branch_id, revision));
        if self.branches.contains_key(&branch_id) {
            return Err(PactError::invalid(format!(
                "branch already exists: {branch_id}"
            )));
        }

        let fork_ref = PactRef {
            root_id: self.root_id.clone(),
            branch_id: from_branch_id.to_string(),
            revision: Some(revision),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };

        let base_ref = PactRef {
            root_id: self.root_id.clone(),
            branch_id: branch_id.clone(),
            revision: Some(0),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };

        let mut empty_tree = source_branch.current.tree.clone();
        empty_tree.root.children.clear();
        empty_tree.index = ComponentViewIndex::default();
        empty_tree.first_seen = BTreeMap::new();
        empty_tree.tool_pairs = BTreeMap::new();
        empty_tree.stats = PactStoreStats {
            node_count: 0,
            edge_count: 0,
            blob_count: 0,
            content_chars: 0,
        };
        let view = PactView {
            tree: empty_tree,
            branch_id: branch_id.clone(),
            head_revision: 0,
            lifecycle_seen: BTreeMap::new(),
            message_order: Vec::new(),
            branch_refs: Vec::new(),
            content_refs: BTreeMap::new(),
        };

        let branch = PactBranch {
            branch_id: branch_id.clone(),
            root_id: self.root_id.clone(),
            parent_branch_id: Some(from_branch_id.to_string()),
            fork_ref: Some(fork_ref),
            base_ref,
            head_revision: 0,
            lifecycle_tick: source_branch.lifecycle_tick,
            transaction_traces: Vec::new(),
            current: view,
            aux,
        };
        self.branches.insert(branch_id.clone(), branch);
        self.current_branch_id = branch_id.clone();
        Ok((self, branch_id))
    }

    pub fn fork_at_component(
        self,
        from_branch_id: &str,
        component_id: &str,
        new_branch_id: Option<String>,
        aux: BTreeMap<String, Value>,
        blob_inline_limit: usize,
    ) -> PactResult<(Self, String)> {
        let branch = self
            .branches
            .get(from_branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {from_branch_id}")))?;
        let revision = branch
            .current
            .tree
            .first_seen
            .get(component_id)
            .copied()
            .ok_or_else(|| {
                PactError::not_found(format!(
                    "node first-seen revision not found: {component_id}"
                ))
            })?;
        self.fork_at_revision(
            from_branch_id,
            Some(revision),
            new_branch_id,
            aux,
            blob_inline_limit,
        )
    }

    pub fn record_transaction_trace(mut self, trace: PactTransactionTrace) -> PactResult<Self> {
        trace.validate()?;

        if trace.root_id != self.root_id {
            return Err(PactError::invalid(
                "transaction trace root_id does not match root store root_id",
            ));
        }
        if !self.branches.contains_key(&trace.branch_id) {
            return Err(PactError::not_found(format!(
                "transaction trace branch not found: {}",
                trace.branch_id
            )));
        }

        if let Some(replay_trace) = trace.replay_trace.clone() {
            replay_trace.validate()?;
            let branch = self.branches.get_mut(&trace.branch_id).ok_or_else(|| {
                PactError::not_found(format!(
                    "transaction trace branch not found: {}",
                    trace.branch_id
                ))
            })?;
            if !branch.transaction_traces.contains(&replay_trace) {
                branch.transaction_traces.push(replay_trace);
            }
        }

        let transaction_id = trace.transaction_id.clone();
        let branch_id = trace.branch_id.clone();
        self.event_index
            .transaction_traces
            .insert(transaction_id.clone(), trace);
        let branch_ids = self
            .event_index
            .by_transaction
            .entry(transaction_id)
            .or_default();
        if !branch_ids.contains(&branch_id) {
            branch_ids.push(branch_id);
        }

        Ok(self)
    }

    pub fn fork_at_transaction(
        self,
        from_branch_id: &str,
        transaction_id: &str,
        new_branch_id: Option<String>,
        aux: BTreeMap<String, Value>,
        blob_inline_limit: usize,
    ) -> PactResult<(Self, String)> {
        let trace = self
            .event_index
            .transaction_traces
            .get(transaction_id)
            .ok_or_else(|| {
                PactError::not_found(format!("transaction trace not found: {transaction_id}"))
            })?;
        if trace.branch_id != from_branch_id {
            return Err(PactError::invalid(format!(
                "transaction trace belongs to branch {}, not {from_branch_id}",
                trace.branch_id
            )));
        }
        let revision = trace.fork_revision().ok_or_else(|| {
            PactError::invalid(format!(
                "transaction trace has no revision boundary: {transaction_id}"
            ))
        })?;

        let (mut forked, branch_id) = self.fork_at_revision(
            from_branch_id,
            Some(revision),
            new_branch_id,
            aux,
            blob_inline_limit,
        )?;
        let branch = forked.branches.get_mut(&branch_id).ok_or_else(|| {
            PactError::not_found(format!(
                "fork branch not found after transaction fork: {branch_id}"
            ))
        })?;
        if let Some(fork_ref) = branch.fork_ref.as_mut() {
            fork_ref.transaction_id = Some(transaction_id.to_string());
        }
        Ok((forked, branch_id))
    }

    pub fn prune(
        self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
    ) -> PactResult<(Self, Value)> {
        let mut resolver = PactResolver::from_ledger(self);
        let report = resolver.delete(
            branch_id,
            target_component_id,
            target_selector,
            BoundaryKind::Authored,
        )?;
        Ok((resolver.into_ledger(), report))
    }
}

pub fn new_store_json(pact_json: &str, blob_inline_limit: usize) -> PactResult<String> {
    let base: Value = serde_json::from_str(pact_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))?;
    let current = build_projection(&base, &BTreeMap::new(), blob_inline_limit)?;
    let store = PactStore {
        schema: STORE_SCHEMA.to_string(),
        base,
        head_revision: 0,
        current,
    };
    encode_store(&store)
}

pub fn new_ledger_json(
    pact_json: &str,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    new_ledger_json_with_base_policy(pact_json, branch_id, blob_inline_limit, true)
}

pub fn new_ledger_json_without_base(
    pact_json: &str,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    new_ledger_json_with_base_policy(pact_json, branch_id, blob_inline_limit, false)
}

fn new_ledger_json_with_base_policy(
    pact_json: &str,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
    include_base_content: bool,
) -> PactResult<String> {
    let base: Value = serde_json::from_str(pact_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))?;
    let root_store = new_root_store_from_value_with_base_policy(
        base,
        branch_id,
        blob_inline_limit,
        include_base_content,
    )?;
    encode_root_store(&root_store)
}

pub fn empty_context_root_value(root_id: &str) -> Value {
    json!({
        "id": root_id,
        "componentType": "root",
        "type": "root",
        "children": [
            {
                "id": "system",
                "componentType": "system_header",
                "type": "system_header",
                "role": "system",
                "pact_depth": -1,
                "depth": -1,
                "offset": 0,
                "children": [],
            },
            {
                "id": "active",
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "user",
                "pact_depth": 0,
                "depth": 0,
                "offset": 0,
                "children": [],
            },
        ],
    })
}

pub fn bootstrap_ledger(
    context_tree_id: &str,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<ContextTreeLedger> {
    new_root_store_from_value_with_base_policy(
        empty_context_root_value(context_tree_id),
        branch_id,
        blob_inline_limit,
        true,
    )
}

pub fn bootstrap_ledger_json(
    context_tree_id: &str,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    encode_root_store(&bootstrap_ledger(
        context_tree_id,
        branch_id,
        blob_inline_limit,
    )?)
}

fn new_root_store_from_value_with_base_policy(
    mut base: Value,
    branch_id: Option<&str>,
    blob_inline_limit: usize,
    include_base_content: bool,
) -> PactResult<ContextTreeLedger> {
    let mut content = ContentStore::default();
    externalize_large_content_in_value(&mut base, &mut content, blob_inline_limit);
    let projection = build_projection(&base, &BTreeMap::new(), blob_inline_limit)?;
    let root_id = projection.tree.root_id.clone();
    let branch_id = branch_id.unwrap_or("main").to_string();
    let mut view = PactView::from_projection(projection, branch_id.clone(), 0);
    sync_view_content_refs_from_store(&mut view, &content);
    let base_content_ref = if include_base_content {
        let base_json = serde_json::to_string(&base)
            .map_err(|err| PactError::codec(format!("failed to encode branch base: {err}")))?;
        Some(
            content
                .put_inline("pact.branch.base", &base_json, blob_inline_limit)
                .id,
        )
    } else {
        None
    };
    let base_ref = PactRef {
        root_id: root_id.clone(),
        branch_id: branch_id.clone(),
        revision: Some(0),
        transaction_id: None,
        component_id: None,
        content_ref: base_content_ref,
    };
    let branch = PactBranch {
        branch_id: branch_id.clone(),
        root_id: root_id.clone(),
        parent_branch_id: None,
        fork_ref: None,
        base_ref,
        head_revision: 0,
        lifecycle_tick: 0,
        transaction_traces: Vec::new(),
        current: view,
        aux: BTreeMap::new(),
    };
    let mut branches = BTreeMap::new();
    branches.insert(branch_id.clone(), branch);
    let root_store = ContextTreeLedger {
        history: Default::default(),
        schema: ROOT_STORE_SCHEMA.to_string(),
        root_id,
        parent: None,
        base: None,
        branches,
        content,
        event_index: BranchEventIndex::default(),
        current_branch_id: branch_id,
    };
    Ok(root_store)
}

pub fn externalize_large_content_in_value(
    value: &mut Value,
    content: &mut ContentStore,
    blob_inline_limit: usize,
) {
    match value {
        Value::Object(object) => {
            externalize_media_payload(object, content);
            externalize_text_slot(
                object,
                content,
                "content",
                "content_ref",
                "node.content",
                "content",
                blob_inline_limit,
                true,
            );
            externalize_json_slot(
                object,
                content,
                "parameters",
                "parameters_ref",
                "node.parameters",
                blob_inline_limit,
            );
            externalize_json_slot(
                object,
                content,
                "result",
                "result_ref",
                "node.result",
                blob_inline_limit,
            );
            externalize_text_slot(
                object,
                content,
                "error_message",
                "error_ref",
                "node.error",
                "error_message",
                blob_inline_limit,
                false,
            );
            for child in object.values_mut() {
                externalize_large_content_in_value(child, content, blob_inline_limit);
            }
        }
        Value::Array(items) => {
            for item in items {
                externalize_large_content_in_value(item, content, blob_inline_limit);
            }
        }
        _ => {}
    }
}

fn extension_bag_key(object: &Map<String, Value>) -> &'static str {
    if object.get("aux").is_some_and(Value::is_object) {
        "aux"
    } else if object.get("attrs").is_some_and(Value::is_object) {
        "attrs"
    } else {
        "aux"
    }
}

fn externalize_media_payload(object: &mut Map<String, Value>, content: &mut ContentStore) {
    let node_type = object
        .get("componentType")
        .or_else(|| object.get("node_type"))
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let media_kind: String = match node_type {
        "media_content" => object
            .get("media_kind")
            .and_then(Value::as_str)
            .unwrap_or("media")
            .to_string(),
        "image_content" | "ImageContent" => "image".to_string(),
        "audio_content" | "AudioContent" => "audio".to_string(),
        "video_content" | "VideoContent" => "video".to_string(),
        "document_content" | "DocumentContent" => "document".to_string(),
        _ => return,
    };
    let media_kind = media_kind.as_str();
    if object.get("content_ref").is_some() || object.get("media_content_ref").is_some() {
        return;
    }

    let bag_key = extension_bag_key(object);
    let Some((bytes, mime_type)) = take_media_bytes(object, bag_key) else {
        return;
    };
    let preview = object
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get(bag_key)
                .and_then(Value::as_object)
                .and_then(|bag| bag.get("filename"))
                .and_then(Value::as_str)
        })
        .unwrap_or(media_kind);
    let kind = format!("media.{media_kind}");
    let hash_key = mime_type.as_deref().unwrap_or(media_kind);
    let record = content.put_bytes(&kind, &bytes, preview, hash_key);
    let Some(bag) = object
        .entry(bag_key.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
    else {
        return;
    };
    bag.insert(
        "media_content_ref".to_string(),
        json!({
            "id": record.id,
            "field": "media_content",
            "mime_type": mime_type,
        }),
    );
}

fn take_media_bytes(
    object: &mut Map<String, Value>,
    bag_key: &str,
) -> Option<(Vec<u8>, Option<String>)> {
    let mut mime_type = None;
    if let Some(bag) = object.get_mut(bag_key).and_then(Value::as_object_mut) {
        mime_type = bag
            .get("mime_type")
            .or_else(|| bag.get("media_type"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(media_content) = bag.get_mut("media_content").and_then(Value::as_object_mut)
            && let Some(data) = media_content.get("data").and_then(Value::as_str)
        {
            let encoding = media_content
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("base64");
            if encoding == "base64"
                && let Ok(bytes) = base64_decode(data)
            {
                media_content.remove("data");
                media_content.remove("encoding");
                if media_content.is_empty() {
                    bag.remove("media_content");
                }
                return Some((bytes, mime_type));
            }
        }
        for key in ["media_data", "data"] {
            if let Some(data) = bag.get(key).and_then(Value::as_str)
                && let Some(bytes) = decode_media_data(data)
            {
                bag.remove(key);
                bag.remove("encoding");
                return Some((bytes, mime_type));
            }
        }
    }
    for key in ["media_data", "data"] {
        if let Some(data) = object.get(key).and_then(Value::as_str)
            && let Some(bytes) = decode_media_data(data)
        {
            object.remove(key);
            object.remove("encoding");
            return Some((bytes, mime_type));
        }
    }
    None
}

fn decode_media_data(data: &str) -> Option<Vec<u8>> {
    let payload = data
        .split_once(";base64,")
        .map(|(_, payload)| payload)
        .unwrap_or(data);
    base64_decode(payload).ok()
}

#[allow(clippy::too_many_arguments)]
fn externalize_text_slot(
    object: &mut Map<String, Value>,
    content: &mut ContentStore,
    slot_key: &str,
    ref_key: &str,
    content_kind: &str,
    field: &str,
    blob_inline_limit: usize,
    keep_preview: bool,
) {
    if object.get(ref_key).is_some() {
        return;
    }
    let Some(text) = object.get(slot_key).and_then(Value::as_str) else {
        return;
    };
    if text.len() <= blob_inline_limit {
        return;
    }
    let record = content.put_inline(content_kind, text, blob_inline_limit);
    object.remove(slot_key);
    if keep_preview {
        object.insert(
            "content_preview".to_string(),
            Value::String(record.preview.clone()),
        );
        object.insert(ref_key.to_string(), Value::String(record.id));
    } else {
        object.insert(ref_key.to_string(), content_ref_value(&record.id, field));
    }
}

fn externalize_json_slot(
    object: &mut Map<String, Value>,
    content: &mut ContentStore,
    slot_key: &str,
    ref_key: &str,
    content_kind: &str,
    blob_inline_limit: usize,
) {
    if object.get(ref_key).is_some() {
        return;
    }
    let Some(value) = object.get(slot_key) else {
        return;
    };
    let Ok(encoded) = serde_json::to_string(value) else {
        return;
    };
    if encoded.len() <= blob_inline_limit {
        return;
    }
    let record = content.put_inline(content_kind, &encoded, blob_inline_limit);
    object.remove(slot_key);
    object.insert(ref_key.to_string(), content_ref_value(&record.id, slot_key));
}

fn content_ref_value(content_id: &str, field: &str) -> Value {
    json!({
        "id": content_id,
        "field": field,
    })
}

pub(super) fn sync_view_content_refs_from_store(view: &mut PactView, content: &ContentStore) {
    let refs = view
        .component_records()
        .values()
        .flat_map(node_content_ref_ids)
        .collect::<BTreeSet<_>>();
    view.content_refs
        .retain(|content_id, _| refs.contains(content_id));
    for content_id in refs {
        if let Some(record) = content.records.get(&content_id) {
            view.content_refs.insert(content_id, record.summary());
        }
    }
}

fn node_content_ref_ids(node: &super::model::ComponentRecord) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(content_ref) = &node.content_ref {
        refs.push(content_ref.clone());
    }
    for value in [&node.parameters_ref, &node.result_ref, &node.error_ref]
        .into_iter()
        .flatten()
    {
        if let Some(content_id) = content_ref_id_from_value(value) {
            refs.push(content_id);
        }
    }
    refs
}

fn content_ref_id_from_value(value: &Value) -> Option<String> {
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

pub fn ledger_from_store_json(store_json: &str, branch_id: Option<&str>) -> PactResult<String> {
    let store = decode_store(store_json)?;
    let root_id = store.current.tree.root_id.clone();
    let branch_id = branch_id.unwrap_or("main").to_string();
    let view = PactView::from_projection(store.current, branch_id.clone(), store.head_revision);
    let base_json = serde_json::to_string(&store.base)
        .map_err(|err| PactError::codec(format!("failed to encode branch base: {err}")))?;
    let mut content = ContentStore::default();
    let base_record = content.put_inline("pact.branch.base", &base_json, 1024);
    let base_ref = PactRef {
        root_id: root_id.clone(),
        branch_id: branch_id.clone(),
        revision: Some(0),
        transaction_id: None,
        component_id: None,
        content_ref: Some(base_record.id),
    };
    let branch = PactBranch {
        branch_id: branch_id.clone(),
        root_id: root_id.clone(),
        parent_branch_id: None,
        fork_ref: None,
        base_ref,
        head_revision: store.head_revision,
        lifecycle_tick: 0,
        transaction_traces: Vec::new(),
        current: view,
        aux: BTreeMap::new(),
    };
    let mut branches = BTreeMap::new();
    branches.insert(branch_id.clone(), branch);
    let root_store = ContextTreeLedger {
        history: Default::default(),
        schema: ROOT_STORE_SCHEMA.to_string(),
        root_id,
        parent: None,
        base: None,
        branches,
        content,
        event_index: BranchEventIndex::default(),
        current_branch_id: branch_id,
    };
    encode_root_store(&root_store)
}

pub fn ledger_create_branch_at_revision_json(
    root_store_json: &str,
    from_branch_id: &str,
    revision: Option<u64>,
    new_branch_id: Option<&str>,
    aux_json: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let aux = parse_aux(aux_json)?;
    let (root_store, _branch_id) = root_store.fork_at_revision(
        from_branch_id,
        revision,
        new_branch_id.map(str::to_string),
        aux,
        blob_inline_limit,
    )?;
    encode_root_store(&root_store)
}

pub fn ledger_create_branch_at_node_json(
    root_store_json: &str,
    from_branch_id: &str,
    component_id: &str,
    new_branch_id: Option<&str>,
    aux_json: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let aux = parse_aux(aux_json)?;
    let (root_store, _branch_id) = root_store.fork_at_component(
        from_branch_id,
        component_id,
        new_branch_id.map(str::to_string),
        aux,
        blob_inline_limit,
    )?;
    encode_root_store(&root_store)
}

pub fn ledger_record_transaction_trace_json(
    root_store_json: &str,
    trace_json: &str,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let trace: PactTransactionTrace = serde_json::from_str(trace_json)
        .map_err(|err| PactError::invalid(format!("invalid transaction trace JSON: {err}")))?;
    let root_store = root_store.record_transaction_trace(trace)?;
    encode_root_store(&root_store)
}

pub fn ledger_create_branch_at_transaction_json(
    root_store_json: &str,
    from_branch_id: &str,
    transaction_id: &str,
    new_branch_id: Option<&str>,
    aux_json: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let aux = parse_aux(aux_json)?;
    let (root_store, _branch_id) = root_store.fork_at_transaction(
        from_branch_id,
        transaction_id,
        new_branch_id.map(str::to_string),
        aux,
        blob_inline_limit,
    )?;
    encode_root_store(&root_store)
}

pub fn ledger_project_branch_tree_json(
    root_store_json: &str,
    branch_id: &str,
    revision: Option<u64>,
) -> PactResult<String> {
    ledger_project_branch_tree_json_with_scope(
        root_store_json,
        branch_id,
        revision,
        ProjectionTreeScope::Head,
    )
}

pub fn ledger_project_branch_tree_json_with_scope(
    root_store_json: &str,
    branch_id: &str,
    revision: Option<u64>,
    scope: ProjectionTreeScope,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let branch = root_store
        .branches
        .get(branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    let mut projected = match scope {
        ProjectionTreeScope::Head => {
            project_head_snapshot(&root_store, branch, revision)?.project_value()
        }
        ProjectionTreeScope::Archive => {
            project_branch_archive_tree_value(&root_store, branch, revision)?.0
        }
    };
    hydrate_content_refs_in_value(&mut projected, &root_store.content);
    serde_json::to_string(&projected)
        .map_err(|err| PactError::codec(format!("failed to encode branch projection: {err}")))
}

pub fn ledger_project_branch_json(
    root_store_json: &str,
    branch_id: &str,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    ledger_project_branch_json_with_scope(
        root_store_json,
        branch_id,
        revision,
        blob_inline_limit,
        ProjectionTreeScope::Head,
    )
}

pub fn ledger_project_branch_with_scope(
    root_store: &ContextTreeLedger,
    branch_id: &str,
    revision: Option<u64>,
    blob_inline_limit: usize,
    scope: ProjectionTreeScope,
) -> PactResult<PactView> {
    root_store.project_branch_with_scope(branch_id, revision, blob_inline_limit, scope)
}

pub fn ledger_project_branch(
    root_store: &ContextTreeLedger,
    branch_id: &str,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<PactView> {
    root_store.project_branch(branch_id, revision, blob_inline_limit)
}

pub fn ledger_project_branch_json_with_scope(
    root_store_json: &str,
    branch_id: &str,
    revision: Option<u64>,
    blob_inline_limit: usize,
    scope: ProjectionTreeScope,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let view = ledger_project_branch_with_scope(
        &root_store,
        branch_id,
        revision,
        blob_inline_limit,
        scope,
    )?;
    serde_json::to_string(&project_view_json_value(&view)?)
        .map_err(|err| PactError::codec(format!("failed to encode branch view: {err}")))
}

fn project_view_json_value(view: &PactView) -> PactResult<Value> {
    let mut value = serde_json::to_value(view)
        .map_err(|err| PactError::codec(format!("failed to encode branch view: {err}")))?;
    if let Some(object) = value.as_object_mut() {
        let components = view
            .component_records()
            .into_iter()
            .map(|(id, record)| (id, serde_json::to_value(record).unwrap_or(Value::Null)))
            .collect::<serde_json::Map<String, Value>>();
        object.insert("components".to_string(), Value::Object(components));
        object.insert(
            "first_seen".to_string(),
            serde_json::to_value(&view.tree.first_seen).unwrap_or(Value::Null),
        );
        object.insert(
            "tool_pairs".to_string(),
            serde_json::to_value(&view.tree.tool_pairs).unwrap_or(Value::Null),
        );
    }
    Ok(value)
}

pub fn ledger_inspect_json(root_store_json: &str) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    serde_json::to_string(&root_store.inspect())
        .map_err(|err| PactError::codec(format!("failed to encode root inspection: {err}")))
}

pub fn ledger_inspect_branch_json(root_store_json: &str, branch_id: &str) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    serde_json::to_string(&root_store.inspect_branch(branch_id)?)
        .map_err(|err| PactError::codec(format!("failed to encode branch inspection: {err}")))
}

pub fn ledger_delete_json(
    root_store_json: &str,
    branch_id: &str,
    target_component_id: Option<&str>,
    target_selector: Option<&str>,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let (root_store, _report) = root_store.prune(
        branch_id,
        target_component_id.map(str::to_string),
        target_selector.map(str::to_string),
    )?;
    encode_root_store(&root_store)
}

pub fn project_store_json(store_json: &str, revision: Option<u64>) -> PactResult<String> {
    let store = decode_store(store_json)?;
    let tree = store.project_value(revision)?;
    serde_json::to_string(&tree)
        .map_err(|err| PactError::codec(format!("failed to encode projected PACT: {err}")))
}

pub fn checkpoint_store_json(
    store_json: &str,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let store = decode_store(store_json)?;
    let rev = revision.unwrap_or(store.head_revision);
    let tree = store.project_value(Some(rev))?;
    let projection = build_projection(
        &tree,
        &store.first_seen_by_replay(Some(rev), blob_inline_limit)?,
        blob_inline_limit,
    )?;
    let mut checkpoint = Map::new();
    checkpoint.insert(
        "schema".to_string(),
        Value::String(CHECKPOINT_SCHEMA.to_string()),
    );
    checkpoint.insert("revision".to_string(), Value::Number(rev.into()));
    checkpoint.insert("tree".to_string(), tree);
    checkpoint.insert(
        "projection".to_string(),
        serde_json::to_value(projection)
            .map_err(|err| PactError::codec(format!("failed to encode projection: {err}")))?,
    );
    serde_json::to_string(&Value::Object(checkpoint))
        .map_err(|err| PactError::codec(format!("failed to encode checkpoint: {err}")))
}

pub fn fork_store_json(
    store_json: &str,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let store = decode_store(store_json)?;
    let rev = revision.unwrap_or(store.head_revision);
    let base = store.project_value(Some(rev))?;
    let current = build_projection(&base, &BTreeMap::new(), blob_inline_limit)?;
    let forked = PactStore {
        schema: STORE_SCHEMA.to_string(),
        base,
        head_revision: 0,
        current,
    };
    encode_store(&forked)
}

pub fn content_store_put_inline_json(
    store_json: Option<&str>,
    kind: &str,
    content: &str,
    preview_chars: usize,
) -> PactResult<String> {
    let mut store = if let Some(store_json) = store_json {
        serde_json::from_str::<ContentStore>(store_json)
            .map_err(|err| PactError::invalid(format!("invalid content store JSON: {err}")))?
    } else {
        ContentStore::default()
    };
    store.rebuild_hash_index();
    store.put_inline(kind, content, preview_chars);
    serde_json::to_string(&store)
        .map_err(|err| PactError::codec(format!("failed to encode content store: {err}")))
}

pub fn transaction_trace_validate_json(trace_json: &str) -> PactResult<String> {
    let trace: PactTransactionTrace = serde_json::from_str(trace_json)
        .map_err(|err| PactError::invalid(format!("invalid transaction trace JSON: {err}")))?;
    trace.validate()?;
    serde_json::to_string(&trace)
        .map_err(|err| PactError::codec(format!("failed to encode transaction trace: {err}")))
}

pub fn transaction_trace_build_json(request_json: &str) -> PactResult<String> {
    let request: PactTransactionTraceBuildRequest =
        serde_json::from_str(request_json).map_err(|err| {
            PactError::invalid(format!(
                "invalid transaction trace build request JSON: {err}"
            ))
        })?;
    let trace = PactTransactionTrace::build(request)?;
    serde_json::to_string(&trace)
        .map_err(|err| PactError::codec(format!("failed to encode transaction trace: {err}")))
}
