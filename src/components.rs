use crate::error::{PactError, PactResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COMPONENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn mint_component_local_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or_default();
    let counter = COMPONENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
    format!("{:08x}", nanos ^ counter)
}

pub(crate) fn now_created_at_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}

pub type PactAttrs = BTreeMap<String, Value>;

pub const PACT_COMPONENT_ESCAPE_HATCH: &str = "PACTComponent";

pub const CANONICAL_COMPONENT_KINDS: &[&str] = &[
    PACT_COMPONENT_ESCAPE_HATCH,
    "PactRoot",
    "SystemHeader",
    "ConversationHistory",
    "ActiveMessage",
    "MessageTurn",
    "MessageContainer",
    "TextContent",
    "ImageContent",
    "AudioContent",
    "VideoContent",
    "DocumentContent",
    "ToolCall",
    "ToolResult",
];

pub const DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES: usize = 1024;
pub const DEFAULT_PREVIEW_INLINE_LIMIT_BYTES: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadExternalizationStrategy {
    Inline,
    ContentRef,
    ExternalRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadSlotPolicy {
    pub field_path: &'static str,
    pub inline_byte_limit: usize,
    pub preview_byte_limit: usize,
    pub provider_visible: bool,
    pub strategy: PayloadExternalizationStrategy,
    pub source_required: bool,
}

impl PayloadSlotPolicy {
    pub const fn content_ref(
        field_path: &'static str,
        provider_visible: bool,
        source_required: bool,
    ) -> Self {
        Self {
            field_path,
            inline_byte_limit: DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES,
            preview_byte_limit: DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
            provider_visible,
            strategy: PayloadExternalizationStrategy::ContentRef,
            source_required,
        }
    }
}

pub const BUILT_IN_PAYLOAD_SLOT_POLICIES: &[PayloadSlotPolicy] = &[
    PayloadSlotPolicy::content_ref("$.content", true, false),
    PayloadSlotPolicy::content_ref("$.media_content", true, true),
    PayloadSlotPolicy::content_ref("$.data", true, true),
    PayloadSlotPolicy::content_ref("$.parameters", true, false),
    PayloadSlotPolicy::content_ref("$.result", true, true),
    PayloadSlotPolicy::content_ref("$.attrs", false, false),
    PayloadSlotPolicy::content_ref("$.aux", false, false),
    PayloadSlotPolicy::content_ref("$.aux", false, false),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalComponentKind {
    Component,
    PactRoot,
    SystemHeader,
    ConversationHistory,
    ActiveMessage,
    MessageTurn,
    MessageContainer,
    TextContent,
    ReasoningContent,
    ImageContent,
    AudioContent,
    VideoContent,
    DocumentContent,
    ToolCall,
    ToolResult,
}

impl CanonicalComponentKind {
    pub fn parse(value: &str) -> Result<Self, ComponentValidationError> {
        match value {
            "PACTComponent" | "pact_component" => Ok(Self::Component),
            "PactRoot" | "root" => Ok(Self::PactRoot),
            "SystemHeader" | "system_header" => Ok(Self::SystemHeader),
            "ConversationHistory" | "conversation_history" => Ok(Self::ConversationHistory),
            "ActiveMessage" | "active_message" => Ok(Self::ActiveMessage),
            "MessageTurn" | "message_turn" => Ok(Self::MessageTurn),
            "MessageContainer" | "message_container" => Ok(Self::MessageContainer),
            "TextContent" | "text_content" => Ok(Self::TextContent),
            "ReasoningContent" | "reasoning_content" => Ok(Self::ReasoningContent),
            "ImageContent" | "image_content" => Ok(Self::ImageContent),
            "AudioContent" | "audio_content" => Ok(Self::AudioContent),
            "VideoContent" | "video_content" => Ok(Self::VideoContent),
            "DocumentContent" | "document_content" => Ok(Self::DocumentContent),
            "ToolCall" | "tool_call" => Ok(Self::ToolCall),
            "ToolResult" | "tool_result" => Ok(Self::ToolResult),
            _ => Err(ComponentValidationError::UnsupportedHostObject {
                path: "$.kind".to_string(),
                detail: format!(
                    "unknown canonical component kind {value:?}; use PACTComponent for extension components"
                ),
            }),
        }
    }

    pub fn node_type(self) -> Option<&'static str> {
        match self {
            Self::Component => None,
            Self::PactRoot => Some("root"),
            Self::SystemHeader => Some("system_header"),
            Self::ConversationHistory => Some("conversation_history"),
            Self::ActiveMessage => Some("active_message"),
            Self::MessageTurn => Some("message_turn"),
            Self::MessageContainer => Some("message_container"),
            Self::TextContent => Some("text_content"),
            Self::ReasoningContent => Some("reasoning_content"),
            Self::ImageContent => Some("image_content"),
            Self::AudioContent => Some("audio_content"),
            Self::VideoContent => Some("video_content"),
            Self::DocumentContent => Some("document_content"),
            Self::ToolCall => Some("tool_call"),
            Self::ToolResult => Some("tool_result"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct ComponentType(pub String);

impl ComponentType {
    pub fn parse(value: impl Into<String>) -> Result<Self, ComponentValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ComponentValidationError::MissingComponentKind);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ComponentType {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ComponentType {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactContentRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ContentCapturePolicy {
    #[default]
    Stored,
    ReferenceOnly,
    Redacted,
}

impl ContentCapturePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::ReferenceOnly => "reference_only",
            Self::Redacted => "redacted",
        }
    }

    pub fn persists_content(self) -> bool {
        matches!(self, Self::Stored)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stored" => Some(Self::Stored),
            "reference_only" => Some(Self::ReferenceOnly),
            "redacted" => Some(Self::Redacted),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactSourceRef {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    #[default]
    Active,
    Dirty,
    RenderRequested,
    Rendered,
    Closed,
    Failed,
}

impl LifecycleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Dirty => "dirty",
            Self::RenderRequested => "render_requested",
            Self::Rendered => "rendered",
            Self::Closed => "closed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, ComponentValidationError> {
        match raw {
            "active" => Ok(Self::Active),
            "dirty" => Ok(Self::Dirty),
            "render_requested" => Ok(Self::RenderRequested),
            "rendered" => Ok(Self::Rendered),
            "closed" => Ok(Self::Closed),
            "failed" => Ok(Self::Failed),
            other => Err(ComponentValidationError::UnsupportedHostObject {
                path: "lifecycle.state".to_string(),
                detail: format!(
                    "non-canonical lifecycle state {other:?}; canonical states are \
                     active, dirty, render_requested, rendered, closed, failed"
                ),
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComponentLifecyclePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(default, alias = "cadence", skip_serializing_if = "Option::is_none")]
    pub cad: Option<i64>,
}

impl ComponentLifecyclePolicy {
    pub fn is_empty(&self) -> bool {
        self.ttl.is_none() && self.cad.is_none()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct InternalRef {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComponentInternalRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<InternalRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<InternalRef>,
}

impl ComponentInternalRefs {
    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.projection.is_none()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComponentLifecycle {
    #[serde(default)]
    pub state: LifecycleState,
    #[serde(default)]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "ComponentLifecyclePolicy::is_empty")]
    pub policy: ComponentLifecyclePolicy,
    #[serde(default, skip_serializing_if = "ComponentInternalRefs::is_empty")]
    pub internal_refs: ComponentInternalRefs,
}

impl ComponentLifecycle {
    pub fn is_empty(&self) -> bool {
        matches!(self.state, LifecycleState::Active)
            && self.revision == 0
            && self.policy.is_empty()
            && self.internal_refs.is_empty()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExternalRef {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TextPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<PactContentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<PactSourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_capture: Option<ContentCapturePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_status: Option<String>,
}

pub type ReasoningPayload = TextPayload;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolCallPayload {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_ref: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolResultPayload {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<PactContentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<PactSourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_capture: Option<ContentCapturePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_status: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaBody {
    InlineBytes {
        bytes: Vec<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encoding: Option<String>,
    },

    InlineText {
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encoding: Option<String>,
    },

    Url {
        url: String,
    },

    ContentRef(PactContentRef),

    ExternalRef(ExternalRef),
}

impl Default for MediaBody {
    fn default() -> Self {
        MediaBody::InlineText {
            data: String::new(),
            encoding: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaPayload {
    pub media_kind: MediaKind,
    pub mime_type: Option<String>,
    pub label: Option<String>,
    pub body: MediaBody,
    pub filename: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub page_count: Option<u32>,
    pub source_ref: Option<PactSourceRef>,
    pub content_capture: Option<ContentCapturePolicy>,
    pub content_status: Option<String>,

    pub aux: PactAttrs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    #[default]
    Image,
    Audio,
    Video,
    Document,
    Custom(String),
}

impl Serialize for MediaKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MediaKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(MediaKind::from_kind_str(Some(raw.as_str())))
    }
}

impl MediaKind {
    pub fn from_media_type(media_type: Option<&str>) -> Self {
        match media_type {
            Some("image_content") => Self::Image,
            Some("audio_content") => Self::Audio,
            Some("video_content") => Self::Video,
            Some("document_content") => Self::Document,
            Some(other) => Self::Custom(other.to_string()),
            None => Self::Image,
        }
    }

    pub fn from_kind_str(raw: Option<&str>) -> Self {
        match raw {
            Some("image") => Self::Image,
            Some("audio") => Self::Audio,
            Some("video") => Self::Video,
            Some("document") => Self::Document,
            Some(other) if !other.is_empty() => Self::Custom(other.to_string()),
            _ => Self::Image,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Document => "document",
            Self::Custom(name) => name.as_str(),
        }
    }
}

impl MediaPayload {
    pub fn media_kind(&self) -> MediaKind {
        self.media_kind.clone()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MessageTurnPayload {}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContainerPayload {
    pub component_type: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UnknownPayload {
    pub component_type: String,

    pub payload: PactAttrs,

    pub attrs: PactAttrs,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "componentType")]
pub enum ComponentKind {
    #[serde(rename = "root")]
    Root,
    #[serde(rename = "system_header")]
    SystemHeader,
    #[serde(rename = "message_turn")]
    MessageTurn(MessageTurnPayload),
    #[serde(rename = "container")]
    Container(ContainerPayload),
    #[serde(rename = "text_content")]
    Text(TextPayload),
    #[serde(rename = "reasoning_content")]
    Reasoning(ReasoningPayload),
    #[serde(rename = "tool_call")]
    ToolCall(ToolCallPayload),
    #[serde(rename = "tool_result")]
    ToolResult(ToolResultPayload),
    #[serde(rename = "media_content")]
    Media(MediaPayload),
    #[serde(rename = "unknown")]
    Unknown(UnknownPayload),
}

impl Default for ComponentKind {
    fn default() -> Self {
        ComponentKind::Unknown(UnknownPayload {
            component_type: "unknown".into(),
            payload: PactAttrs::new(),
            attrs: PactAttrs::new(),
        })
    }
}

impl ComponentKind {
    pub fn type_name(&self) -> &str {
        match self {
            Self::Root => "root",
            Self::SystemHeader => "system_header",
            Self::MessageTurn(_) => "message_turn",
            Self::Container(p) => &p.component_type,
            Self::Text(_) => "text_content",
            Self::Reasoning(_) => "reasoning_content",
            Self::ToolCall(_) => "tool_call",
            Self::ToolResult(_) => "tool_result",

            Self::Media(_) => "media_content",
            Self::Unknown(p) => &p.component_type,
        }
    }

    pub fn externalize_content(&mut self, content_ref: PactContentRef) {
        match self {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => {
                p.content = None;
                p.content_ref = Some(content_ref);
            }
            ComponentKind::ToolResult(p) => {
                p.content = None;
                p.content_ref = Some(content_ref);
            }
            ComponentKind::Media(p) => {
                p.body = MediaBody::ContentRef(content_ref);
            }
            ComponentKind::Unknown(p) => {
                if let Ok(ref_value) = serde_json::to_value(&content_ref) {
                    p.payload.remove("content");
                    p.payload.insert("content_ref".to_string(), ref_value);
                }
            }
            _ => {}
        }
    }

    pub fn has_content_ref(&self) -> bool {
        match self {
            ComponentKind::Unknown(p) => p.payload.contains_key("content_ref"),
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => p.content_ref.is_some(),
            ComponentKind::ToolResult(p) => p.content_ref.is_some(),
            ComponentKind::Media(p) => matches!(p.body, MediaBody::ContentRef(_)),
            _ => false,
        }
    }

    pub fn hydrate_content(&mut self, store: &crate::store::ContentStore) {
        let resolve = |content_ref: &PactContentRef| -> Option<String> {
            let record = store.records.get(&content_ref.id)?;
            record.storage.inline.clone()
        };
        match self {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => {
                if p.content.as_deref().is_none_or(str::is_empty)
                    && let Some(content_ref) = &p.content_ref
                    && let Some(body) = resolve(content_ref)
                {
                    p.content = Some(body);
                }
            }
            ComponentKind::ToolResult(p) => {
                if p.content.as_deref().is_none_or(str::is_empty)
                    && let Some(content_ref) = &p.content_ref
                    && let Some(body) = resolve(content_ref)
                {
                    p.content = Some(body);
                }
            }
            ComponentKind::Unknown(p) => {
                let hollow = p
                    .payload
                    .get("content")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty);
                if hollow
                    && let Some(ref_id) = p
                        .payload
                        .get("content_ref")
                        .and_then(|r| r.get("id"))
                        .and_then(Value::as_str)
                    && let Some(record) = store.records.get(ref_id)
                    && let Some(body) = record.storage.inline.clone()
                {
                    p.payload.insert("content".to_string(), Value::String(body));
                }
            }
            _ => {}
        }
    }

    fn project_into(&self, object: &mut Map<String, Value>) {
        fn put_opt(o: &mut Map<String, Value>, key: &str, v: &Option<String>) {
            if let Some(s) = v {
                o.insert(key.to_string(), Value::String(s.clone()));
            }
        }

        fn put_nz(o: &mut Map<String, Value>, key: &str, s: &str) {
            if !s.is_empty() {
                o.insert(key.to_string(), Value::String(s.to_string()));
            }
        }
        fn put_cref(o: &mut Map<String, Value>, v: &Option<PactContentRef>) {
            if let Some(r) = v {
                o.insert(
                    "content_ref".to_string(),
                    serde_json::to_value(r).unwrap_or(Value::Null),
                );
            }
        }
        fn put_sref(o: &mut Map<String, Value>, v: &Option<PactSourceRef>) {
            if let Some(r) = v {
                o.insert(
                    "source_ref".to_string(),
                    serde_json::to_value(r).unwrap_or(Value::Null),
                );
            }
        }
        fn put_cap(o: &mut Map<String, Value>, v: &Option<ContentCapturePolicy>) {
            if let Some(c) = v {
                o.insert(
                    "content_capture".to_string(),
                    Value::String(c.as_str().to_string()),
                );
            }
        }
        fn put_val(o: &mut Map<String, Value>, key: &str, v: &Option<Value>) {
            if let Some(x) = v {
                o.insert(key.to_string(), x.clone());
            }
        }
        match self {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => {
                put_opt(object, "content", &p.content);
                put_cref(object, &p.content_ref);
                put_sref(object, &p.source_ref);
                put_cap(object, &p.content_capture);
                put_opt(object, "content_status", &p.content_status);
            }
            ComponentKind::ToolCall(p) => {
                put_nz(object, "tool_call_id", &p.tool_call_id);
                put_nz(object, "tool_name", &p.tool_name);
                put_val(object, "parameters", &p.parameters);
                put_val(object, "parameters_ref", &p.parameters_ref);
            }
            ComponentKind::ToolResult(p) => {
                put_nz(object, "tool_call_id", &p.tool_call_id);
                put_opt(object, "tool_name", &p.tool_name);
                if let Some(s) = p.success {
                    object.insert("success".to_string(), Value::Bool(s));
                }
                put_opt(object, "content", &p.content);
                put_cref(object, &p.content_ref);
                put_val(object, "result", &p.result);
                put_val(object, "result_ref", &p.result_ref);
                put_opt(object, "error_message", &p.error_message);
                put_val(object, "error_ref", &p.error_ref);
                put_sref(object, &p.source_ref);
                put_cap(object, &p.content_capture);
                put_opt(object, "content_status", &p.content_status);
            }
            ComponentKind::Media(p) => {
                object.insert(
                    "media_kind".to_string(),
                    Value::String(p.media_kind.as_str().to_string()),
                );
                match &p.body {
                    MediaBody::InlineText { data, encoding } => {
                        if !data.is_empty() {
                            object.insert("content".to_string(), Value::String(data.clone()));
                        }
                        if let Some(enc) = encoding {
                            object.insert("encoding".to_string(), Value::String(enc.clone()));
                        }
                    }
                    MediaBody::InlineBytes { bytes, encoding } => {
                        object.insert(
                            "bytes".to_string(),
                            Value::Array(
                                bytes
                                    .iter()
                                    .map(|byte| Value::Number((*byte).into()))
                                    .collect(),
                            ),
                        );
                        if let Some(enc) = encoding {
                            object.insert("encoding".to_string(), Value::String(enc.clone()));
                        }
                    }
                    MediaBody::Url { url } => {
                        object.insert("url".to_string(), Value::String(url.clone()));
                    }
                    MediaBody::ContentRef(content_ref) => {
                        put_cref(object, &Some(content_ref.clone()));
                    }
                    MediaBody::ExternalRef(external_ref) => {
                        object.insert(
                            "external_ref".to_string(),
                            serde_json::to_value(external_ref).unwrap_or(Value::Null),
                        );
                    }
                }
                put_opt(object, "mime_type", &p.mime_type);
                put_opt(object, "label", &p.label);
                put_opt(object, "filename", &p.filename);
                if let Some(width) = p.width {
                    object.insert("width".to_string(), Value::Number(width.into()));
                }
                if let Some(height) = p.height {
                    object.insert("height".to_string(), Value::Number(height.into()));
                }
                if let Some(duration_ms) = p.duration_ms {
                    object.insert("duration_ms".to_string(), Value::Number(duration_ms.into()));
                }
                if let Some(page_count) = p.page_count {
                    object.insert("page_count".to_string(), Value::Number(page_count.into()));
                }
                put_sref(object, &p.source_ref);
                put_cap(object, &p.content_capture);
                put_opt(object, "content_status", &p.content_status);
                for (key, value) in p.aux.iter() {
                    object.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            ComponentKind::Unknown(payload) => {
                for (key, value) in payload.payload.iter() {
                    object.insert(key.clone(), value.clone());
                }
                if !payload.attrs.is_empty() {
                    object.insert(
                        "attrs".to_string(),
                        Value::Object(payload.attrs.clone().into_iter().collect()),
                    );
                }
            }
            ComponentKind::Root
            | ComponentKind::SystemHeader
            | ComponentKind::MessageTurn(_)
            | ComponentKind::Container(_) => {}
        }
    }
}

#[derive(Default)]
struct MediaParts {
    media_kind: Option<String>,
    mime_type: Option<String>,
    label: Option<String>,
    filename: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration_ms: Option<u64>,
    page_count: Option<u32>,
    url: Option<String>,
    bytes: Option<Vec<u8>>,
    encoding: Option<String>,
    external_ref: Option<ExternalRef>,

    aux: PactAttrs,
}

impl MediaParts {
    fn into_body(
        self,
        content_opt: Option<String>,
        content_ref: Option<PactContentRef>,
    ) -> MediaBody {
        if let Some(bytes) = self.bytes {
            MediaBody::InlineBytes {
                bytes,
                encoding: self.encoding,
            }
        } else if let Some(content_ref) = content_ref {
            MediaBody::ContentRef(content_ref)
        } else if let Some(external_ref) = self.external_ref {
            MediaBody::ExternalRef(external_ref)
        } else if let Some(url) = self.url {
            MediaBody::Url { url }
        } else {
            MediaBody::InlineText {
                data: content_opt.unwrap_or_default(),
                encoding: self.encoding,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn component_kind_from_parts(
    node_type: &str,
    content: String,
    content_ref: Option<PactContentRef>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    parameters: Option<Value>,
    parameters_ref: Option<Value>,
    success: Option<bool>,
    result: Option<Value>,
    result_ref: Option<Value>,
    error_message: Option<String>,
    error_ref: Option<Value>,
    source_ref: Option<PactSourceRef>,
    content_capture: Option<ContentCapturePolicy>,
    content_status: Option<String>,
    media: MediaParts,

    _attrs: &PactAttrs,
) -> ComponentKind {
    let content_opt = if content.is_empty() {
        None
    } else {
        Some(content)
    };
    match node_type {
        "root" => ComponentKind::Root,
        "system_header" => ComponentKind::SystemHeader,
        "message_turn" => ComponentKind::MessageTurn(MessageTurnPayload {}),
        "message_container" | "container" | "conversation_history" | "active_message" => {
            ComponentKind::Container(ContainerPayload {
                component_type: node_type.to_string(),
            })
        }
        "text_content" => ComponentKind::Text(TextPayload {
            content: content_opt,
            content_ref,
            source_ref,
            content_capture,
            content_status,
        }),
        "reasoning_content" => ComponentKind::Reasoning(TextPayload {
            content: content_opt,
            content_ref,
            source_ref,
            content_capture,
            content_status,
        }),
        "tool_call" => ComponentKind::ToolCall(ToolCallPayload {
            tool_call_id: tool_call_id.unwrap_or_default(),
            tool_name: tool_name.unwrap_or_default(),
            parameters,
            parameters_ref,
        }),
        "tool_result" => ComponentKind::ToolResult(ToolResultPayload {
            tool_call_id: tool_call_id.unwrap_or_default(),
            tool_name,
            success,
            content: content_opt,
            content_ref,
            result,
            result_ref,
            error_message,
            error_ref,
            source_ref,
            content_capture,
            content_status,
        }),

        "media_content" | "image_content" | "audio_content" | "video_content"
        | "document_content" => {
            let media_kind = if node_type == "media_content" {
                MediaKind::from_kind_str(media.media_kind.as_deref())
            } else {
                MediaKind::from_media_type(Some(node_type))
            };
            let mime_type = media.mime_type.clone();
            let label = media.label.clone();
            let filename = media.filename.clone();
            let width = media.width;
            let height = media.height;
            let duration_ms = media.duration_ms;
            let page_count = media.page_count;
            let aux = media.aux.clone();
            let body = media.into_body(content_opt, content_ref);
            ComponentKind::Media(MediaPayload {
                media_kind,
                mime_type,
                label,
                body,
                filename,
                width,
                height,
                duration_ms,
                page_count,
                source_ref,
                content_capture,
                content_status,
                aux,
            })
        }
        _ => {
            let mut unknown = PactAttrs::new();
            if let Some(value) = content_opt {
                unknown.insert("content".to_string(), Value::String(value));
            }
            if let Some(value) = content_ref {
                unknown.insert(
                    "content_ref".to_string(),
                    serde_json::to_value(&value).unwrap_or(Value::Null),
                );
            }
            if let Some(value) = tool_call_id {
                unknown.insert("tool_call_id".to_string(), Value::String(value));
            }
            if let Some(value) = tool_name {
                unknown.insert("tool_name".to_string(), Value::String(value));
            }
            if let Some(value) = parameters {
                unknown.insert("parameters".to_string(), value);
            }
            if let Some(value) = parameters_ref {
                unknown.insert("parameters_ref".to_string(), value);
            }
            if let Some(value) = success {
                unknown.insert("success".to_string(), Value::Bool(value));
            }
            if let Some(value) = result {
                unknown.insert("result".to_string(), value);
            }
            if let Some(value) = result_ref {
                unknown.insert("result_ref".to_string(), value);
            }
            if let Some(value) = error_message {
                unknown.insert("error_message".to_string(), Value::String(value));
            }
            if let Some(value) = error_ref {
                unknown.insert("error_ref".to_string(), value);
            }
            if let Some(value) = source_ref {
                unknown.insert(
                    "source_ref".to_string(),
                    serde_json::to_value(&value).unwrap_or(Value::Null),
                );
            }
            if let Some(value) = content_capture {
                unknown.insert(
                    "content_capture".to_string(),
                    Value::String(value.as_str().to_string()),
                );
            }
            if let Some(value) = content_status {
                unknown.insert("content_status".to_string(), Value::String(value));
            }
            ComponentKind::Unknown(UnknownPayload {
                component_type: node_type.to_string(),
                payload: unknown,

                attrs: PactAttrs::new(),
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub id: String,
    pub subtype: Option<String>,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub offset: i32,
    pub pact_depth: Option<i32>,
    pub priority: i32,
    pub transaction_index: u64,
    pub created_at_ns: u64,
    pub created_at_iso: Option<String>,
    pub creation_index: u64,
    pub key: Option<String>,
    pub tags: Vec<String>,

    pub aux: PactAttrs,

    pub kind: ComponentKind,
    pub provenance: Option<PactProvenance>,
    pub lifecycle: ComponentLifecycle,
    pub external_refs: Vec<ExternalRef>,
    pub children: Vec<Component>,
}

impl serde::Serialize for Component {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.project_component_value().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Component {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Component::from_component_at(&value, "$").map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthoringComponentOptions {
    pub local_id: Option<String>,
    pub created_at_ns: Option<u64>,
    pub ttl: Option<i64>,
    pub cad: Option<i64>,
    pub key: Option<String>,
    pub tags: Vec<String>,
    pub aux: PactAttrs,
    pub external_refs: Vec<ExternalRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentValidationError {
    ExpectedObject {
        path: String,
    },
    MissingId {
        path: String,
    },
    MissingComponentKind,
    MissingComponentType {
        path: String,
    },
    InvalidFieldType {
        path: String,
        expected: &'static str,
    },
    UnsupportedHostObject {
        path: String,
        detail: String,
    },
}

impl fmt::Display for ComponentValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedObject { path } => {
                write!(f, "PACT component at {path} must be an object")
            }
            Self::MissingId { path } => write!(f, "PACT component at {path} is missing string id"),
            Self::MissingComponentKind => write!(f, "PACT component kind cannot be empty"),
            Self::MissingComponentType { path } => {
                write!(
                    f,
                    "PACT component at {path} is missing string componentType/type"
                )
            }
            Self::InvalidFieldType { path, expected } => {
                write!(f, "PACT field {path} must be {expected}")
            }
            Self::UnsupportedHostObject { path, detail } => {
                write!(f, "unsupported PACT host object at {path}: {detail}")
            }
        }
    }
}

impl std::error::Error for ComponentValidationError {}

impl Component {
    pub(crate) fn clone_without_children(&self) -> Self {
        Self {
            id: self.id.clone(),
            subtype: self.subtype.clone(),
            parent_id: self.parent_id.clone(),
            role: self.role.clone(),
            offset: self.offset,
            pact_depth: self.pact_depth,
            priority: self.priority,
            transaction_index: self.transaction_index,
            created_at_ns: self.created_at_ns,
            created_at_iso: self.created_at_iso.clone(),
            creation_index: self.creation_index,
            key: self.key.clone(),
            tags: self.tags.clone(),
            aux: self.aux.clone(),
            kind: self.kind.clone(),
            provenance: self.provenance.clone(),
            lifecycle: self.lifecycle.clone(),
            external_refs: self.external_refs.clone(),
            children: Vec::new(),
        }
    }

    fn authoring(kind: ComponentKind, options: AuthoringComponentOptions) -> Self {
        let AuthoringComponentOptions {
            local_id,
            created_at_ns,
            ttl,
            cad,
            key,
            tags,
            aux,
            external_refs,
        } = options;
        Self {
            id: local_id.unwrap_or_else(mint_component_local_id),
            subtype: None,
            parent_id: None,
            role: None,
            offset: 0,
            pact_depth: None,
            priority: 0,
            transaction_index: 0,

            created_at_ns: created_at_ns.unwrap_or_else(now_created_at_ns),
            created_at_iso: None,
            creation_index: 0,
            key,
            tags,
            aux,
            kind,
            provenance: None,
            lifecycle: ComponentLifecycle {
                state: LifecycleState::Active,
                revision: 0,
                policy: ComponentLifecyclePolicy { ttl, cad },
                internal_refs: ComponentInternalRefs::default(),
            },
            external_refs,
            children: Vec::new(),
        }
    }

    pub fn text_authoring(content: String, options: AuthoringComponentOptions) -> Self {
        Self::authoring(
            ComponentKind::Text(TextPayload {
                content: Some(content),
                ..TextPayload::default()
            }),
            options,
        )
    }

    pub fn reasoning_authoring(content: String, options: AuthoringComponentOptions) -> Self {
        Self::authoring(
            ComponentKind::Reasoning(TextPayload {
                content: Some(content),
                ..TextPayload::default()
            }),
            options,
        )
    }

    pub fn media_authoring(payload: MediaPayload, options: AuthoringComponentOptions) -> Self {
        Self::authoring(ComponentKind::Media(payload), options)
    }

    pub fn tool_call_authoring(
        tool_call_id: String,
        tool_name: String,
        parameters: Option<Value>,
        options: AuthoringComponentOptions,
    ) -> Self {
        Self::authoring(
            ComponentKind::ToolCall(ToolCallPayload {
                tool_call_id,
                tool_name,
                parameters,
                parameters_ref: None,
            }),
            options,
        )
    }

    pub fn tool_result_authoring(
        tool_call_id: String,
        tool_name: Option<String>,
        success: Option<bool>,
        result: Option<Value>,
        error_message: Option<String>,
        content: Option<String>,
        options: AuthoringComponentOptions,
    ) -> Self {
        Self::authoring(
            ComponentKind::ToolResult(ToolResultPayload {
                tool_call_id,
                tool_name,
                success,
                content,
                content_ref: None,
                result,
                result_ref: None,
                error_message,
                error_ref: None,
                source_ref: None,
                content_capture: None,
                content_status: None,
            }),
            options,
        )
    }

    pub fn system_header_authoring(options: AuthoringComponentOptions) -> Self {
        Self::authoring(ComponentKind::SystemHeader, options)
    }

    pub fn message_turn_authoring(
        role: Option<String>,
        content: Option<Value>,
        mut options: AuthoringComponentOptions,
    ) -> Self {
        if let Some(content) = content {
            options.aux.insert("content".to_string(), content);
        }
        let mut component = Self::authoring(
            ComponentKind::MessageTurn(MessageTurnPayload::default()),
            options,
        );
        component.role = role;
        component
    }

    pub fn container_authoring(
        component_type: String,
        content: Option<Value>,
        mut options: AuthoringComponentOptions,
    ) -> Self {
        if let Some(content) = content {
            options.aux.insert("content".to_string(), content);
        }
        Self::authoring(
            ComponentKind::Container(ContainerPayload { component_type }),
            options,
        )
    }

    pub fn unknown_authoring(
        component_type: String,
        content: Option<String>,
        attrs: PactAttrs,
        options: AuthoringComponentOptions,
    ) -> Result<Self, ComponentValidationError> {
        ComponentType::parse(component_type.clone())?;
        let mut payload = PactAttrs::new();
        if let Some(content) = content {
            payload.insert("content".to_string(), Value::String(content));
        }

        Ok(Self::authoring(
            ComponentKind::Unknown(UnknownPayload {
                component_type,
                payload,
                attrs,
            }),
            options,
        ))
    }

    pub fn from_component_value(value: &Value) -> Result<Self, ComponentValidationError> {
        let mut node = Self::from_component_at(value, "$")?;
        node.sort_children_recursive();
        Ok(node)
    }

    pub fn project_component_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(self.id.clone()));

        object.insert(
            "componentType".to_string(),
            Value::String(self.kind.type_name().to_string()),
        );
        if let Some(subtype) = &self.subtype {
            object.insert("type".to_string(), Value::String(subtype.clone()));
        }
        object.insert(
            "parent_id".to_string(),
            self.parent_id
                .as_ref()
                .map(|value| Value::String(value.clone()))
                .unwrap_or(Value::Null),
        );
        if let Some(role) = &self.role {
            object.insert("role".to_string(), Value::String(role.clone()));
        }
        object.insert("offset".to_string(), Value::Number(self.offset.into()));
        if let Some(depth) = self.pact_depth {
            object.insert("pact_depth".to_string(), Value::Number(depth.into()));
        }
        object.insert("priority".to_string(), Value::Number(self.priority.into()));
        object.insert(
            "transaction_index".to_string(),
            Value::Number(self.transaction_index.into()),
        );
        object.insert(
            "created_at_ns".to_string(),
            Value::Number(self.created_at_ns.into()),
        );
        if let Some(created_at_iso) = &self.created_at_iso {
            object.insert(
                "created_at_iso".to_string(),
                Value::String(created_at_iso.clone()),
            );
        }
        object.insert(
            "creation_index".to_string(),
            Value::Number(self.creation_index.into()),
        );
        object.insert(
            "ttl".to_string(),
            self.lifecycle
                .policy
                .ttl
                .map(|value| Value::Number(value.into()))
                .unwrap_or(Value::Null),
        );
        if let Some(cad) = self.lifecycle.policy.cad {
            object.insert("cad".to_string(), Value::Number(cad.into()));
        }
        if let Some(key) = &self.key {
            object.insert("key".to_string(), Value::String(key.clone()));
        }
        if !self.tags.is_empty() {
            object.insert(
                "tags".to_string(),
                Value::Array(
                    self.tags
                        .iter()
                        .map(|tag| Value::String(tag.clone()))
                        .collect(),
                ),
            );
        }
        if !self.aux.is_empty() {
            object.insert(
                "aux".to_string(),
                Value::Object(self.aux.clone().into_iter().collect()),
            );
        }

        if !self.external_refs.is_empty() {
            object.insert(
                "external_refs".to_string(),
                serde_json::to_value(&self.external_refs)
                    .unwrap_or_else(|_| Value::Array(Vec::new())),
            );
        }

        if !matches!(self.lifecycle.state, LifecycleState::Active) || self.lifecycle.revision != 0 {
            let mut lifecycle = Map::new();
            lifecycle.insert(
                "state".to_string(),
                Value::String(self.lifecycle.state.as_str().to_string()),
            );
            lifecycle.insert(
                "revision".to_string(),
                Value::Number(self.lifecycle.revision.into()),
            );
            object.insert("lifecycle".to_string(), Value::Object(lifecycle));
        }
        if let Some(provenance) = &self.provenance {
            object.insert(
                "provenance".to_string(),
                serde_json::to_value(provenance).unwrap_or(Value::Null),
            );
        }

        self.kind.project_into(&mut object);
        object.insert(
            "child_ids".to_string(),
            Value::Array(
                self.children
                    .iter()
                    .map(|child| Value::String(child.id.clone()))
                    .collect(),
            ),
        );
        object.insert(
            "children".to_string(),
            Value::Array(
                self.children
                    .iter()
                    .map(Component::project_component_value)
                    .collect(),
            ),
        );
        Value::Object(object)
    }

    pub fn inspect_component_value(&self) -> Value {
        let mut value = self.project_component_value();
        if let Value::Object(object) = &mut value {
            object.insert(
                "lifecycle".to_string(),
                serde_json::to_value(&self.lifecycle).unwrap_or(Value::Null),
            );
            object.insert(
                "children".to_string(),
                Value::Array(
                    self.children
                        .iter()
                        .map(Component::inspect_component_value)
                        .collect(),
                ),
            );
        }
        value
    }

    fn from_component_at(value: &Value, path: &str) -> Result<Self, ComponentValidationError> {
        let object = value
            .as_object()
            .ok_or_else(|| ComponentValidationError::ExpectedObject {
                path: path.to_string(),
            })?;
        if object.contains_key("surface") {
            return Err(ComponentValidationError::InvalidFieldType {
                path: format!("{path}.surface"),
                expected: "not a PACT component field; use canonical components or namespaced aux metadata",
            });
        }

        let id = get_string(object, "id", path)?
            .map(str::to_string)
            .unwrap_or_default();

        let component_type = object
            .get("componentType")
            .and_then(Value::as_str)
            .or_else(|| object.get("type").and_then(Value::as_str))
            .ok_or_else(|| ComponentValidationError::MissingComponentType {
                path: path.to_string(),
            })?;
        if component_type.trim().is_empty() {
            return Err(ComponentValidationError::MissingComponentKind);
        }

        let children = match object.get("children") {
            Some(Value::Array(items)) => {
                let mut children = Vec::with_capacity(items.len());
                for (index, child) in items.iter().enumerate() {
                    children.push(Self::from_component_at(
                        child,
                        &format!("{path}.children[{index}]"),
                    )?);
                }
                children
            }
            Some(_) => {
                return Err(ComponentValidationError::InvalidFieldType {
                    path: format!("{path}.children"),
                    expected: "an array",
                });
            }
            None => Vec::new(),
        };

        let mut attrs = PactAttrs::new();
        let mut xml_attr_keys: BTreeSet<String> = BTreeSet::new();
        match object.get("attrs") {
            Some(Value::Object(values)) => {
                for (key, attr_value) in values {
                    validate_json_value(attr_value, &format!("{path}.attrs.{key}"))?;
                    xml_attr_keys.insert(key.clone());
                    attrs.insert(key.clone(), attr_value.clone());
                }
            }
            Some(_) => {
                return Err(ComponentValidationError::InvalidFieldType {
                    path: format!("{path}.attrs"),
                    expected: "an object",
                });
            }
            None => {}
        }
        for (key, attr_value) in object {
            if RESERVED_COMPONENT_FIELDS.contains(&key.as_str()) {
                continue;
            }
            validate_json_value(attr_value, &format!("{path}.{key}"))?;
            attrs.insert(key.clone(), attr_value.clone());
        }
        let tool_call_id = promoted_string_slot(object, &mut attrs, "tool_call_id");
        let tool_name = promoted_string_slot(object, &mut attrs, "tool_name");
        let parameters = promoted_value_slot(object, &mut attrs, "parameters");
        let parameters_ref = promoted_value_slot(object, &mut attrs, "parameters_ref");
        let success = promoted_bool_slot(object, &mut attrs, "success");
        let result = promoted_value_slot(object, &mut attrs, "result");
        let result_ref = promoted_value_slot(object, &mut attrs, "result_ref");
        let error_message = promoted_string_slot(object, &mut attrs, "error_message");
        let error_ref = promoted_value_slot(object, &mut attrs, "error_ref");
        let ttl = promoted_i64_slot(object, &mut attrs, "ttl");
        let cad = promoted_i64_slot_alias(object, &mut attrs, "cad", "cadence");
        validate_lifecycle_slots(path, ttl, cad)?;
        let key = promoted_string_slot(object, &mut attrs, "key");
        let tags = promoted_string_vec_slot_alias(object, &mut attrs, "tags", "tag")?;

        let mut aux = match object.get("aux") {
            Some(Value::Object(values)) => values.clone().into_iter().collect(),
            Some(_) => {
                return Err(ComponentValidationError::InvalidFieldType {
                    path: format!("{path}.aux"),
                    expected: "an object",
                });
            }
            None => PactAttrs::new(),
        };
        let content = match object.get("content") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(_) => {
                return Err(ComponentValidationError::InvalidFieldType {
                    path: format!("{path}.content"),
                    expected: "a string for leaf nodes or children for containers",
                });
            }
        };

        let content_ref = parse_content_ref(object, path)?;
        let provenance = parse_provenance(object, path)?;
        let content_capture = parse_content_capture(object, &mut attrs, path)?;
        let source_ref = parse_source_ref(object, &mut attrs, path)?;
        let content_status = parse_content_status(object, &mut attrs, path)?;

        let mut media = if is_media_component_type(component_type) {
            promote_media_parts(object, &mut attrs)
        } else {
            MediaParts::default()
        };
        if is_media_component_type(component_type) {
            media.aux = std::mem::take(&mut attrs);
        }

        let kind = component_kind_from_parts(
            component_type,
            content,
            content_ref,
            tool_call_id,
            tool_name,
            parameters,
            parameters_ref,
            success,
            result,
            result_ref,
            error_message,
            error_ref,
            source_ref,
            content_capture,
            content_status,
            media,
            &attrs,
        );

        let (lifecycle_state, lifecycle_revision) = parse_lifecycle_state_revision(object, path)?;

        let mut xml_attrs = PactAttrs::new();
        let mut extension = PactAttrs::new();
        for (key, value) in attrs {
            if xml_attr_keys.contains(&key) {
                xml_attrs.insert(key, value);
            } else {
                extension.insert(key, value);
            }
        }
        let kind = match kind {
            ComponentKind::Unknown(mut payload) => {
                payload.attrs = xml_attrs;
                ComponentKind::Unknown(payload)
            }
            other => {
                for (key, value) in xml_attrs {
                    aux.entry(key).or_insert(value);
                }
                other
            }
        };
        for (key, value) in extension {
            aux.entry(key).or_insert(value);
        }

        Ok(Self {
            id,
            subtype: get_string(object, "type", path)?.map(str::to_string),
            parent_id: get_string(object, "parent_id", path)?.map(str::to_string),
            role: get_string(object, "role", path)?.map(str::to_string),
            offset: get_i32(object, "offset", path)?.unwrap_or(0),
            pact_depth: get_i32(object, "pact_depth", path)?,
            priority: get_i32(object, "priority", path)?.unwrap_or(0),
            transaction_index: get_u64(object, "transaction_index", path)?.unwrap_or(0),
            created_at_ns: get_u64(object, "created_at_ns", path)?.unwrap_or(0),
            created_at_iso: get_string(object, "created_at_iso", path)?.map(str::to_string),
            creation_index: get_u64(object, "creation_index", path)?.unwrap_or(0),
            key,
            tags,
            aux,
            kind,
            provenance,
            children,
            lifecycle: ComponentLifecycle {
                state: lifecycle_state,
                revision: lifecycle_revision,
                policy: ComponentLifecyclePolicy { ttl, cad },
                internal_refs: ComponentInternalRefs::default(),
            },
            external_refs: parse_external_refs(object, path)?,
        })
    }

    pub fn kind(&self) -> &ComponentKind {
        &self.kind
    }

    pub fn component_type(&self) -> &str {
        self.kind.type_name()
    }

    pub fn content(&self) -> Option<&str> {
        match &self.kind {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => p.content.as_deref(),
            ComponentKind::ToolResult(p) => p.content.as_deref(),

            ComponentKind::Media(p) => match &p.body {
                MediaBody::InlineText { data, .. } if !data.is_empty() => Some(data.as_str()),
                _ => None,
            },

            ComponentKind::Unknown(payload) => {
                payload.payload.get("content").and_then(Value::as_str)
            }
            _ => None,
        }
    }

    pub fn xml_attrs(&self) -> Option<&PactAttrs> {
        match &self.kind {
            ComponentKind::Unknown(payload) => Some(&payload.attrs),
            _ => None,
        }
    }
    pub fn content_ref(&self) -> Option<&PactContentRef> {
        match &self.kind {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => p.content_ref.as_ref(),
            ComponentKind::ToolResult(p) => p.content_ref.as_ref(),
            ComponentKind::Media(p) => match &p.body {
                MediaBody::ContentRef(content_ref) => Some(content_ref),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn source_ref(&self) -> Option<&PactSourceRef> {
        match &self.kind {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => p.source_ref.as_ref(),
            ComponentKind::ToolResult(p) => p.source_ref.as_ref(),
            ComponentKind::Media(p) => p.source_ref.as_ref(),
            _ => None,
        }
    }
    pub fn content_capture(&self) -> Option<ContentCapturePolicy> {
        match &self.kind {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => p.content_capture,
            ComponentKind::ToolResult(p) => p.content_capture,
            ComponentKind::Media(p) => p.content_capture,
            _ => None,
        }
    }

    pub fn externalize_content(&mut self, content_ref: PactContentRef) {
        self.kind.externalize_content(content_ref);
    }

    pub fn has_content_ref(&self) -> bool {
        self.kind.has_content_ref()
    }

    pub fn hydrate_content(&mut self, store: &crate::store::ContentStore) {
        self.kind.hydrate_content(store);
        for child in &mut self.children {
            child.hydrate_content(store);
        }
    }
    pub fn content_status(&self) -> Option<&str> {
        match &self.kind {
            ComponentKind::Text(p) | ComponentKind::Reasoning(p) => p.content_status.as_deref(),
            ComponentKind::ToolResult(p) => p.content_status.as_deref(),
            ComponentKind::Media(p) => p.content_status.as_deref(),
            _ => None,
        }
    }
    pub fn tool_call_id(&self) -> Option<&str> {
        match &self.kind {
            ComponentKind::ToolCall(p) => Some(p.tool_call_id.as_str()),
            ComponentKind::ToolResult(p) => Some(p.tool_call_id.as_str()),
            _ => None,
        }
    }
    pub fn tool_name(&self) -> Option<&str> {
        match &self.kind {
            ComponentKind::ToolCall(p) => Some(p.tool_name.as_str()),
            ComponentKind::ToolResult(p) => p.tool_name.as_deref(),
            _ => None,
        }
    }
    pub fn parameters(&self) -> Option<&Value> {
        match &self.kind {
            ComponentKind::ToolCall(p) => p.parameters.as_ref(),
            _ => None,
        }
    }
    pub fn parameters_ref(&self) -> Option<&Value> {
        match &self.kind {
            ComponentKind::ToolCall(p) => p.parameters_ref.as_ref(),
            _ => None,
        }
    }
    pub fn success(&self) -> Option<bool> {
        match &self.kind {
            ComponentKind::ToolResult(p) => p.success,
            _ => None,
        }
    }
    pub fn result(&self) -> Option<&Value> {
        match &self.kind {
            ComponentKind::ToolResult(p) => p.result.as_ref(),
            _ => None,
        }
    }
    pub fn result_ref(&self) -> Option<&Value> {
        match &self.kind {
            ComponentKind::ToolResult(p) => p.result_ref.as_ref(),
            _ => None,
        }
    }
    pub fn error_message(&self) -> Option<&str> {
        match &self.kind {
            ComponentKind::ToolResult(p) => p.error_message.as_deref(),
            _ => None,
        }
    }
    pub fn error_ref(&self) -> Option<&Value> {
        match &self.kind {
            ComponentKind::ToolResult(p) => p.error_ref.as_ref(),
            _ => None,
        }
    }

    pub fn sort_children_recursive(&mut self) {
        if self.component_type() == "root" {
            self.children.sort_by(|left, right| {
                (
                    root_child_rank(left.component_type(), left.subtype.as_deref()),
                    left.offset,
                    left.priority,
                    left.created_at_ns,
                    left.creation_index,
                    left.id.as_str(),
                )
                    .cmp(&(
                        root_child_rank(right.component_type(), right.subtype.as_deref()),
                        right.offset,
                        right.priority,
                        right.created_at_ns,
                        right.creation_index,
                        right.id.as_str(),
                    ))
            });
        } else if is_conversation_history_region(self.component_type(), self.subtype.as_deref()) {
            self.children.sort_by(|left, right| {
                (
                    right.pact_depth.unwrap_or(0),
                    left.offset,
                    left.priority,
                    left.created_at_ns,
                    left.creation_index,
                    left.id.as_str(),
                )
                    .cmp(&(
                        left.pact_depth.unwrap_or(0),
                        right.offset,
                        right.priority,
                        right.created_at_ns,
                        right.creation_index,
                        right.id.as_str(),
                    ))
            });
        } else {
            self.children.sort_by(|left, right| {
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
        }
        for child in &mut self.children {
            child.sort_children_recursive();
        }
    }
}

fn root_child_rank(node_type: &str, kind: Option<&str>) -> i32 {
    if is_system_region(node_type, kind) {
        0
    } else if is_conversation_history_region(node_type, kind) {
        1
    } else if is_active_message_region(node_type, kind) {
        2
    } else {
        3
    }
}

fn is_system_region(node_type: &str, kind: Option<&str>) -> bool {
    node_type == "system_header" || kind == Some("system_header")
}

fn is_conversation_history_region(node_type: &str, kind: Option<&str>) -> bool {
    node_type == "conversation_history" || kind == Some("conversation_history")
}

fn is_active_message_region(node_type: &str, kind: Option<&str>) -> bool {
    node_type == "active_message"
        || kind == Some("active_message")
        || node_type == "active_provider_message"
        || kind == Some("active_provider_message")
}

pub fn component_from_json(component_json: &str) -> PactResult<String> {
    let value: Value = serde_json::from_str(component_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT component JSON: {err}")))?;
    let component = Component::from_component_value(&value)?;
    serde_json::to_string(&component)
        .map_err(|err| PactError::codec(format!("failed to encode native PACT component: {err}")))
}

pub fn pact_canonical_component_kinds_json() -> PactResult<String> {
    serde_json::to_string(CANONICAL_COMPONENT_KINDS).map_err(|err| {
        PactError::codec(format!(
            "failed to encode canonical PACT component kinds: {err}"
        ))
    })
}

pub fn pact_canonical_component_json(kind: &str, payload_json: &str) -> PactResult<String> {
    canonical_component_json(kind, payload_json)
}

fn canonical_component_json(kind: &str, payload_json: &str) -> PactResult<String> {
    let kind = CanonicalComponentKind::parse(kind)?;
    let payload: Value = serde_json::from_str(payload_json).map_err(|err| {
        PactError::invalid(format!(
            "invalid canonical PACT component payload JSON: {err}"
        ))
    })?;
    let mut object = payload
        .as_object()
        .cloned()
        .ok_or_else(|| PactError::invalid("canonical PACT component payload must be an object"))?;

    let node_type = match kind.node_type() {
        Some(node_type) => node_type.to_string(),
        None => object
            .get("componentType")
            .and_then(Value::as_str)
            .or_else(|| object.get("componentType").and_then(Value::as_str))
            .or_else(|| object.get("type").and_then(Value::as_str))
            .ok_or_else(|| {
                PactError::invalid(
                    "PACT component escape hatch requires a concrete componentType/type in the payload",
                )
            })?
            .to_string(),
    };

    ComponentType::parse(node_type.clone())?;
    object.insert(
        "componentType".to_string(),
        Value::String(node_type.clone()),
    );
    object
        .entry("type".to_string())
        .or_insert_with(|| Value::String(node_type));

    validate_canonical_node_payload(kind, &object)?;
    let node = Component::from_component_value(&Value::Object(object))?;
    serde_json::to_string(&node).map_err(|err| {
        PactError::codec(format!("failed to encode canonical PACT component: {err}"))
    })
}

pub fn component_project_json(component_json: &str) -> PactResult<String> {
    let component: Component = serde_json::from_str(component_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT component JSON: {err}")))?;
    serde_json::to_string(&component.project_component_value()).map_err(|err| {
        PactError::codec(format!("failed to encode projected PACT component: {err}"))
    })
}

pub fn component_prepare_for_context_json(
    component_json: &str,
    context_json: &str,
) -> PactResult<String> {
    let value: Value = serde_json::from_str(component_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT component JSON: {err}")))?;
    let mut node = Component::from_component_value(&value)?;
    let context: Value = serde_json::from_str(context_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT node context JSON: {err}")))?;
    let context = context
        .as_object()
        .ok_or_else(|| PactError::invalid("PACT node context payload must be an object"))?;

    let mut next_creation_index = context
        .get("next_creation_index")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut assigned_ids = Vec::new();
    apply_context_to_node(
        &mut node,
        context,
        &mut next_creation_index,
        &mut assigned_ids,
    );

    serde_json::to_string(&serde_json::json!({
        "node": node,
        "next_creation_index": next_creation_index,
        "assigned_ids": assigned_ids,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode prepared PACT component: {err}")))
}

const RESERVED_COMPONENT_FIELDS: &[&str] = &[
    "id",
    "componentType",
    "node_type",
    "type",
    "parent_id",
    "role",
    "offset",
    "pact_depth",
    "priority",
    "transaction_index",
    "created_at_ns",
    "created_at_iso",
    "creation_index",
    "ttl",
    "cad",
    "cadence",
    "key",
    "tags",
    "tag",
    "attrs",
    "aux",
    "content",
    "content_ref",
    "tool_call_id",
    "tool_name",
    "parameters",
    "parameters_ref",
    "success",
    "result",
    "result_ref",
    "error_message",
    "error_ref",
    "provenance",
    "content_capture",
    "source_ref",
    "content_status",
    "external_refs",
    "lifecycle",
    "owner",
    "context_id",
    "children",
    "child_ids",
];

fn parse_lifecycle_state_revision(
    object: &Map<String, Value>,
    path: &str,
) -> Result<(LifecycleState, u64), ComponentValidationError> {
    match object.get("lifecycle") {
        None | Some(Value::Null) => Ok((LifecycleState::Active, 0)),
        Some(Value::Object(lifecycle)) => {
            let state = match lifecycle.get("state").and_then(Value::as_str) {
                Some(raw) => LifecycleState::parse(raw)?,
                None => LifecycleState::Active,
            };
            let revision = lifecycle
                .get("revision")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Ok((state, revision))
        }
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.lifecycle"),
            expected: "an object with state/revision",
        }),
    }
}

fn parse_external_refs(
    object: &Map<String, Value>,
    path: &str,
) -> Result<Vec<ExternalRef>, ComponentValidationError> {
    match object.get("external_refs") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            serde_json::from_value::<Vec<ExternalRef>>(Value::Array(items.clone())).map_err(|_| {
                ComponentValidationError::InvalidFieldType {
                    path: format!("{path}.external_refs"),
                    expected: "an array of external refs ({kind, uri?, role?, data?})",
                }
            })
        }
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.external_refs"),
            expected: "an array",
        }),
    }
}

fn promoted_value_slot(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    key: &str,
) -> Option<Value> {
    let attr_value = attrs.remove(key);
    object.get(key).cloned().or(attr_value)
}

fn promoted_string_slot(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    key: &str,
) -> Option<String> {
    let attr_value = attrs.remove(key);
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| attr_value.and_then(|value| value.as_str().map(str::to_string)))
}

fn promoted_bool_slot(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    key: &str,
) -> Option<bool> {
    let attr_value = attrs.remove(key);
    object
        .get(key)
        .and_then(Value::as_bool)
        .or_else(|| attr_value.and_then(|value| value.as_bool()))
}

fn promoted_i64_slot(object: &Map<String, Value>, attrs: &mut PactAttrs, key: &str) -> Option<i64> {
    let attr_value = attrs.remove(key);
    object
        .get(key)
        .and_then(Value::as_i64)
        .or_else(|| attr_value.and_then(|value| value.as_i64()))
}

fn promoted_u64_slot(object: &Map<String, Value>, attrs: &mut PactAttrs, key: &str) -> Option<u64> {
    let attr_value = attrs.remove(key);
    object
        .get(key)
        .and_then(Value::as_u64)
        .or_else(|| attr_value.and_then(|value| value.as_u64()))
}

fn promoted_u32_slot(object: &Map<String, Value>, attrs: &mut PactAttrs, key: &str) -> Option<u32> {
    promoted_u64_slot(object, attrs, key).and_then(|value| u32::try_from(value).ok())
}

fn promote_media_parts(object: &Map<String, Value>, attrs: &mut PactAttrs) -> MediaParts {
    let media_kind = promoted_string_slot(object, attrs, "media_kind");
    let mime_type = promoted_string_slot(object, attrs, "mime_type");
    let label = promoted_string_slot(object, attrs, "label");
    let filename = promoted_string_slot(object, attrs, "filename");
    let width = promoted_u32_slot(object, attrs, "width");
    let height = promoted_u32_slot(object, attrs, "height");
    let duration_ms = promoted_u64_slot(object, attrs, "duration_ms");
    let page_count = promoted_u32_slot(object, attrs, "page_count");
    let url = promoted_string_slot(object, attrs, "url");
    let encoding = promoted_string_slot(object, attrs, "encoding");
    let bytes = promoted_value_slot(object, attrs, "bytes").and_then(|value| match value {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let byte = item.as_u64().and_then(|n| u8::try_from(n).ok())?;
                out.push(byte);
            }
            Some(out)
        }
        _ => None,
    });
    let external_ref = promoted_value_slot(object, attrs, "external_ref")
        .and_then(|value| serde_json::from_value::<ExternalRef>(value).ok());
    MediaParts {
        media_kind,
        mime_type,
        label,
        filename,
        width,
        height,
        duration_ms,
        page_count,
        url,
        bytes,
        encoding,
        external_ref,
        aux: PactAttrs::new(),
    }
}

fn is_media_component_type(component_type: &str) -> bool {
    matches!(
        component_type,
        "media_content" | "image_content" | "audio_content" | "video_content" | "document_content"
    )
}

fn promoted_i64_slot_alias(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    primary: &str,
    alias: &str,
) -> Option<i64> {
    promoted_i64_slot(object, attrs, primary).or_else(|| promoted_i64_slot(object, attrs, alias))
}

fn validate_lifecycle_slots(
    path: &str,
    ttl: Option<i64>,
    cad: Option<i64>,
) -> Result<(), ComponentValidationError> {
    if ttl.is_some_and(|value| value < 0) {
        return Err(ComponentValidationError::UnsupportedHostObject {
            path: format!("{path}.ttl"),
            detail: "ttl must be null, zero, or a positive cycle count".to_string(),
        });
    }
    if cad.is_some_and(|value| value <= 0) {
        return Err(ComponentValidationError::UnsupportedHostObject {
            path: format!("{path}.cad"),
            detail: "cad must be a positive cycle count".to_string(),
        });
    }
    if cad.is_some() && ttl.is_none() {
        return Err(ComponentValidationError::UnsupportedHostObject {
            path: format!("{path}.cad"),
            detail: "cad without ttl is invalid; cadence needs a ttl window".to_string(),
        });
    }
    Ok(())
}

fn promoted_string_vec_slot_alias(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    primary: &str,
    alias: &str,
) -> Result<Vec<String>, ComponentValidationError> {
    let primary_attr = attrs.remove(primary);
    let alias_attr = attrs.remove(alias);
    let primary_value = object.get(primary).cloned().or(primary_attr);
    let alias_value = object.get(alias).cloned().or(alias_attr);
    let Some(value) = primary_value.or(alias_value) else {
        return Ok(Vec::new());
    };
    string_vec_from_value(value, primary)
}

fn string_vec_from_value(value: Value, key: &str) -> Result<Vec<String>, ComponentValidationError> {
    match value {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                match item {
                    Value::String(tag) if !tag.is_empty() => out.push(tag),
                    _ => {
                        return Err(ComponentValidationError::InvalidFieldType {
                            path: format!("$.{key}[{index}]"),
                            expected: "a non-empty string",
                        });
                    }
                }
            }
            Ok(out)
        }
        Value::String(tag) if !tag.is_empty() => Ok(vec![tag]),
        Value::Null => Ok(Vec::new()),
        _ => Err(ComponentValidationError::InvalidFieldType {
            path: format!("$.{key}"),
            expected: "a string or array of strings",
        }),
    }
}

fn apply_context_to_node(
    node: &mut Component,
    context: &Map<String, Value>,
    next_creation_index: &mut u64,
    assigned_ids: &mut Vec<String>,
) {
    if node.creation_index == 0 {
        node.creation_index = *next_creation_index;
        *next_creation_index += 1;
        assigned_ids.push(node.id.clone());
    }
    if node.created_at_ns == 0
        && let Some(created_at_ns) = context.get("created_at_ns").and_then(Value::as_u64)
    {
        node.created_at_ns = created_at_ns;
    }
    if node.created_at_iso.is_none()
        && let Some(created_at_iso) = context.get("created_at_iso").and_then(Value::as_str)
    {
        node.created_at_iso = Some(created_at_iso.to_string());
    }
    if node.transaction_index == 0
        && let Some(transaction_index) = context.get("transaction_index").and_then(Value::as_u64)
    {
        node.transaction_index = transaction_index;
    }
    if let Some(born_turn_index) = context.get("born_turn_index") {
        node.aux
            .entry("born_turn_index".to_string())
            .or_insert_with(|| born_turn_index.clone());
    }
    merge_context_provenance(node, context);
    for child in &mut node.children {
        apply_context_to_node(child, context, next_creation_index, assigned_ids);
    }
}

fn merge_context_provenance(node: &mut Component, context: &Map<String, Value>) {
    let mut provenance = node.provenance.clone().unwrap_or_default();
    if provenance.transaction_id.is_none() {
        provenance.transaction_id = context
            .get("transaction_id")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if provenance.trace_id.is_none() {
        provenance.trace_id = context
            .get("trace_id")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if provenance.branch_id.is_none() {
        provenance.branch_id = context
            .get("branch_id")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if provenance.source.is_none() {
        provenance.source = context
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if provenance.transaction_id.is_some()
        || provenance.trace_id.is_some()
        || provenance.branch_id.is_some()
        || provenance.source.is_some()
    {
        node.provenance = Some(provenance);
    }
}

fn validate_canonical_node_payload(
    kind: CanonicalComponentKind,
    object: &Map<String, Value>,
) -> Result<(), ComponentValidationError> {
    reject_inline_content_with_ref(object, "$")?;
    enforce_payload_slot_policy(kind, object)?;
    match kind {
        CanonicalComponentKind::Component => Ok(()),
        CanonicalComponentKind::PactRoot
        | CanonicalComponentKind::SystemHeader
        | CanonicalComponentKind::ConversationHistory
        | CanonicalComponentKind::ActiveMessage
        | CanonicalComponentKind::MessageTurn
        | CanonicalComponentKind::MessageContainer => Ok(()),
        CanonicalComponentKind::TextContent | CanonicalComponentKind::ReasoningContent => {
            if object.get("content").is_none() && object.get("content_ref").is_none() {
                return Err(ComponentValidationError::InvalidFieldType {
                    path: "$.content".to_string(),
                    expected: "text content or content_ref",
                });
            }
            Ok(())
        }
        CanonicalComponentKind::ToolCall => {
            require_string_slot(object, "tool_call_id")?;
            require_string_slot(object, "tool_name")?;
            require_object_slot(object, "parameters")?;
            Ok(())
        }
        CanonicalComponentKind::ToolResult => {
            require_string_slot(object, "tool_call_id")?;
            require_string_slot(object, "tool_name")?;
            require_bool_slot(object, "success")?;
            if object.get("content").is_none()
                && object.get("content_ref").is_none()
                && slot_value(object, "result").is_none()
                && slot_value(object, "result_ref").is_none()
                && slot_value(object, "error_message").is_none()
            {
                return Err(ComponentValidationError::InvalidFieldType {
                    path: "$.content".to_string(),
                    expected: "tool result content, result, error_message, or content_ref",
                });
            }
            Ok(())
        }
        CanonicalComponentKind::ImageContent
        | CanonicalComponentKind::AudioContent
        | CanonicalComponentKind::VideoContent
        | CanonicalComponentKind::DocumentContent => {
            if object.get("content_ref").is_some()
                || slot_value(object, "media_content").is_some()
                || slot_value(object, "media_url").is_some()
                || slot_value(object, "url").is_some()
                || slot_value(object, "media_data").is_some()
                || slot_value(object, "data").is_some()
            {
                Ok(())
            } else {
                Err(ComponentValidationError::InvalidFieldType {
                    path: "$.content_ref".to_string(),
                    expected: "media content_ref, media_content, url, or data",
                })
            }
        }
    }
}

fn enforce_payload_slot_policy(
    kind: CanonicalComponentKind,
    object: &Map<String, Value>,
) -> Result<(), ComponentValidationError> {
    enforce_large_extension_map("$", object.get("attrs"))?;
    enforce_large_extension_map("$", object.get("aux"))?;
    enforce_large_extension_map("$", object.get("aux"))?;

    if kind == CanonicalComponentKind::ToolResult {
        enforce_ref_for_large_slot(
            "$.content",
            object.get("content"),
            object.get("content_ref"),
            true,
        )?;
        enforce_ref_for_large_slot(
            "$.result",
            slot_value(object, "result"),
            slot_value(object, "result_ref").or_else(|| object.get("content_ref")),
            true,
        )?;
    }
    Ok(())
}

fn enforce_large_extension_map(
    path: &str,
    value: Option<&Value>,
) -> Result<(), ComponentValidationError> {
    let Some(Value::Object(object)) = value else {
        return Ok(());
    };
    let inline_fields = inline_payload_policy_fields(object);
    for (key, child) in object {
        if inline_fields.contains(key) {
            continue;
        }
        let child_path = format!("{path}.{key}");
        if extension_value_is_refish(child) {
            validate_refish_source(&child_path, child)?;
            continue;
        }
        enforce_ref_for_large_slot(&child_path, Some(child), None, false)?;
    }
    Ok(())
}

fn inline_payload_policy_fields(object: &Map<String, Value>) -> std::collections::BTreeSet<String> {
    let mut fields = std::collections::BTreeSet::new();
    for policies_key in ["payload_slot_policies", "payload_slots"] {
        let Some(Value::Object(policies)) = object.get(policies_key) else {
            continue;
        };
        for (field, policy) in policies {
            if payload_policy_declares_inline(policy) {
                fields.insert(canonical_payload_policy_field(field));
            }
        }
    }
    fields
}

fn payload_policy_declares_inline(policy: &Value) -> bool {
    let Some(object) = policy.as_object() else {
        return policy.as_str() == Some("inline");
    };
    ["strategy", "mode", "overflow", "storage", "externalization"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .any(|value| value == "inline" || value == "inline_required")
}

fn canonical_payload_policy_field(field: &str) -> String {
    field
        .trim_start_matches("$.")
        .rsplit('.')
        .next()
        .unwrap_or(field)
        .to_string()
}

fn enforce_ref_for_large_slot(
    path: &str,
    value: Option<&Value>,
    ref_value: Option<&Value>,
    source_required: bool,
) -> Result<(), ComponentValidationError> {
    let Some(value) = value else {
        return Ok(());
    };
    let bytes = payload_value_bytes(value);
    if bytes <= DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES {
        return Ok(());
    }

    let ref_value = match ref_value {
        Some(ref_value) if value_is_refish(ref_value) => {
            validate_refish_source(path, ref_value)?;
            ref_value
        }
        _ => {
            return Err(ComponentValidationError::InvalidFieldType {
                path: path.to_string(),
                expected: "a declared content_ref/external_ref for payloads larger than the inline limit",
            });
        }
    };
    if source_required && !refish_has_source(ref_value) {
        return Err(ComponentValidationError::InvalidFieldType {
            path: path.to_string(),
            expected: "a content_ref/external_ref with source metadata",
        });
    }
    Ok(())
}

fn payload_value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        _ => serde_json::to_vec(value).map_or(0, |bytes| bytes.len()),
    }
}

fn value_is_refish(value: &Value) -> bool {
    if value.as_str().is_some() {
        return true;
    }
    value.as_object().is_some_and(|object| {
        object.contains_key("content_ref")
            || object.contains_key("external_ref")
            || object.contains_key("id")
    })
}

fn extension_value_is_refish(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.contains_key("content_ref")
        || object.contains_key("external_ref")
        || object.get("id").and_then(Value::as_str).is_some_and(|id| {
            (id.starts_with("cnt_") || id.starts_with("blob_") || id.starts_with("ref_"))
                && (object.contains_key("source")
                    || object.contains_key("field")
                    || object.contains_key("preview")
                    || object.contains_key("preview_inline")
                    || object.contains_key("chars"))
        })
}

fn validate_refish_source(path: &str, value: &Value) -> Result<(), ComponentValidationError> {
    if let Some(object) = value.as_object() {
        if let Some(content_ref) = object.get("content_ref") {
            return validate_refish_source(path, content_ref);
        }
        if let Some(external_ref) = object.get("external_ref") {
            return validate_refish_source(path, external_ref);
        }
        if let Some(source) = object.get("source")
            && !source.is_object()
        {
            return Err(ComponentValidationError::InvalidFieldType {
                path: format!("{path}.source"),
                expected: "an object",
            });
        }
    }
    Ok(())
}

fn refish_has_source(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| {
            object
                .get("source")
                .or_else(|| {
                    object
                        .get("content_ref")
                        .and_then(|value| value.get("source"))
                })
                .or_else(|| {
                    object
                        .get("external_ref")
                        .and_then(|value| value.get("source"))
                })
        })
        .is_some_and(Value::is_object)
}

fn reject_inline_content_with_ref(
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), ComponentValidationError> {
    if object
        .get("content_ref")
        .is_some_and(|value| !value.is_null())
        && let Some(Value::String(content)) = object.get("content")
        && !content.is_empty()
    {
        return Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.content"),
            expected: "empty or omitted when content_ref is present; put previews on content_ref.preview",
        });
    }
    Ok(())
}

fn slot_value<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object
        .get(key)
        .or_else(|| object.get("attrs").and_then(|attrs| attrs.get(key)))
}

fn require_string_slot(
    object: &Map<String, Value>,
    key: &str,
) -> Result<(), ComponentValidationError> {
    match slot_value(object, key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(()),
        _ => Err(ComponentValidationError::InvalidFieldType {
            path: format!("$.{key}"),
            expected: "a non-empty string",
        }),
    }
}

fn require_object_slot(
    object: &Map<String, Value>,
    key: &str,
) -> Result<(), ComponentValidationError> {
    match slot_value(object, key) {
        Some(Value::Object(_)) => Ok(()),
        _ => Err(ComponentValidationError::InvalidFieldType {
            path: format!("$.{key}"),
            expected: "an object",
        }),
    }
}

fn require_bool_slot(
    object: &Map<String, Value>,
    key: &str,
) -> Result<(), ComponentValidationError> {
    match slot_value(object, key) {
        Some(Value::Bool(_)) => Ok(()),
        _ => Err(ComponentValidationError::InvalidFieldType {
            path: format!("$.{key}"),
            expected: "a boolean",
        }),
    }
}

fn get_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<&'a str>, ComponentValidationError> {
    match object.get(key) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.{key}"),
            expected: "a string",
        }),
    }
}

fn get_i32(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<i32>, ComponentValidationError> {
    match object.get(key) {
        Some(Value::Number(value)) => value
            .as_i64()
            .and_then(|number| i32::try_from(number).ok())
            .map(Some)
            .ok_or_else(|| ComponentValidationError::InvalidFieldType {
                path: format!("{path}.{key}"),
                expected: "a 32-bit integer",
            }),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.{key}"),
            expected: "an integer",
        }),
    }
}

fn get_u64(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<u64>, ComponentValidationError> {
    match object.get(key) {
        Some(Value::Number(value)) => {
            value
                .as_u64()
                .map(Some)
                .ok_or_else(|| ComponentValidationError::InvalidFieldType {
                    path: format!("{path}.{key}"),
                    expected: "an unsigned integer",
                })
        }
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.{key}"),
            expected: "an unsigned integer",
        }),
    }
}

fn parse_content_ref(
    object: &Map<String, Value>,
    path: &str,
) -> Result<Option<PactContentRef>, ComponentValidationError> {
    let Some(value) = object.get("content_ref") else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(id) => {
            validate_content_ref_id(id, path)?;
            Ok(Some(PactContentRef {
                id: id.clone(),
                field: Some("content".to_string()),
                chars: get_u64(object, "content_chars", path)?,
                preview: get_string(object, "content_preview", path)?.map(str::to_string),
                source: None,
            }))
        }
        Value::Object(_) => {
            let content_ref: PactContentRef =
                serde_json::from_value(value.clone()).map_err(|err| {
                    ComponentValidationError::UnsupportedHostObject {
                        path: format!("{path}.content_ref"),
                        detail: err.to_string(),
                    }
                })?;
            validate_content_ref_id(&content_ref.id, path)?;
            Ok(Some(content_ref))
        }
        _ => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.content_ref"),
            expected: "a string id or object",
        }),
    }
}

fn validate_content_ref_id(id: &str, path: &str) -> Result<(), ComponentValidationError> {
    if !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
    {
        Ok(())
    } else {
        Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.content_ref.id"),
            expected: "a non-empty storage ref id, not inline content",
        })
    }
}
fn parse_content_capture(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    path: &str,
) -> Result<Option<ContentCapturePolicy>, ComponentValidationError> {
    let attr_value = attrs.remove("content_capture");
    let raw = object.get("content_capture").cloned().or(attr_value);
    match raw {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(text)) => ContentCapturePolicy::parse(text.trim())
            .map(Some)
            .ok_or_else(|| ComponentValidationError::UnsupportedHostObject {
                path: format!("{path}.content_capture"),
                detail: format!(
                    "invalid content_capture {:?}; expected stored, reference_only, or redacted",
                    text.trim()
                ),
            }),
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.content_capture"),
            expected: "a string (stored, reference_only, or redacted)",
        }),
    }
}

fn parse_source_ref(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    path: &str,
) -> Result<Option<PactSourceRef>, ComponentValidationError> {
    let attr_value = attrs.remove("source_ref");
    let raw = object.get("source_ref").cloned().or(attr_value);
    match raw {
        Some(Value::Null) | None => Ok(None),
        Some(value @ Value::Object(_)) => {
            let parsed: PactSourceRef = serde_json::from_value(value).map_err(|err| {
                ComponentValidationError::UnsupportedHostObject {
                    path: format!("{path}.source_ref"),
                    detail: err.to_string(),
                }
            })?;
            validate_source_ref(&parsed, path)?;
            Ok(Some(parsed))
        }
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.source_ref"),
            expected: "an object with at least a kind field",
        }),
    }
}

fn parse_content_status(
    object: &Map<String, Value>,
    attrs: &mut PactAttrs,
    path: &str,
) -> Result<Option<String>, ComponentValidationError> {
    let attr_value = attrs.remove("content_status");
    let raw = object.get("content_status").cloned().or(attr_value);
    match raw {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.content_status"),
            expected: "a string",
        }),
    }
}

fn validate_source_ref(
    source_ref: &PactSourceRef,
    path: &str,
) -> Result<(), ComponentValidationError> {
    if source_ref.kind.trim().is_empty() {
        return Err(ComponentValidationError::UnsupportedHostObject {
            path: format!("{path}.source_ref.kind"),
            detail: "source_ref.kind is required".to_string(),
        });
    }
    if source_ref.kind == "file"
        && source_ref
            .path
            .as_deref()
            .map(str::is_empty)
            .unwrap_or(true)
    {
        return Err(ComponentValidationError::UnsupportedHostObject {
            path: format!("{path}.source_ref.path"),
            detail: "source_ref.path is required when kind=\"file\"".to_string(),
        });
    }
    if let Some([start, end]) = source_ref.byte_range
        && start > end
    {
        return Err(ComponentValidationError::UnsupportedHostObject {
            path: format!("{path}.source_ref.byte_range"),
            detail: format!("byte_range start {start} must be <= end {end}"),
        });
    }
    Ok(())
}

fn parse_provenance(
    object: &Map<String, Value>,
    path: &str,
) -> Result<Option<PactProvenance>, ComponentValidationError> {
    match object.get("provenance") {
        Some(Value::Object(_)) => serde_json::from_value(object["provenance"].clone())
            .map(Some)
            .map_err(|err| ComponentValidationError::UnsupportedHostObject {
                path: format!("{path}.provenance"),
                detail: err.to_string(),
            }),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(ComponentValidationError::InvalidFieldType {
            path: format!("{path}.provenance"),
            expected: "an object",
        }),
    }
}

fn validate_json_value(value: &Value, path: &str) -> Result<(), ComponentValidationError> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_json_value(item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (key, item) in values {
                validate_json_value(item, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
