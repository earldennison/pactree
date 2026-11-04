use super::layout::{COMMIT_RECORD_SCHEMA, FROZEN_CYCLE_SCHEMA, HEAD_RECORD_SCHEMA};
use super::storage::file_view_payload;
use crate::components::ExternalRef;
use crate::store::{PactRef, PactView};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFilePageSummary {
    pub page_type: String,
    pub codec: String,
    pub page_seq: u64,
    pub record_count: usize,
    pub offset: u64,
    pub payload_bytes: usize,
    pub stored_payload_bytes: usize,
    pub decoded_payload_bytes: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileManifest {
    pub schema: String,
    pub version: u16,
    #[serde(default)]
    pub file_kind: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub file_bytes: usize,
    pub page_count: usize,
    pub record_count: usize,
    pub symbol_count: usize,
    pub content_record_count: usize,
    pub content_chunk_count: usize,
    #[serde(default)]
    pub content_manifest_count: usize,
    pub root_count: usize,
    pub branch_count: usize,
    pub node_count: usize,
    pub trace_count: usize,
    pub component_count: usize,
    pub component_transaction_count: usize,
    pub component_payload_count: usize,
    pub view_count: usize,
    #[serde(default)]
    pub commit_count: usize,
    #[serde(default)]
    pub frozen_cycle_count: usize,
    #[serde(default)]
    pub head_count: usize,
    #[serde(default)]
    pub cycle_count: usize,
    #[serde(default)]
    pub cycle_ranges: Vec<PactFileCycleRange>,
    pub index_count: usize,
    #[serde(default)]
    pub footer_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_footer: Option<PactFileFooter>,
    pub last_valid_offset: usize,
    pub pages: Vec<PactFilePageSummary>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileFooter {
    pub version: u16,
    pub footer_len: u16,
    pub manifest_offset: u64,
    pub branch_head_offset: u64,
    pub hot_index_offset: u64,
    pub last_commit_offset: u64,
    pub checksum: String,
    pub offset: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactRecordAddress {
    pub page_offset: u64,
    pub page_seq: u64,
    pub record_seq: u64,
    pub record_kind: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_len: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactContentChunkAddress {
    pub content_id: String,
    pub ordinal: usize,
    pub page_offset: u64,
    pub page_seq: u64,
    pub record_seq: u64,
    pub decoded_start_byte: usize,
    pub decoded_end_byte: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct FrozenCycleRecord {
    #[serde(default = "frozen_cycle_schema")]
    pub schema: String,
    pub cycle: u64,
    pub revision: u64,
    pub root_ref: String,
    #[serde(default)]
    pub index_refs: Vec<String>,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CommitRecord {
    #[serde(default = "commit_record_schema")]
    pub schema: String,
    pub branch_id: String,
    pub cycle: u64,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_tick: Option<u64>,
    #[serde(default)]
    pub ops: Vec<CommitRecordOp>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HeadRecord {
    #[serde(default = "head_record_schema")]
    pub schema: String,
    pub branch_id: String,
    pub head_revision: u64,
    pub head_cycle: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_tick: Option<u64>,
    pub commit_offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_node_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_node_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_ref_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub record_sequence_ids: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CommitRecordOp {
    InsertComponent {
        parent_id: String,
        node_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<i64>,
    },
    ReplaceComponent {
        component_id: String,
        node_ref: String,
    },
    DeleteComponent {
        component_id: String,
    },
    AppendComponents {
        parent_id: String,
        node_refs: Vec<String>,
    },
    SetComponentAux {
        component_id: String,
        key_symbol: String,
        value_ref: String,
    },
    UnsetComponentAux {
        component_id: String,
        key_symbol: String,
    },
    AttachContentRef {
        component_id: String,
        blob_id: String,
    },
    ReplaceContentRange {
        component_id: String,
        range: PactFileRange,
        blob_id: String,
    },
}

fn frozen_cycle_schema() -> String {
    FROZEN_CYCLE_SCHEMA.to_string()
}

fn commit_record_schema() -> String {
    COMMIT_RECORD_SCHEMA.to_string()
}

fn head_record_schema() -> String {
    HEAD_RECORD_SCHEMA.to_string()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileCycleRange {
    pub cycle: u64,
    pub first_component_id: String,
    pub last_component_id: String,
    pub first_created_at_ns: u64,
    pub last_created_at_ns: u64,
    pub node_count: usize,
    pub message_count: usize,
    pub tool_call_count: usize,
    pub tool_result_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileRange {
    pub start: u64,
    pub end: u64,
}

impl PactFileRange {
    pub fn contains(&self, value: u64) -> bool {
        self.start <= value && value <= self.end
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileProjectionFilters {
    pub view: Option<String>,
    pub component_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileProjectionRange {
    pub kind: String,
    pub syntax: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactFileProjection {
    pub schema: String,
    pub branch_id: String,
    pub revision: u64,
    pub scope: String,
    pub filters: PactFileProjectionFilters,
    #[serde(serialize_with = "serialize_projection_branch")]
    pub branch: PactView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<PactFileProjectionRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_record: Option<PactViewRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<Option<u64>>,
}

fn serialize_projection_branch<S>(branch: &PactView, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let value = file_view_payload(branch, true).map_err(serde::ser::Error::custom)?;
    value.serialize(serializer)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactFileValidation {
    pub ok: bool,
    pub status: String,
    pub manifest: PactFileManifest,
    pub latest_valid_record_sequence: Option<u64>,
    pub latest_valid_transaction_id: Option<String>,
    pub latest_valid_branch_heads: BTreeMap<String, u64>,
    pub recoverability: String,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactFileQueryResult {
    pub schema: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub matches: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactTextSearchQuery {
    pub text: String,
    #[serde(default = "default_text_search_scope", alias = "stream")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default = "default_text_search_limit")]
    pub limit: usize,
    #[serde(default = "default_text_search_preview_chars")]
    pub preview_chars: usize,
}

impl PactTextSearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            scope: default_text_search_scope(),
            selector: None,
            case_insensitive: false,
            regex: false,
            limit: default_text_search_limit(),
            preview_chars: default_text_search_preview_chars(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactTextSearchResult {
    pub schema: String,
    pub query: PactTextSearchQuery,
    pub hits: Vec<PactTextSearchHit>,
    #[serde(default = "default_text_search_source")]
    pub source: String,
    #[serde(default)]
    pub fallback_used: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_stats: Option<PactSelectorStreamStats>,
}

fn default_text_search_source() -> String {
    "record_scan".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileSelectorQuery {
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    #[serde(default = "default_selector_limit")]
    pub limit: usize,
    #[serde(default = "default_selector_order")]
    pub order: String,
}

impl PactFileSelectorQuery {
    pub fn new(selector: impl Into<String>) -> Self {
        Self {
            selector: selector.into(),
            branch_id: None,
            limit: default_selector_limit(),
            order: default_selector_order(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileSelectorResult {
    pub schema: String,
    pub query: PactFileSelectorQuery,
    pub temporal: PactTemporalSelectorReport,
    pub source: String,
    pub fallback_used: bool,
    pub hits: Vec<PactSelectorHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_stats: Option<PactSelectorStreamStats>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactTemporalSelectorReport {
    pub raw: String,
    pub normalized: String,
    pub cycles: Vec<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactSelectorHit {
    pub component_id: String,
    pub branch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_ref: Option<String>,
    pub element_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub resident_in_current_surface: bool,
    pub archived_in_pact: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactSelectorStreamStats {
    pub records_seen: usize,
    pub records_temporal_rejected: usize,
    pub records_selector_rejected: usize,
    pub payloads_touched: usize,
    pub source: String,
    pub fallback_used: bool,
}

fn default_selector_limit() -> usize {
    100
}

fn default_selector_order() -> String {
    "newest".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactTextSearchHit {
    pub component_id: String,
    pub branch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u64>,
    pub element_kind: String,
    pub field: String,
    pub match_range: TextRange,
    pub exact_text_preview: String,
    pub node_text_len_bytes: u32,
    pub node_text_len_chars: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_container: Option<MessageContainerHit>,
    pub lifecycle: PactNodeLifecycle,
    pub visibility: NodeVisibility,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TextRange {
    pub byte_start: u32,
    pub byte_end: u32,
    pub char_start: u32,
    pub char_end: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeVisibility {
    pub resident_in_current_surface: bool,
    pub archived_in_pact: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactNodeLifecycle {
    pub born: NodeBirth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_as_this: Option<NodeSeen>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeBirth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeSeen {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MessageContainerHit {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokenizer_ref: Option<String>,
    pub token_count_estimated: bool,
}

fn default_text_search_scope() -> String {
    "all".to_string()
}

fn default_text_search_limit() -> usize {
    20
}

fn default_text_search_preview_chars() -> usize {
    240
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileHead {
    pub schema: String,
    pub file_path: String,
    pub file_kind: String,
    pub root_id: String,
    pub branch_id: String,
    pub current_branch_id: String,
    pub is_current_branch: bool,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    pub pact_ref: PactRef,
    pub branch_count: usize,
    pub trace_count: usize,
    pub record_count: usize,
    pub node_count: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct PactViewRecords {
    #[serde(default)]
    pub views: Vec<PactViewRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactViewRecord {
    pub view_id: String,
    pub root_id: String,
    pub source_branch_id: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_boundary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_transaction_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_transaction_end: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_component_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_node_refs: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_refs: Vec<ExternalRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub conflict_policy_hints: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactViewMergePreflight {
    pub schema: String,
    pub view_id: String,
    pub mode: String,
    pub source_branch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_branch_id: Option<String>,
    pub source_head_revision: Option<u64>,
    pub view_head_revision: Option<u64>,
    pub mount_revision: Option<u64>,
    #[serde(default)]
    pub changed_component_ids: Vec<String>,
    #[serde(default)]
    pub touched_node_refs: Vec<Value>,
    #[serde(default)]
    pub external_refs: Vec<ExternalRef>,
    #[serde(default)]
    pub conflicts: Vec<Value>,
    pub merge_ready: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactViewMergeReport {
    pub schema: String,
    pub view_id: String,
    pub policy: String,
    pub source_branch_id: String,
    pub view_branch_id: String,
    #[serde(default)]
    pub merged_component_ids: Vec<String>,
    pub conflicts: Vec<Value>,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactViewRebaseReport {
    pub schema: String,
    pub view_id: String,
    pub source_branch_id: String,
    pub view_branch_id: String,
    pub previous_parent_revision: Option<u64>,
    pub parent_revision: u64,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactFileRepairReport {
    pub schema: String,
    pub repaired_path: String,
    pub source_validation: PactFileValidation,
    pub repaired_validation: PactFileValidation,
    pub truncated_tail: bool,
    pub rebuilt_indexes: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactFileImportReport {
    pub schema: String,
    pub source_path: String,
    pub target_path: String,
    pub imported_branch_ids: Vec<String>,
    pub merged_component_count: usize,
    pub merged_view_count: usize,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PactFileSliceMode {
    CurrentHeadOnly,
    TransactionRange,
    BranchWithLineage,
    BranchCopyOnly,
    FullRoot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PactFileSliceSpec {
    pub mode: PactFileSliceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_cycle: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_cycle: Option<u64>,
}
