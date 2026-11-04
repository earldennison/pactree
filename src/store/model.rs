use crate::components::{
    Component, ComponentKind, ComponentLifecycle, ComponentLifecyclePolicy, MediaBody,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const STORE_SCHEMA: &str = "pact.store.v1";
pub const ROOT_STORE_SCHEMA: &str = "pact.root_store.v1";
pub const CHECKPOINT_SCHEMA: &str = "pact.checkpoint.v1";
pub const TRANSACTION_TRACE_SCHEMA: &str = "pact.transaction_trace.v1";
pub const TRANSACTION_TRACE_BUILD_REQUEST_SCHEMA: &str = "pact.transaction_trace_build_request.v1";

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactRef {
    pub root_id: String,
    pub branch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactParentProvenance {
    pub parent_ref: PactRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locator_hints: Vec<PactParentLocator>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactParentLocator {
    pub kind: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PactBaseMode {
    Pinned,
    Following,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactBaseProvenance {
    pub base_ref: PactRef,
    pub mode: PactBaseMode,
    pub created_from_head: PactRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locator_hints: Vec<PactParentLocator>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PactBoundary {
    Authored,
    #[serde(rename = "Preflight", alias = "pre_request")]
    PreRequest,
    PostRequestAlteration,
    ProviderRequest,
    ProviderResponse,
    PostResponseAlteration,
    #[serde(rename = "PostResponse", alias = "post_response")]
    PostResponse,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactViewMode {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<PactBoundary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactViewRow {
    pub node_ref: PactRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactViewRows {
    pub root_id: String,
    pub branch_id: String,
    pub revision: u64,
    pub mode: PactViewMode,
    #[serde(default)]
    pub rows: Vec<PactViewRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alteration_log_ref: Option<String>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactAlterationRef {
    pub id: String,
    pub from_boundary: PactBoundary,
    pub to_boundary: PactBoundary,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactBoundaryView {
    pub boundary: PactBoundary,
    pub root_id: String,
    pub branch_id: String,
    pub transaction_id: String,
    pub revision: u64,
    #[serde(default)]
    pub rows: Vec<PactViewRow>,
    #[serde(default)]
    pub alterations: Vec<PactAlterationRef>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AlterationSource {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactAlteration {
    pub id: String,
    pub from_boundary: PactBoundary,
    pub to_boundary: PactBoundary,
    pub source: AlterationSource,
    pub before_ref: PactRef,
    pub after_ref: PactRef,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContentStorageRef {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<crate::components::ExternalRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContentRecord {
    pub id: String,
    pub kind: String,
    pub hash: String,
    pub bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<usize>,
    pub preview: String,
    pub storage: ContentStorageRef,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContentRecordSummary {
    pub id: String,
    pub kind: String,
    pub hash: String,
    pub bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<usize>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preview: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContentStore {
    #[serde(default)]
    pub records: BTreeMap<String, ContentRecord>,
    #[serde(default, skip_serializing)]
    pub records_by_hash: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactCommit {
    pub commit_id: String,
    pub phase: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pact_ref: Option<PactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactRefSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactTransactionTraceBuildRequest {
    #[serde(default = "transaction_trace_build_request_schema")]
    pub schema: String,
    pub root_id: String,
    pub branch_id: String,
    pub transaction_id: String,
    pub authored_ref: PactRefSpec,
    pub pre_request_ref: PactRefSpec,
    pub post_request_alteration_ref: PactRefSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_ref: Option<PactRefSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_ref: Option<PactRefSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_response_alteration_ref: Option<PactRefSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_response_ref: Option<PactRefSpec>,
    #[serde(default)]
    pub alteration_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<PactCommit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_trace: Option<crate::transaction_trace::TransactionTrace>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactTransactionTrace {
    #[serde(default = "transaction_trace_schema")]
    pub schema: String,
    pub root_id: String,
    pub branch_id: String,
    pub transaction_id: String,
    pub authored_ref: PactRef,
    pub pre_request_ref: PactRef,
    pub post_request_alteration_ref: PactRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_ref: Option<PactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_ref: Option<PactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_response_alteration_ref: Option<PactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_response_ref: Option<PactRef>,
    #[serde(default)]
    pub alteration_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<PactCommit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_trace: Option<crate::transaction_trace::TransactionTrace>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum LedgerHistory {
    #[default]
    Complete,

    Lazy {
        trace_count: usize,
    },
}

impl ContextTreeLedger {
    pub fn is_lazy(&self) -> bool {
        matches!(self.history, LedgerHistory::Lazy { .. })
    }

    pub fn trace_count(&self) -> usize {
        match self.history {
            LedgerHistory::Complete => self.event_index.transaction_traces.len(),
            LedgerHistory::Lazy { trace_count } => {
                trace_count + self.event_index.transaction_traces.len().saturating_sub(1)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ContextTreeLedger {
    #[serde(default = "root_store_schema")]
    pub schema: String,
    pub root_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<PactParentProvenance>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<PactBaseProvenance>,
    pub branches: BTreeMap<String, PactBranch>,
    #[serde(default)]
    pub content: ContentStore,
    #[serde(default)]
    pub event_index: BranchEventIndex,
    pub current_branch_id: String,

    #[serde(skip)]
    pub history: LedgerHistory,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactBranch {
    pub branch_id: String,
    pub root_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_ref: Option<PactRef>,
    pub base_ref: PactRef,
    pub head_revision: u64,
    #[serde(default)]
    pub lifecycle_tick: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transaction_traces: Vec<crate::transaction_trace::TransactionTrace>,
    pub current: PactView,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

pub const PACT_DERIVED_DEPTH_AUX_KEY: &str = "pact.depth.derived";

impl PactBranch {
    pub fn derived_depth(&self) -> bool {
        self.aux
            .get(PACT_DERIVED_DEPTH_AUX_KEY)
            .and_then(Value::as_bool)
            == Some(true)
    }

    pub(crate) fn mark_derived_depth(&mut self) {
        self.aux
            .insert(PACT_DERIVED_DEPTH_AUX_KEY.to_string(), Value::Bool(true));
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactView {
    pub tree: ContextTreeView,
    pub branch_id: String,
    pub head_revision: u64,
    #[serde(default)]
    pub lifecycle_seen: BTreeMap<String, u64>,
    #[serde(default)]
    pub message_order: Vec<String>,
    #[serde(default)]
    pub branch_refs: Vec<PactRef>,
    #[serde(default)]
    pub content_refs: BTreeMap<String, ContentRecordSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ContextTreeView {
    pub root_id: String,
    pub stats: PactStoreStats,
    pub root: Component,
    pub index: ComponentViewIndex,
    pub first_seen: BTreeMap<String, u64>,
    pub tool_pairs: BTreeMap<String, ToolPairRecord>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComponentPath {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComponentViewIndex {
    #[serde(default)]
    pub by_id: BTreeMap<String, ComponentPath>,
    #[serde(default)]
    pub child_ids: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_type: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_aux_key: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct BranchEventIndex {
    #[serde(default)]
    pub by_transaction: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub transaction_traces: BTreeMap<String, PactTransactionTrace>,
    #[serde(default)]
    pub by_role: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_type: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_aux_key: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactStore {
    pub schema: String,
    pub base: Value,
    pub head_revision: u64,
    pub current: PactProjection,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactProjection {
    pub tree: ContextTreeView,
    pub blobs: BTreeMap<String, BlobRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactStoreStats {
    #[serde(rename = "component_count")]
    pub node_count: usize,
    pub edge_count: usize,
    pub blob_count: usize,
    pub content_chars: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct ComponentRecord {
    pub id: String,
    #[serde(rename = "componentType")]
    pub node_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub offset: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pact_depth: Option<i64>,
    pub created_at_ns: u64,
    pub creation_index: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(default, alias = "cadence", skip_serializing_if = "Option::is_none")]
    pub cad: Option<i64>,
    #[serde(default, skip_serializing_if = "ComponentLifecycle::is_empty")]
    pub lifecycle: ComponentLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, alias = "tag", skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_ids: Vec<String>,

    #[serde(default, rename = "attrs", skip_serializing_if = "BTreeMap::is_empty")]
    pub xml_attrs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aux: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_capture: Option<crate::components::ContentCapturePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<crate::components::PactSourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_status: Option<String>,
}

impl ComponentRecord {
    pub(crate) fn from_component_with_parent(
        component: &Component,
        parent_id: Option<String>,
    ) -> Self {
        let mut aux = component.aux.clone();
        if let ComponentKind::Media(payload) = component.kind() {
            aux.extend(payload.aux.clone());
            aux.entry("media_kind".to_string())
                .or_insert_with(|| Value::String(payload.media_kind.as_str().to_string()));
            if let Some(value) = &payload.mime_type {
                aux.entry("mime_type".to_string())
                    .or_insert_with(|| Value::String(value.clone()));
            }
            if let Some(value) = &payload.label {
                aux.entry("label".to_string())
                    .or_insert_with(|| Value::String(value.clone()));
            }
            if let Some(value) = &payload.filename {
                aux.entry("filename".to_string())
                    .or_insert_with(|| Value::String(value.clone()));
            }
            if let Some(value) = payload.width {
                aux.entry("width".to_string())
                    .or_insert_with(|| Value::Number(value.into()));
            }
            if let Some(value) = payload.height {
                aux.entry("height".to_string())
                    .or_insert_with(|| Value::Number(value.into()));
            }
            if let Some(value) = payload.duration_ms {
                aux.entry("duration_ms".to_string())
                    .or_insert_with(|| Value::Number(value.into()));
            }
            if let Some(value) = payload.page_count {
                aux.entry("page_count".to_string())
                    .or_insert_with(|| Value::Number(value.into()));
            }
            match &payload.body {
                MediaBody::InlineBytes { bytes, encoding } => {
                    aux.entry("media_content".to_string()).or_insert_with(|| {
                        json!({
                            "data": crate::file_format::base64_encode(bytes),
                            "encoding": encoding.as_deref().unwrap_or("base64"),
                        })
                    });
                }
                MediaBody::ExternalRef(external_ref) => {
                    aux.entry("external_ref".to_string()).or_insert_with(|| {
                        serde_json::to_value(external_ref).unwrap_or(Value::Null)
                    });
                }
                MediaBody::Url { url } => {
                    aux.entry("url".to_string())
                        .or_insert_with(|| Value::String(url.clone()));
                }
                MediaBody::InlineText { .. } | MediaBody::ContentRef(_) => {}
            }
        }
        Self {
            id: component.id.clone(),
            node_type: component.component_type().to_string(),
            kind: component.subtype.clone(),
            role: component.role.clone(),
            parent_id,
            offset: component.offset.into(),
            pact_depth: component.pact_depth.map(Into::into),
            created_at_ns: component.created_at_ns,
            creation_index: component.creation_index,
            ttl: component.lifecycle.policy.ttl,
            cad: component.lifecycle.policy.cad,
            lifecycle: component_record_lifecycle(component),
            key: component.key.clone(),
            tags: component.tags.clone(),
            child_ids: component
                .children
                .iter()
                .map(|child| child.id.clone())
                .collect(),
            xml_attrs: component.xml_attrs().cloned().unwrap_or_default(),
            aux,
            content: component.content().map(str::to_string),
            content_ref: component
                .content_ref()
                .map(|content_ref| content_ref.id.clone()),
            content_preview: None,
            tool_call_id: component.tool_call_id().map(str::to_string),
            tool_name: component.tool_name().map(str::to_string),
            parameters: component.parameters().cloned(),
            parameters_ref: component.parameters_ref().cloned(),
            success: component.success(),
            result: component.result().cloned(),
            result_ref: component.result_ref().cloned(),
            error_message: component.error_message().map(str::to_string),
            error_ref: component.error_ref().cloned(),
            content_capture: component.content_capture(),
            source_ref: component.source_ref().cloned(),
            content_status: component.content_status().map(str::to_string),
        }
    }
}

fn component_record_lifecycle(component: &Component) -> ComponentLifecycle {
    ComponentLifecycle {
        state: component.lifecycle.state,
        revision: component.lifecycle.revision,
        policy: ComponentLifecyclePolicy::default(),
        internal_refs: component.lifecycle.internal_refs.clone(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolPairRecord {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub result_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BlobRecord {
    pub id: String,
    pub chars: usize,
    pub preview: String,
}

fn transaction_trace_schema() -> String {
    TRANSACTION_TRACE_SCHEMA.to_string()
}

fn transaction_trace_build_request_schema() -> String {
    TRANSACTION_TRACE_BUILD_REQUEST_SCHEMA.to_string()
}

fn root_store_schema() -> String {
    ROOT_STORE_SCHEMA.to_string()
}
