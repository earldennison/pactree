use crate::components::{Component, ComponentKind, MediaBody, MediaPayload};
use crate::payload_hydration::hydrate_payload_refs;
use crate::store::ContentStore;
use crate::tree::ComponentTree;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub const PROVIDER_THREAD_SCHEMA: &str = "pact.provider_thread.v1";

pub type ProviderThreadView = Value;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderThreadOptions {
    #[serde(default)]
    pub include_partial_tool_calls: bool,

    #[serde(default = "default_true")]
    pub fail_on_orphan_tool_calls: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProviderThreadOptions {
    fn default() -> Self {
        Self {
            include_partial_tool_calls: false,
            fail_on_orphan_tool_calls: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderThreadError {
    pub schema: String,
    pub reason: String,
    pub offending_tool_call_ids: Vec<String>,
}

impl std::fmt::Display for ProviderThreadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)?;
        if !self.offending_tool_call_ids.is_empty() {
            write!(
                f,
                " (tool calls: {})",
                self.offending_tool_call_ids.join(", ")
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderThreadError {}

pub fn build_provider_thread(
    serialized_projection: &Value,
    options: &ProviderThreadOptions,
) -> Result<Value, ProviderThreadError> {
    let tree =
        ComponentTree::from_value(serialized_projection).map_err(|err| ProviderThreadError {
            schema: PROVIDER_THREAD_SCHEMA.to_string(),
            reason: format!("invalid provider-thread component tree: {err}"),
            offending_tool_call_ids: vec![],
        })?;
    build_provider_thread_tree(&tree, options)
}

pub(crate) fn build_provider_thread_tree(
    tree: &ComponentTree,
    options: &ProviderThreadOptions,
) -> Result<Value, ProviderThreadError> {
    let unpaired = unpaired_tool_call_ids_tree(tree);
    if !unpaired.is_empty()
        && !options.include_partial_tool_calls
        && options.fail_on_orphan_tool_calls
    {
        return Err(ProviderThreadError {
            schema: PROVIDER_THREAD_SCHEMA.to_string(),
            reason: format!(
                "{} tool call(s) unpaired; provider surface refuses to emit",
                unpaired.len()
            ),
            offending_tool_call_ids: unpaired,
        });
    }
    let unpaired_tool_call_ids = unpaired_tool_call_ids_tree(tree)
        .into_iter()
        .collect::<BTreeSet<_>>();

    let mut messages: Vec<Value> = Vec::new();

    if let Some(header) = build_system_header_tree(tree) {
        messages.push(header);
    }

    let mut pending_tool_call_ids = BTreeSet::new();
    let mut history_turns = collect_history_turns_tree(tree);
    history_turns.sort_by_key(|node| -i64::from(node.pact_depth.unwrap_or(0)));
    for turn in history_turns {
        messages.extend(build_turn_messages_tree(
            turn,
            options,
            &unpaired_tool_call_ids,
            &mut pending_tool_call_ids,
        ));
    }

    if let Some(active) = find_active_turn_tree(tree) {
        messages.extend(build_turn_messages_tree(
            active,
            options,
            &unpaired_tool_call_ids,
            &mut pending_tool_call_ids,
        ));
    }

    Ok(json!({
        "messages": messages,
    }))
}

pub fn build_provider_thread_with_content_store(
    serialized_projection: &Value,
    content_store: &ContentStore,
    options: &ProviderThreadOptions,
) -> Result<Value, ProviderThreadError> {
    let mut hydrated = serialized_projection.clone();
    hydrate_payload_refs(&mut hydrated, content_store).map_err(|reason| ProviderThreadError {
        schema: PROVIDER_THREAD_SCHEMA.to_string(),
        reason: reason.to_string(),
        offending_tool_call_ids: vec![],
    })?;
    let hydrated = ComponentTree::from_value(&hydrated).map_err(|err| ProviderThreadError {
        schema: PROVIDER_THREAD_SCHEMA.to_string(),
        reason: format!("invalid provider-thread component tree: {err}"),
        offending_tool_call_ids: vec![],
    })?;
    build_provider_thread_tree(&hydrated, options)
}

pub(crate) fn build_provider_thread_tree_with_content_store(
    tree: &ComponentTree,
    content_store: &ContentStore,
    options: &ProviderThreadOptions,
) -> Result<Value, ProviderThreadError> {
    let mut hydrated = tree.project_value();
    hydrate_payload_refs(&mut hydrated, content_store).map_err(|reason| ProviderThreadError {
        schema: PROVIDER_THREAD_SCHEMA.to_string(),
        reason: reason.to_string(),
        offending_tool_call_ids: vec![],
    })?;
    let hydrated = ComponentTree::from_value(&hydrated).map_err(|err| ProviderThreadError {
        schema: PROVIDER_THREAD_SCHEMA.to_string(),
        reason: format!("invalid hydrated provider-thread component tree: {err}"),
        offending_tool_call_ids: vec![],
    })?;
    build_provider_thread_tree(&hydrated, options)
}

fn build_system_header_tree(tree: &ComponentTree) -> Option<Value> {
    let header_node = find_component_by_type(tree.root(), "system_header")?;
    let mut content_blocks: Vec<Value> = Vec::new();
    collect_text_blocks_component(header_node, &mut content_blocks);

    if content_blocks.is_empty() {
        return None;
    }
    Some(json!({
        "message_type": "system_header",
        "content": content_blocks,
    }))
}

fn build_turn_messages_tree(
    turn_node: &Component,
    options: &ProviderThreadOptions,
    unpaired_tool_call_ids: &BTreeSet<String>,
    pending_tool_call_ids: &mut BTreeSet<String>,
) -> Vec<Value> {
    let role = turn_node.role.as_deref().unwrap_or("user").to_string();

    let mut content_blocks: Vec<Value> = Vec::new();
    let current_turn_call_ids =
        collect_emittable_tool_call_ids_component(turn_node, options, unpaired_tool_call_ids);
    let mut emitted_result_ids = BTreeSet::new();
    walk_turn_for_blocks_component(
        turn_node,
        options,
        unpaired_tool_call_ids,
        &mut content_blocks,
        pending_tool_call_ids,
        &current_turn_call_ids,
        &mut emitted_result_ids,
    );
    for tool_call_id in emitted_result_ids {
        pending_tool_call_ids.remove(&tool_call_id);
    }

    if content_blocks.is_empty() {
        return Vec::new();
    }

    if role == "assistant" {
        let mut provider_blocks: Vec<Value> = Vec::new();
        let mut tool_result_blocks: Vec<Value> = Vec::new();
        for block in content_blocks {
            if block.get("success").is_some() || block.get("message").is_some() {
                tool_result_blocks.push(block);
            } else {
                provider_blocks.push(block);
            }
        }

        let mut messages = Vec::new();
        if !provider_blocks.is_empty() {
            let mut message = json!({
                "message_type": "provider_response",
                "content": provider_blocks,
            });
            let metadata = collect_assistant_metadata_component(turn_node);
            if !metadata.is_empty() {
                message["metadata"] = Value::Object(metadata);
            }
            messages.push(message);
        }
        if !tool_result_blocks.is_empty() {
            messages.push(json!({
                "message_type": "client_request",
                "content": tool_result_blocks,
            }));
        }
        messages
    } else {
        vec![json!({
            "message_type": "client_request",
            "content": content_blocks,
        })]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolWireKind {
    Call,
    Result,
}

pub(crate) fn unpaired_tool_call_ids_tree(tree: &ComponentTree) -> Vec<String> {
    tree.indexed_unpaired_tool_call_ids()
}

fn tool_wire_kind_component(node: &Component) -> Option<ToolWireKind> {
    component_tool_call_id(node)?;

    match node.component_type() {
        "tool_call" => return Some(ToolWireKind::Call),
        "tool_result" => return Some(ToolWireKind::Result),
        _ => {}
    }

    if node.success().is_some()
        || node.error_message().is_some()
        || node.result().is_some()
        || node.aux.contains_key("success")
        || node.aux.contains_key("error_message")
        || node.aux.contains_key("result")
    {
        return Some(ToolWireKind::Result);
    }

    if node.parameters().is_some()
        || node.tool_name().is_some()
        || node.aux.contains_key("parameters")
        || node.aux.contains_key("tool_name")
    {
        return Some(ToolWireKind::Call);
    }

    None
}

fn component_tool_call_id(node: &Component) -> Option<&str> {
    node.tool_call_id()
        .or_else(|| node.aux.get("tool_call_id").and_then(Value::as_str))
}

fn collect_assistant_metadata_component(turn_node: &Component) -> Map<String, Value> {
    let _ = turn_node;
    Map::new()
}

fn walk_turn_for_blocks_component(
    node: &Component,
    options: &ProviderThreadOptions,
    unpaired_tool_call_ids: &BTreeSet<String>,
    out: &mut Vec<Value>,
    pending_tool_call_ids: &mut BTreeSet<String>,
    current_turn_call_ids: &BTreeSet<String>,
    emitted_result_ids: &mut BTreeSet<String>,
) {
    for child in &node.children {
        match tool_wire_kind_component(child) {
            Some(ToolWireKind::Call) => {
                if let Some(id) = component_tool_call_id(child)
                    && unpaired_tool_call_ids.contains(id)
                    && !options.include_partial_tool_calls
                {
                    continue;
                }
                if let Some(block) = build_provider_tool_call_component(child) {
                    if let Some(id) = component_tool_call_id(child) {
                        pending_tool_call_ids.insert(id.to_string());
                    }
                    out.push(block);
                }
                continue;
            }
            Some(ToolWireKind::Result) => {
                let Some(tool_call_id) = component_tool_call_id(child) else {
                    continue;
                };
                if !pending_tool_call_ids.contains(tool_call_id)
                    && !current_turn_call_ids.contains(tool_call_id)
                {
                    continue;
                }
                if let Some(block) = build_client_tool_response_component(child) {
                    emitted_result_ids.insert(tool_call_id.to_string());
                    out.push(block);
                }
                continue;
            }
            None => {}
        }
        match child.component_type() {
            "text_content" => {
                if let Some(text) = child.content()
                    && !text.is_empty()
                {
                    out.push(json!({"content": text}));
                }
            }
            "reasoning_content" => {
                if let Some(block) = build_reasoning_content_component(child) {
                    out.push(block);
                }
            }
            "media_content" | "image_content" | "audio_content" | "video_content"
            | "document_content" => {
                if let Some(block) = build_multimedia_content_component(child) {
                    out.push(block);
                }
            }
            "message_container" | "container" => {
                walk_turn_for_blocks_component(
                    child,
                    options,
                    unpaired_tool_call_ids,
                    out,
                    pending_tool_call_ids,
                    current_turn_call_ids,
                    emitted_result_ids,
                );
            }
            _ => {
                if let Some(text) = child.content()
                    && !text.is_empty()
                {
                    out.push(json!({"content": text}));
                }
            }
        }
    }
}

fn build_reasoning_content_component(node: &Component) -> Option<Value> {
    let visibility = component_aux_string(node, "visibility").unwrap_or("displayable");
    let visibility = if visibility == "opaque" {
        "opaque"
    } else {
        "displayable"
    };
    let provider = component_aux_string(node, "provider").map(str::to_string);
    let encoding = component_aux_string(node, "encoding").map(str::to_string);
    let data = component_aux_string(node, "data")
        .or_else(|| component_aux_string(node, "encrypted_content"))
        .map(str::to_string);
    let content = if visibility == "displayable" {
        node.content()
            .or_else(|| component_aux_string(node, "content"))
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };

    if visibility == "displayable" && content.is_empty() {
        return None;
    }
    if visibility == "opaque" && data.as_deref().unwrap_or("").is_empty() {
        return None;
    }

    let mut block = Map::new();
    block.insert("type".to_string(), json!("reasoning_content"));
    block.insert("content".to_string(), json!(content));
    block.insert("visibility".to_string(), json!(visibility));
    if let Some(provider) = provider {
        block.insert("provider".to_string(), json!(provider));
    }
    if let Some(encoding) = encoding {
        block.insert("encoding".to_string(), json!(encoding));
    }
    if let Some(data) = data {
        block.insert("data".to_string(), json!(data));
    }
    Some(Value::Object(block))
}

fn collect_emittable_tool_call_ids_component(
    node: &Component,
    options: &ProviderThreadOptions,
    unpaired_tool_call_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_emittable_tool_call_ids_component_inner(
        node,
        options,
        unpaired_tool_call_ids,
        &mut out,
    );
    out
}

fn collect_emittable_tool_call_ids_component_inner(
    node: &Component,
    options: &ProviderThreadOptions,
    unpaired_tool_call_ids: &BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    if matches!(tool_wire_kind_component(node), Some(ToolWireKind::Call))
        && build_provider_tool_call_component(node).is_some()
        && let Some(tool_call_id) = component_tool_call_id(node)
        && (!unpaired_tool_call_ids.contains(tool_call_id) || options.include_partial_tool_calls)
    {
        out.insert(tool_call_id.to_string());
    }
    for child in &node.children {
        collect_emittable_tool_call_ids_component_inner(
            child,
            options,
            unpaired_tool_call_ids,
            out,
        );
    }
}

fn build_provider_tool_call_component(node: &Component) -> Option<Value> {
    let tool_call_id = component_tool_call_id(node)?;
    let tool_name = node
        .tool_name()
        .or_else(|| node.aux.get("operation_name").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    let parameters = node
        .parameters()
        .cloned()
        .or_else(|| node.aux.get("parameters").cloned())
        .map(normalize_json_object_string)
        .or_else(|| {
            node.content()
                .and_then(|content| serde_json::from_str::<Value>(content).ok())
                .filter(Value::is_object)
        })
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));

    Some(json!({
        "tool_name": tool_name,
        "tool_call_id": tool_call_id,
        "parameters": parameters,
    }))
}

fn normalize_json_object_string(value: Value) -> Value {
    match value {
        Value::String(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .unwrap_or(Value::String(text)),
        other => other,
    }
}

fn component_aux_string<'a>(node: &'a Component, key: &str) -> Option<&'a str> {
    node.aux.get(key).and_then(Value::as_str)
}

fn build_multimedia_content_component(node: &Component) -> Option<Value> {
    let ComponentKind::Media(media) = &node.kind else {
        return build_unknown_multimedia_content_component(node);
    };
    let media_kind = media.media_kind.as_str();
    let content = media_label_component(node, Some(media), media_kind);
    let media_content = explicit_media_content_component(node, Some(media))
        .or_else(|| typed_media_body_content(media))
        .or_else(|| media_url_ref_component(node, Some(media)).map(|url| json!({"url": url})))
        .or_else(|| {
            media_data_ref_component(node, Some(media)).map(|(data, encoding)| {
                json!({
                    "data": data,
                    "encoding": encoding,
                })
            })
        });

    let Some(media_content) = media_content else {
        if content.is_empty() {
            return None;
        }
        return Some(json!({"content": content}));
    };

    let mime_type = media
        .mime_type
        .as_deref()
        .or_else(|| component_media_aux_string(node, Some(media), "mime_type"))
        .unwrap_or_else(|| default_mime_type(media_kind))
        .to_string();

    let mut block = Map::new();
    block.insert("content".to_string(), Value::String(content));
    block.insert("media_content".to_string(), media_content);
    block.insert("mime_type".to_string(), Value::String(mime_type));

    copy_media_field_component(node, Some(media), &mut block, "filename");
    copy_media_field_component(node, Some(media), &mut block, "width");
    copy_media_field_component(node, Some(media), &mut block, "height");
    copy_media_field_component(node, Some(media), &mut block, "duration_ms");
    copy_media_field_component(node, Some(media), &mut block, "duration");
    copy_media_field_component(node, Some(media), &mut block, "format");
    copy_media_field_component(node, Some(media), &mut block, "page_count");

    Some(Value::Object(block))
}

fn build_unknown_multimedia_content_component(node: &Component) -> Option<Value> {
    let media_kind = media_kind_from_component_type(node.component_type());
    let content = media_label_component(node, None, media_kind);
    let media_content = explicit_media_content_component(node, None)
        .or_else(|| media_url_ref_component(node, None).map(|url| json!({"url": url})))
        .or_else(|| {
            media_data_ref_component(node, None).map(|(data, encoding)| {
                json!({
                    "data": data,
                    "encoding": encoding,
                })
            })
        });
    let Some(media_content) = media_content else {
        if content.is_empty() {
            return None;
        }
        return Some(json!({"content": content}));
    };

    let mime_type = component_media_aux_string(node, None, "mime_type")
        .unwrap_or_else(|| default_mime_type(media_kind))
        .to_string();
    let mut block = Map::new();
    block.insert("content".to_string(), Value::String(content));
    block.insert("media_content".to_string(), media_content);
    block.insert("mime_type".to_string(), Value::String(mime_type));
    copy_media_field_component(node, None, &mut block, "filename");
    copy_media_field_component(node, None, &mut block, "width");
    copy_media_field_component(node, None, &mut block, "height");
    copy_media_field_component(node, None, &mut block, "duration_ms");
    copy_media_field_component(node, None, &mut block, "duration");
    copy_media_field_component(node, None, &mut block, "format");
    copy_media_field_component(node, None, &mut block, "page_count");
    Some(Value::Object(block))
}

fn media_label_component(
    node: &Component,
    media: Option<&MediaPayload>,
    media_kind: &str,
) -> String {
    media
        .and_then(|media| match &media.body {
            MediaBody::InlineText { data, .. }
                if !data.is_empty() && !looks_like_media_ref(data) =>
            {
                Some(data.as_str())
            }
            _ => None,
        })
        .or_else(|| media.and_then(|media| media.label.as_deref()))
        .or_else(|| media.and_then(|media| media.filename.as_deref()))
        .or_else(|| node.content())
        .or_else(|| component_media_aux_string(node, media, "content"))
        .or_else(|| component_media_aux_string(node, media, "label"))
        .or_else(|| component_media_aux_string(node, media, "filename"))
        .unwrap_or_else(|| default_media_label(media_kind))
        .to_string()
}

fn explicit_media_content_component(
    node: &Component,
    media: Option<&MediaPayload>,
) -> Option<Value> {
    let value = component_media_aux_value(node, media, "media_content")?;
    if value.get("url").and_then(Value::as_str).is_some()
        || value.get("data").and_then(Value::as_str).is_some()
    {
        Some(value)
    } else {
        None
    }
}

fn typed_media_body_content(media: &MediaPayload) -> Option<Value> {
    match &media.body {
        MediaBody::Url { url } => Some(json!({"url": url})),
        MediaBody::InlineText { data, encoding }
            if !data.is_empty() && looks_like_media_ref(data) =>
        {
            Some(json!({"url": data}))
        }
        MediaBody::InlineText {
            data,
            encoding: Some(encoding),
        } if !data.is_empty() => Some(json!({
            "data": data,
            "encoding": encoding,
        })),
        _ => None,
    }
}

fn media_url_ref_component<'a>(
    node: &'a Component,
    media: Option<&'a MediaPayload>,
) -> Option<&'a str> {
    if let Some(MediaPayload {
        body: MediaBody::Url { url },
        ..
    }) = media
    {
        return Some(url);
    }
    for key in ["media_url", "url", "path", "file_path"] {
        if let Some(value) = component_media_aux_string(node, media, key) {
            return Some(value);
        }
    }
    let content = node.content()?;
    if looks_like_media_ref(content) {
        Some(content)
    } else {
        None
    }
}

fn media_data_ref_component<'a>(
    node: &'a Component,
    media: Option<&'a MediaPayload>,
) -> Option<(&'a str, &'a str)> {
    let data = component_media_aux_string(node, media, "media_data")
        .or_else(|| component_media_aux_string(node, media, "data"))?;
    let encoding = component_media_aux_string(node, media, "encoding").unwrap_or("base64");
    Some((data, encoding))
}

fn component_media_aux_value(
    node: &Component,
    media: Option<&MediaPayload>,
    key: &str,
) -> Option<Value> {
    media
        .and_then(|media| media.aux.get(key))
        .or_else(|| node.aux.get(key))
        .cloned()
}

fn component_media_aux_string<'a>(
    node: &'a Component,
    media: Option<&'a MediaPayload>,
    key: &str,
) -> Option<&'a str> {
    media
        .and_then(|media| media.aux.get(key))
        .or_else(|| node.aux.get(key))
        .and_then(Value::as_str)
}

fn looks_like_media_ref(value: &str) -> bool {
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("file://")
        || value.starts_with("data:")
}

fn media_kind_from_component_type(node_type: &str) -> &'static str {
    match node_type {
        "image_content" => "image",
        "audio_content" => "audio",
        "video_content" => "video",
        "document_content" => "document",
        _ => "media",
    }
}

fn default_media_label(media_kind: &str) -> &'static str {
    match media_kind {
        "image" => "image",
        "audio" => "audio",
        "video" => "video",
        "document" => "document",
        _ => "media",
    }
}

fn default_mime_type(media_kind: &str) -> &'static str {
    match media_kind {
        "image" => "image/png",
        "audio" => "audio/mpeg",
        "video" => "video/mp4",
        "document" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn copy_media_field_component(
    node: &Component,
    media: Option<&MediaPayload>,
    block: &mut Map<String, Value>,
    key: &str,
) {
    let typed = media.and_then(|media| match key {
        "filename" => media
            .filename
            .as_ref()
            .map(|value| Value::String(value.clone())),
        "width" => media.width.map(|value| Value::Number(value.into())),
        "height" => media.height.map(|value| Value::Number(value.into())),
        "duration_ms" => media.duration_ms.map(|value| Value::Number(value.into())),
        "page_count" => media.page_count.map(|value| Value::Number(value.into())),
        _ => None,
    });
    if let Some(value) = typed
        .or_else(|| media.and_then(|media| media.aux.get(key).cloned()))
        .or_else(|| node.aux.get(key).cloned())
        .filter(|value| !value.is_null())
    {
        block.insert(key.to_string(), value);
    }
}

fn build_client_tool_response_component(node: &Component) -> Option<Value> {
    let tool_call_id = component_tool_call_id(node)?.to_string();
    let tool_name = node
        .tool_name()
        .or_else(|| node.aux.get("operation_name").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    let success = node
        .success()
        .or_else(|| node.aux.get("success").and_then(Value::as_bool))
        .unwrap_or(true);
    let message = node
        .content()
        .map(str::to_string)
        .or_else(|| node.result().and_then(render_result_value))
        .or_else(|| node.aux.get("result").and_then(render_result_value))
        .or_else(|| node.error_message().map(str::to_string))
        .unwrap_or_default();

    let mut block = Map::new();
    block.insert("tool_call_id".to_string(), Value::String(tool_call_id));
    block.insert("tool_name".to_string(), Value::String(tool_name));
    block.insert("success".to_string(), Value::Bool(success));
    block.insert("message".to_string(), Value::String(message));

    Some(Value::Object(block))
}

fn render_result_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    serde_json::to_string(value).ok()
}

fn collect_text_blocks_component(node: &Component, out: &mut Vec<Value>) {
    for child in &node.children {
        let nt = child.component_type();
        if let Some(text) = child.content()
            && !text.is_empty()
            && nt != "system_header"
        {
            out.push(json!({"content": text}));
        }
        if matches!(nt, "message_container" | "container") {
            collect_text_blocks_component(child, out);
        }
    }
}

fn collect_history_turns_tree(tree: &ComponentTree) -> Vec<&Component> {
    let mut out: Vec<&Component> = Vec::new();
    for child in &tree.root().children {
        let nt = child.component_type();
        if nt == "conversation_history" {
            for turn in &child.children {
                if turn.component_type() == "message_turn" {
                    out.push(turn);
                }
            }
        } else if nt == "message_turn"
            && let Some(depth) = child.pact_depth
            && depth >= 1
        {
            out.push(child);
        }
    }
    out
}

fn find_active_turn_tree(tree: &ComponentTree) -> Option<&Component> {
    for child in &tree.root().children {
        let nt = child.component_type();
        if matches!(nt, "active_message" | "message_turn") && child.pact_depth.unwrap_or(0) == 0 {
            return Some(child);
        }
    }
    None
}

fn find_component_by_type<'a>(node: &'a Component, node_type: &str) -> Option<&'a Component> {
    if node.component_type() == node_type {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_component_by_type(child, node_type) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests;
