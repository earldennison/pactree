use crate::atomic_output::{PublishMode, publish_bytes};
use crate::components::Component;
use crate::error::{PactError, PactResult};
use crate::store::{
    ComponentRecord, ContentRecord, ContentStorageRef, ContentStore, ContextTreeLedger, PactBranch,
    PactRef, PactTransactionTrace, PactView, ProjectionTreeScope,
};

mod builder;
mod components;
mod content;
mod conversion;
pub(crate) mod corpus;
mod delta;
mod english_lexpack;
mod history;
mod inspect;
mod io;
mod layout;
mod page_codec;
mod parent;
mod parent_aware;
mod projection_replay;
mod query;
mod record_codec;
mod scan;
mod shell_lexpack;
mod state;
mod storage;
mod symbols;
mod text_search;
mod util;
mod validation;
mod value_codec;
mod view;
mod wire;

pub use crate::pact_file::change_feed::{
    PactTextChange, PactTextChangeCursor, PactTextChangePage, file_text_changes,
};
pub use crate::pact_file::search_change_feed::{
    PactSearchChange, PactSearchChangeKind, PactSearchChangePage, PactSearchDependencyScope,
    file_search_changes,
};
pub use builder::PactFileBuilder;
use builder::{
    HOT_QUERY_INDEX_NAME, encode_index_record, hot_node_depth_keys, hot_node_kind,
    hot_query_shard_key,
};
use components::{
    collect_aux_namespaces, collect_view_changed_components, collect_view_conflicts,
    decode_view_records, find_view, merge_view_records, push_index_value, unique_branch_id,
    validate_view_records, view_projection_branch, view_projection_revision,
    view_records_from_records,
};

use content::{
    ContentCollector, compress_zstd_archive, compress_zstd_chunk, compress_zstd_with_dict,
    content_chunks_from_records, content_payloads_by_hash, content_payloads_by_id,
    content_preview_inline_by_id, content_texts_from_records, decode_content_chunk_bytes,
    decode_content_payload_bytes, decode_content_payload_text, decompress_page_payload,
    decompress_zstd_chunk,
};
pub(crate) fn compact_indexed_node(mut value: serde_json::Value) -> serde_json::Value {
    const POSITIONAL_FIELDS: &[&str] = &[
        "id",
        "componentType",
        "type",
        "role",
        "parent_id",
        "offset",
        "pact_depth",
        "created_at_ns",
        "creation_index",
        "child_ids",
        "children",
        "attrs",
        "aux",
        "content",
        "content_ref",
        "content_preview",
        "ttl",
        "cad",
        "lifecycle",
        "key",
        "tags",
        "tool_call_id",
        "tool_name",
        "parameters",
        "parameters_ref",
        "success",
        "result",
        "result_ref",
        "error_message",
        "error_ref",
        "content_capture",
        "source_ref",
        "content_status",
    ];
    crate::pact_file::canonicalize_component_depth(&mut value);
    if let Some(object) = value.as_object_mut() {
        object.remove("child_ids");
        object.remove("children");
    }
    let extensions = value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(key, value)| {
                    (!POSITIONAL_FIELDS.contains(&key.as_str())
                        && !matches!(key.as_str(), "priority" | "transaction_index"))
                        || (matches!(key.as_str(), "priority" | "transaction_index")
                            && value.as_u64().unwrap_or_default() != 0)
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    let compact = record_codec::compact_canonical_record_payload(RecordKind::Node, value);
    if extensions.is_empty() {
        compact
    } else {
        serde_json::json!([compact, extensions])
    }
}

pub(crate) fn expand_indexed_node(value: serde_json::Value) -> PactResult<serde_json::Value> {
    let (compact, extensions) = match value {
        serde_json::Value::Array(mut wrapper)
            if wrapper.len() == 2 && wrapper[0].is_array() && wrapper[1].is_object() =>
        {
            let extensions = wrapper.pop().and_then(|value| value.as_object().cloned());
            (wrapper.pop().unwrap_or_default(), extensions)
        }
        compact => (compact, None),
    };
    let expanded = record_codec::expand_canonical_record_payload("node", compact)?;
    let expanded_fields = expanded.as_object().cloned().unwrap_or_default();
    let mut canonical = crate::Component::from_component_value(&expanded)
        .map(|component| component.project_component_value())
        .map_err(|error| {
            PactError::corrupt(format!("invalid compact indexed component: {error}"))
        })?;
    let object = canonical
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt("canonical indexed component is not an object"))?;
    for (key, value) in expanded_fields {
        object.insert(key, value);
    }
    if let Some(extensions) = extensions {
        object.extend(extensions);
    }
    Ok(canonical)
}

pub use conversion::{
    file_convert_exported_records_json, file_convert_exported_records_json_owned,
    file_convert_exported_records_reader, file_convert_pre_v01_file,
};
pub use corpus::file_select_json;
use corpus::{select_records, selector_reaches_prefork_history};
pub use delta::{DeltaProjection, project_delta_overlay};
pub(crate) use history::ledger_from_records;
pub use history::{
    PactRepackOptions, file_append_commit_json, file_append_commit_record,
    file_append_commit_record_with_content, file_append_commit_with_content_json,
    file_commit_context_tree, file_commit_context_tree_json, file_reanchor_json,
    file_reindex_head_json, file_repack_json, file_repack_packed_v1_json,
    file_repack_with_options_json,
};
use history::{
    append_missing_file_level_content_records, append_reindexed_head_record, apply_slice_spec,
    branch_heads_from_records, ledger_from_records_with_commits, packed_v1_to_v0_bytes,
    records_with_monotonic_commit_cycles, repack_records_preserving_history,
};
pub use inspect::{
    file_bench_storage_json, file_bench_storage_with_profile_json,
    file_export_storage_profile_json, file_info_json, file_inspect_full_json, file_inspect_json,
    file_live_shape_info_json, file_manifest_info_json, file_page_compare_json,
    file_storage_audit_json, file_token_span_stats_json,
};
use io::{
    append_bytes, read_footer_from_end, read_index_record_named_from_file,
    read_page_records_from_file, read_page_records_matching_from_file, read_record_from_file,
    read_symbol_table_prefix_from_file,
};
use layout::{
    BUILTIN_SYMBOLS, CONTENT_HASH_SCHEME, FILE_FOOTER_LEN, FILE_FOOTER_MAGIC, FILE_HEADER_LEN,
    FILE_VERSION, FROZEN_CYCLE_SCHEMA, HEAD_RECORD_SCHEMA, PACT_BUNDLE_SCHEMA, PAGE_HEADER_LEN,
    PageKind, RECORD_HEADER_LEN, RecordKind, TRACE_COMPACT_V2, decode_page_kind,
};

pub use layout::{
    DEFAULT_FREEZE_COMMIT_BYTES_THRESHOLD, DEFAULT_FREEZE_CYCLE_INTERVAL, PACT_FILE_HEAD_SCHEMA,
    PACT_FILE_PROJECTION_SCHEMA, PACT_FILE_SCHEMA,
};

use page_codec::{
    decode_record_payload_with_symbols, encode_record_payload, latest_valid_footer,
    parse_page_records, parse_page_records_with_symbols, parse_page_records_with_symbols_filter,
    prepare_binary_payload_slots, read_footer_at, write_footer, write_page,
};

pub use parent::{ParentResolverConfig, VerifiedParent};
pub use parent_aware::{
    ParentAwareProjection, ParentAwareScope, file_select_with_parents, project_with_parents,
};
use projection_replay::{
    commit_trace_base_from_records, project_records_revision_file_wire_value,
    transaction_traces_from_commit_records_from,
};
pub use projection_replay::{
    file_batch_open_head_fast_json, file_fast_open_summary_json, file_open_head_fast_json,
    file_project_head_json, file_project_revision_json,
};
use query::{
    enrich_manifest_from_records, filter_projection_array, filter_projection_array_any, node_cycle,
    query_component_payload_records, query_records, try_fast_query_json,
};

use record_codec::{
    TRACE_BOUNDARY_REF_KEYS, compact_canonical_record_payload, compact_trace_ref_value,
    object_looks_like_pact_ref,
};
pub use scan::{
    ScanMode, ScannedFile, ScannedRecord, scan_file, scan_file_with_mode, scan_pact_file,
};
use scan::{
    invalidate_scan_cache, scan_cache_snapshot, scan_file_cached, scan_file_page_summary_from_path,
};

use storage::{
    apply_capture_policy_to_node_value, collect_content_ref_ids_from_value, node_records_by_id,
    omitted_duplicate_branch_base_bytes,
};
use symbols::{SymbolTable, estimate_symbol_saved_bytes, string_counts_from_records};
pub use text_search::file_text_search_json;
pub(crate) use util::{base64_decode, base64_encode};
use util::{escape_xml_attr, read_u16, read_u32, read_u64, stable_checksum, stable_hash};
pub use util::{should_freeze_commit_bytes, should_freeze_cycle_interval};
use validation::{ref_id_from_content_ref_value, ref_id_from_refish_value};
pub use validation::{validation_from_scan, validation_from_scan_ref};
use value_codec::{PactValueCodecOptions, encode_pact_value_payload_with_symbols_and_options};

pub use view::{
    view_archived_json, view_archived_rows_json, view_head_json, view_head_rows_json, view_head_xml,
};
pub use wire::{
    CommitRecord, CommitRecordOp, FrozenCycleRecord, HeadRecord, MessageContainerHit, NodeBirth,
    NodeSeen, NodeVisibility, PactContentChunkAddress, PactFileCycleRange, PactFileFooter,
    PactFileHead, PactFileImportReport, PactFileManifest, PactFilePageSummary, PactFileProjection,
    PactFileProjectionFilters, PactFileProjectionRange, PactFileQueryResult, PactFileRange,
    PactFileRepairReport, PactFileSelectorQuery, PactFileSelectorResult, PactFileSliceMode,
    PactFileSliceSpec, PactFileValidation, PactNodeLifecycle, PactRecordAddress, PactSelectorHit,
    PactSelectorStreamStats, PactTemporalSelectorReport, PactTextSearchHit, PactTextSearchQuery,
    PactTextSearchResult, PactViewMergePreflight, PactViewMergeReport, PactViewRebaseReport,
    PactViewRecord, PactViewRecords, TextRange,
};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReanchorOptions {
    pub freeze_cycle_interval: Option<u64>,
    pub freeze_commit_bytes: Option<usize>,
}

impl Default for ReanchorOptions {
    fn default() -> Self {
        Self {
            freeze_cycle_interval: Some(DEFAULT_FREEZE_CYCLE_INTERVAL),
            freeze_commit_bytes: None,
        }
    }
}

impl ReanchorOptions {
    pub fn no_anchors() -> Self {
        Self {
            freeze_cycle_interval: Some(0),
            freeze_commit_bytes: Some(0),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ContentPayload {
    id: String,
    kind: String,
    hash: String,
    bytes: usize,
    chars: Option<usize>,
    #[serde(
        default,
        rename = "preview_inline",
        alias = "preview",
        skip_serializing_if = "Option::is_none"
    )]
    preview_inline: Option<String>,
    chunk_count: usize,
    codec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ContentChunkPayload {
    content_id: String,
    ordinal: usize,
    hash: String,
    bytes: usize,
    codec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ref_content_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ref_ordinal: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ContentManifestPayload {
    manifest_id: String,
    kind: String,
    hash: String,
    bytes: usize,
    chars: Option<usize>,
    chunk_count: usize,
    chunks: Vec<ContentManifestChunkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aux: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ContentManifestChunkRef {
    key: String,
    ordinal: usize,
    content_id: String,
    hash: String,
    bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chars: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
struct ContentManifestInput {
    #[serde(default)]
    manifest_id: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    chunks: Vec<ContentManifestChunkInput>,
    #[serde(default)]
    aux: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct ContentManifestChunkInput {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IndexPayload {
    name: String,
    entries: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
struct EncodedRecord {
    kind: RecordKind,
    seq: u64,
    payload: Vec<u8>,
}

pub fn file_create_from_ledger_json(
    path: &Path,
    root_store_json: &str,
    _inline_limit: usize,
) -> PactResult<String> {
    file_create_from_ledger_with_options_json(
        path,
        root_store_json,
        PactFileCreateOptions::default(),
    )
}

pub fn file_create_from_ledger_with_previews_json(
    path: &Path,
    root_store_json: &str,
    preview_limit: usize,
) -> PactResult<String> {
    file_create_from_ledger_with_previews_and_token_span_json(
        path,
        root_store_json,
        preview_limit,
        true,
    )
}

pub fn file_create_from_ledger_with_previews_and_token_span_json(
    path: &Path,
    root_store_json: &str,
    preview_limit: usize,
    token_span_strings: bool,
) -> PactResult<String> {
    file_create_from_ledger_with_options_json(
        path,
        root_store_json,
        PactFileCreateOptions {
            include_preview_index: true,
            preview_limit,
            token_span_strings,
        },
    )
}

pub fn file_create_from_ledger_with_token_span_json(
    path: &Path,
    root_store_json: &str,
    token_span_strings: bool,
) -> PactResult<String> {
    file_create_from_ledger_with_options_json(
        path,
        root_store_json,
        PactFileCreateOptions {
            token_span_strings,
            ..PactFileCreateOptions::default()
        },
    )
}

fn file_create_from_ledger_with_options_json(
    path: &Path,
    root_store_json: &str,
    options: PactFileCreateOptions,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let omitted_base_bytes = omitted_duplicate_branch_base_bytes(&root_store);
    let file = crate::pact_file::encode_file_baseline(&root_store)?;
    publish_bytes(path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    let node_count = root_store
        .branches
        .get(&root_store.current_branch_id)
        .map_or(0, |branch| branch.current.tree.index.by_id.len());
    let trace_count = root_store.trace_count();
    let mut report = json!({
        "schema": "pact.file.v2",
        "version": 2,
        "file_kind": "pact",
        "file_bytes": file.len(),
        "branch_count": root_store.branches.len(),
        "trace_count": trace_count,
        "record_count": node_count + trace_count + root_store.branches.len() + 2,
        "node_count": node_count,
    });
    report["slimming"] = json!({
        "omitted_duplicate_branch_base_bytes": omitted_base_bytes,
        "include_preview_index": options.include_preview_index,
        "preview_limit": options.preview_limit,
        "token_span_strings": options.token_span_strings,
    });
    serde_json::to_string_pretty(&report)
        .map_err(|err| PactError::codec(format!("failed to encode .pact create report: {err}")))
}

pub fn file_create_from_ledger_json_zstd(
    path: &Path,
    root_store_json: &str,
    _inline_limit: usize,
) -> PactResult<String> {
    let root_store = decode_context_tree_ledger(root_store_json)?;
    let file = crate::pact_file::encode_file_baseline(&root_store)?;
    publish_bytes(path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    let node_count = root_store
        .branches
        .get(&root_store.current_branch_id)
        .map_or(0, |branch| branch.current.tree.index.by_id.len());
    let trace_count = root_store.trace_count();
    serde_json::to_string_pretty(&json!({
        "schema": "pact.file.v2",
        "version": 2,
        "file_kind": "pact",
        "file_bytes": file.len(),
        "branch_count": root_store.branches.len(),
        "trace_count": trace_count,
        "record_count": node_count + trace_count + root_store.branches.len() + 2,
        "node_count": node_count,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact create report: {err}")))
}

#[derive(Clone, Copy, Debug)]
struct PactFileCreateOptions {
    include_preview_index: bool,
    preview_limit: usize,
    token_span_strings: bool,
}

impl Default for PactFileCreateOptions {
    fn default() -> Self {
        Self {
            include_preview_index: false,
            preview_limit: crate::components::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
            token_span_strings: true,
        }
    }
}

pub fn file_append_view_records_json(
    path: &Path,
    view_records_json: &str,
    _inline_limit: usize,
) -> PactResult<String> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&raw) {
        let mut ledger = crate::pact_file::open_ledger(path)?;
        let mut views = v2_view_records(&ledger)?;
        let incoming = decode_view_records(view_records_json)?;
        let existing_view_ids = views
            .views
            .iter()
            .map(|view| view.view_id.clone())
            .collect::<BTreeSet<_>>();
        for view in &incoming.views {
            if existing_view_ids.contains(&view.view_id) {
                return Err(PactError::invalid(format!(
                    "view {} already exists; append-mode view replacement requires explicit reanchor",
                    view.view_id
                )));
            }
        }
        merge_view_records(&mut views, incoming);
        validate_view_records(&views, &ledger)?;
        set_v2_view_records(&mut ledger, &views)?;
        write_v2_ledger(path, &ledger, PublishMode::ReplaceExisting)?;
        return file_inspect_json(path);
    }
    let scanned = scan_file_cached(path, true)?;
    let validation = validation_from_scan_ref(&scanned);
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot append views to invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    let mut views = view_records_from_records(&scanned.records)?;
    let incoming = decode_view_records(view_records_json)?;
    let existing_view_ids = views
        .views
        .iter()
        .map(|view| view.view_id.clone())
        .collect::<BTreeSet<_>>();
    for view in &incoming.views {
        if existing_view_ids.contains(&view.view_id) {
            return Err(PactError::invalid(format!(
                "view {} already exists; append-mode view replacement requires explicit reanchor",
                view.view_id
            )));
        }
    }
    merge_view_records(&mut views, incoming);
    validate_view_records(&views, &root_store)?;

    let appended_views = views
        .views
        .iter()
        .filter(|view| !existing_view_ids.contains(&view.view_id))
        .collect::<Vec<_>>();
    if appended_views.is_empty() {
        return file_inspect_json(path);
    }
    let next_seq = scanned.manifest.record_count as u64 + 1;
    let next_page_seq = scanned
        .manifest
        .pages
        .iter()
        .map(|page| page.page_seq)
        .max()
        .unwrap_or(0)
        + 1;
    let mut records = Vec::new();
    for (index, view) in appended_views.into_iter().enumerate() {
        records.push(EncodedRecord {
            kind: RecordKind::View,
            seq: next_seq + index as u64,
            payload: encode_record_payload(RecordKind::View, view)?,
        });
    }
    let mut bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let page_offset = bytes.len() as u64;
    let refs = records.iter().collect::<Vec<_>>();
    write_page(&mut bytes, PageKind::Record, next_page_seq, &refs)?;
    let footer = scanned.manifest.latest_footer.clone().unwrap_or_default();
    write_footer(
        &mut bytes,
        footer.manifest_offset,
        footer.branch_head_offset,
        footer.hot_index_offset,
        footer.last_commit_offset,
    );
    append_bytes(path, &bytes[page_offset as usize..])?;
    file_inspect_json(path)
}

const V2_VIEWS_AUX_KEY: &str = "pact_file_views";

fn v2_view_records(ledger: &ContextTreeLedger) -> PactResult<PactViewRecords> {
    let Some(value) = ledger
        .branches
        .get(&ledger.current_branch_id)
        .and_then(|branch| branch.aux.get(V2_VIEWS_AUX_KEY))
    else {
        return Ok(PactViewRecords::default());
    };
    serde_json::from_value(value.clone())
        .map_err(|error| PactError::corrupt(format!("invalid Pact view metadata: {error}")))
}

fn set_v2_view_records(ledger: &mut ContextTreeLedger, views: &PactViewRecords) -> PactResult<()> {
    let branch = ledger
        .branches
        .get_mut(&ledger.current_branch_id)
        .ok_or_else(|| PactError::corrupt("current Pact branch is missing"))?;
    branch.aux.insert(
        V2_VIEWS_AUX_KEY.to_string(),
        serde_json::to_value(views)
            .map_err(|error| PactError::codec(format!("encode Pact views: {error}")))?,
    );
    Ok(())
}

fn write_v2_ledger(path: &Path, ledger: &ContextTreeLedger, mode: PublishMode) -> PactResult<()> {
    let bytes = crate::pact_file::encode_canonical_file(ledger)?;
    publish_bytes(path, &bytes, mode)?;
    invalidate_scan_cache(path);
    Ok(())
}

pub fn file_append_trace_record_json(path: &Path, trace_json: &str) -> PactResult<String> {
    let mut bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot append trace to invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let trace_value: Value = serde_json::from_str(trace_json)
        .map_err(|err| PactError::invalid(format!("invalid trace record JSON: {err}")))?;
    ensure_trace_content_refs_are_recorded(&trace_value, &scanned.records)?;
    let next_seq = scanned.manifest.record_count as u64 + 1;
    let next_page_seq = scanned
        .manifest
        .pages
        .iter()
        .map(|page| page.page_seq)
        .max()
        .unwrap_or(0)
        + 1;
    let record = EncodedRecord {
        kind: RecordKind::Trace,
        seq: next_seq,
        payload: encode_record_payload(RecordKind::Trace, &trace_value)?,
    };
    let page_offset = bytes.len() as u64;
    write_page(&mut bytes, PageKind::Record, next_page_seq, &[&record])?;
    let footer = scanned.manifest.latest_footer.unwrap_or_default();
    write_footer(
        &mut bytes,
        page_offset,
        footer.branch_head_offset,
        footer.hot_index_offset,
        footer.last_commit_offset,
    );
    append_bytes(path, &bytes[page_offset as usize..])?;
    file_inspect_json(path)
}

fn ensure_trace_content_refs_are_recorded(
    trace_value: &Value,
    records: &[ScannedRecord],
) -> PactResult<()> {
    let mut trace_content_refs = BTreeSet::new();
    collect_content_ref_ids_from_value(trace_value, &mut trace_content_refs);
    if trace_content_refs.is_empty() {
        return Ok(());
    }

    let recorded_content_ids = records
        .iter()
        .filter(|record| record.kind == RecordKind::Content.as_str())
        .filter_map(|record| record.payload.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let missing = trace_content_refs
        .into_iter()
        .filter(|content_id| !recorded_content_ids.contains(content_id.as_str()))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    Err(PactError::corrupt(format!(
        "cannot append trace before referenced content records are present: {}",
        missing.join(", ")
    )))
}

pub fn file_append_content_manifest_json(path: &Path, manifest_json: &str) -> PactResult<String> {
    let mut bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot append content manifest to invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let input: ContentManifestInput = serde_json::from_str(manifest_json)
        .map_err(|err| PactError::invalid(format!("invalid content manifest JSON: {err}")))?;
    let kind = input.kind.unwrap_or_else(|| "content.manifest".to_string());
    let planned_chunks = input
        .chunks
        .iter()
        .enumerate()
        .map(|(ordinal, chunk)| {
            let key = chunk
                .key
                .clone()
                .unwrap_or_else(|| format!("chunk_{ordinal}"));
            let chunk_kind = chunk.kind.clone().unwrap_or_else(|| kind.clone());
            let bytes = chunk.text.len();
            let chars = chunk.text.chars().count();
            let hash = stable_hash(&chunk_kind, chunk.text.as_bytes());
            json!({
                "key": key,
                "ordinal": ordinal,
                "kind": chunk_kind,
                "hash": hash,
                "bytes": bytes,
                "chars": chars,
            })
        })
        .collect::<Vec<_>>();
    let manifest_seed = serde_json::to_vec(&json!({
        "kind": kind,
        "chunks": planned_chunks,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode content manifest seed: {err}")))?;
    let manifest_hash = stable_hash("content.manifest", &manifest_seed);
    let manifest_id = input
        .manifest_id
        .unwrap_or_else(|| format!("mf_{}", &manifest_hash[..8]));
    if scanned.records.iter().any(|record| {
        record.kind == "content_manifest"
            && record.payload.get("manifest_id").and_then(Value::as_str) == Some(&manifest_id)
    }) {
        return Err(PactError::invalid(format!(
            "content manifest {manifest_id} already exists"
        )));
    }

    let existing = content_payloads_by_hash(&scanned.records)?;
    let mut content = ContentCollector::with_compression();
    let mut refs = Vec::new();
    let mut reused_chunks = 0usize;
    let mut total_bytes = 0usize;
    let mut total_chars = 0usize;
    for (ordinal, chunk) in input.chunks.iter().enumerate() {
        let key = chunk
            .key
            .clone()
            .unwrap_or_else(|| format!("chunk_{ordinal}"));
        let chunk_kind = chunk.kind.as_deref().unwrap_or(&kind);
        let hash = stable_hash(chunk_kind, chunk.text.as_bytes());
        let bytes_len = chunk.text.len();
        let chars_len = chunk.text.chars().count();
        total_bytes += bytes_len;
        total_chars += chars_len;
        let content_id = if let Some(payload) = existing.get(&hash) {
            reused_chunks += 1;
            payload.id.clone()
        } else {
            content.put_text(chunk_kind, &format!("{manifest_id}_{key}"), &chunk.text)?
        };
        refs.push(ContentManifestChunkRef {
            key,
            ordinal,
            content_id,
            hash,
            bytes: bytes_len,
            chars: Some(chars_len),
        });
    }

    let manifest = ContentManifestPayload {
        manifest_id: manifest_id.clone(),
        kind: kind.clone(),
        hash: manifest_hash,
        bytes: total_bytes,
        chars: Some(total_chars),
        chunk_count: refs.len(),
        chunks: refs,
        aux: input.aux,
    };

    let next_seq = scanned.manifest.record_count as u64 + 1;
    let next_page_seq = scanned
        .manifest
        .pages
        .iter()
        .map(|page| page.page_seq)
        .max()
        .unwrap_or(0)
        + 1;
    let mut records = Vec::new();
    let mut seq = next_seq;
    for payload in content.by_hash.values() {
        records.push(EncodedRecord {
            kind: RecordKind::Content,
            seq,
            payload: encode_record_payload(RecordKind::Content, payload)?,
        });
        seq += 1;
    }
    for chunk in &content.chunks {
        records.push(EncodedRecord {
            kind: RecordKind::ContentChunk,
            seq,
            payload: encode_record_payload(RecordKind::ContentChunk, chunk)?,
        });
        seq += 1;
    }
    records.push(EncodedRecord {
        kind: RecordKind::ContentManifest,
        seq,
        payload: encode_record_payload(RecordKind::ContentManifest, &manifest)?,
    });

    let page_offset = bytes.len() as u64;
    let refs = records.iter().collect::<Vec<_>>();
    write_page(&mut bytes, PageKind::Content, next_page_seq, &refs)?;
    let footer = scanned.manifest.latest_footer.unwrap_or_default();
    write_footer(
        &mut bytes,
        footer.manifest_offset,
        footer.branch_head_offset,
        footer.hot_index_offset,
        footer.last_commit_offset,
    );
    append_bytes(path, &bytes[page_offset as usize..])?;
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.content_manifest_append.v1"),
        "path": path.display().to_string(),
        "manifest_id": manifest_id,
        "kind": kind,
        "chunk_count": manifest.chunk_count,
        "new_content_count": content.by_hash.len(),
        "new_content_chunk_count": content.chunks.len(),
        "reused_chunk_count": reused_chunks,
        "appended_bytes": bytes.len().saturating_sub(page_offset as usize),
    }))
    .map_err(|err| {
        PactError::codec(format!(
            "failed to encode content manifest append report: {err}"
        ))
    })
}

pub fn file_content_manifest_payload_json(path: &Path, manifest_id: &str) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    let validation = validation_from_scan_ref(&scanned);
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot read content manifest from invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let manifest_record = scanned
        .records
        .iter()
        .rev()
        .find(|record| {
            record.kind == "content_manifest"
                && record.payload.get("manifest_id").and_then(Value::as_str) == Some(manifest_id)
        })
        .ok_or_else(|| PactError::not_found(format!("content manifest {manifest_id} not found")))?;
    let manifest: ContentManifestPayload = serde_json::from_value(manifest_record.payload.clone())
        .map_err(|err| {
            PactError::codec(format!("invalid content manifest {manifest_id}: {err}"))
        })?;
    let content = content_texts_from_records(&scanned.records);
    let mut chunks = manifest.chunks.clone();
    chunks.sort_by_key(|chunk| chunk.ordinal);
    let mut text = String::with_capacity(manifest.bytes);
    for chunk in chunks {
        let Some(chunk_text) = content.get(&chunk.content_id) else {
            return Err(PactError::not_found(format!(
                "content manifest {manifest_id} references missing content {}",
                chunk.content_id
            )));
        };
        text.push_str(chunk_text);
    }
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.content_manifest_payload.v1"),
        "manifest_id": manifest.manifest_id,
        "kind": manifest.kind,
        "hash": manifest.hash,
        "bytes": text.len(),
        "chars": text.chars().count(),
        "text": text,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode content manifest payload: {err}")))
}

pub fn file_content_payload_json(path: &Path, content_id: &str) -> PactResult<String> {
    let ledger = crate::pact_file::open_ledger(path)?;
    let content = ledger
        .content
        .records
        .get(content_id)
        .ok_or_else(|| PactError::not_found(format!("content {content_id} not found")))?;
    let text = content
        .storage
        .inline
        .clone()
        .or_else(|| {
            content
                .storage
                .inline_bytes
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
        })
        .ok_or_else(|| PactError::not_found(format!("content {content_id} has no text payload")))?;
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.content_payload.v1"),
        "content_id": content.id,
        "kind": content.kind,
        "hash": content.hash,
        "bytes": text.len(),
        "chars": text.chars().count(),
        "text": text,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode content payload: {err}")))
}

pub(crate) fn validate_file(path: &Path) -> PactResult<PactFileValidation> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&raw) {
        let committed_len = crate::pact_file::committed_prefix(&raw)
            .map(|bytes| bytes.len())
            .unwrap_or_default();
        let validation = crate::pact_file::validate(path)?;
        let ledger = crate::pact_file::open_ledger(path);
        return match ledger {
            Ok(ledger) => {
                let has_tail = committed_len != raw.len();
                let mut errors = validation.errors;
                if has_tail {
                    errors.push("uncommitted bytes follow the latest valid root slot".to_string());
                }
                let ok = validation.ok && !has_tail;
                Ok(v2_repair_validation(
                    &ledger,
                    raw.len(),
                    committed_len,
                    ok,
                    errors,
                ))
            }
            Err(error) => Ok(PactFileValidation {
                ok: false,
                status: "invalid".to_string(),
                manifest: PactFileManifest {
                    schema: crate::pact_file::layout::PACT_FILE_SCHEMA.to_string(),
                    version: crate::pact_file::layout::FILE_VERSION,
                    file_kind: "branch".to_string(),
                    file_bytes: raw.len(),
                    last_valid_offset: committed_len,
                    ..PactFileManifest::default()
                },
                latest_valid_record_sequence: None,
                latest_valid_transaction_id: None,
                latest_valid_branch_heads: BTreeMap::new(),
                recoverability: if committed_len > 0 {
                    "partially_recoverable"
                } else {
                    "unrecoverable"
                }
                .to_string(),
                errors: vec![error.to_string()],
            }),
        };
    }
    if crate::pact_pack::is_packed_v1(&raw) {
        let validation = match crate::pact_pack::PackedV1::read(&raw)
            .and_then(|container| container.project_head().map(|_| container))
        {
            Ok(container) => PactFileValidation {
                ok: true,
                status: "ok".to_string(),
                manifest: PactFileManifest {
                    schema: PACT_FILE_SCHEMA.to_string(),
                    version: FILE_VERSION,
                    file_kind: "packed-v1".to_string(),
                    capabilities: vec!["packed-v1".to_string(), "canonical-ledger".to_string()],
                    file_bytes: raw.len(),
                    page_count: container.pack_count(),
                    record_count: 0,
                    symbol_count: 0,
                    content_record_count: 0,
                    content_chunk_count: 0,
                    content_manifest_count: 0,
                    root_count: 1,
                    branch_count: 1,
                    node_count: 0,
                    trace_count: 0,
                    component_count: 0,
                    component_transaction_count: 0,
                    component_payload_count: 0,
                    view_count: 0,
                    commit_count: container.head_rev as usize + 1,
                    frozen_cycle_count: 0,
                    head_count: 1,
                    cycle_count: container.head_rev as usize + 1,
                    cycle_ranges: Vec::new(),
                    index_count: 0,
                    footer_count: 0,
                    latest_footer: None,
                    last_valid_offset: raw.len(),
                    pages: Vec::new(),
                },
                latest_valid_record_sequence: None,
                latest_valid_transaction_id: None,
                latest_valid_branch_heads: BTreeMap::new(),
                recoverability: "fully_recoverable".to_string(),
                errors: Vec::new(),
            },
            Err(err) => PactFileValidation {
                ok: false,
                status: "invalid".to_string(),
                manifest: PactFileManifest {
                    schema: PACT_FILE_SCHEMA.to_string(),
                    version: FILE_VERSION,
                    file_kind: "packed-v1".to_string(),
                    file_bytes: raw.len(),
                    ..PactFileManifest::default()
                },
                latest_valid_record_sequence: None,
                latest_valid_transaction_id: None,
                latest_valid_branch_heads: BTreeMap::new(),
                recoverability: "unrecoverable".to_string(),
                errors: vec![err.to_string()],
            },
        };
        return Ok(validation);
    }
    let scanned = scan_file_cached(path, true)?;
    Ok(validation_from_scan_ref(&scanned))
}

pub fn file_validate_json(path: &Path) -> PactResult<String> {
    serde_json::to_string_pretty(&validate_file(path)?)
        .map_err(|err| PactError::codec(format!("failed to encode .pact validation: {err}")))
}

pub fn file_project_tree_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    file_project_tree_json_with_scope(path, branch_id, revision, ProjectionTreeScope::Head)
}

pub(crate) fn file_open_ledger(path: &Path) -> PactResult<crate::store::ContextTreeLedger> {
    crate::pact_file::open_ledger(path).map_err(|error| match error {
        PactError::Io(error) => {
            PactError::storage(format!("failed to read .pact file {path:?}: {error}"))
        }
        other => other,
    })
}

pub fn file_root_id(path: &Path) -> PactResult<String> {
    Ok(file_open_ledger(path)?.root_id)
}

pub fn file_load_child_head_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    parent_config: &ParentResolverConfig,
) -> PactResult<String> {
    let ledger = file_open_ledger(path)?;
    let branch = branch_id
        .map(str::to_string)
        .unwrap_or_else(|| ledger.current_branch_id.clone());
    let projection = project_with_parents(
        Some(path),
        &ledger,
        &branch,
        revision,
        ParentAwareScope::ChildHead,
        parent_config,
    )?;
    let report = serde_json::json!({
        "schema": "pact.file.v0.load.v1",
        "path": path.display().to_string(),
        "branch_id": branch,
        "revision": revision,
        "root_id": ledger.root_id,
        "has_parent_provenance": ledger.parent.is_some(),
        "parent_ref": ledger.parent.as_ref().map(|parent| &parent.parent_ref),
        "scope": "child_head",
        "tree": projection.tree,
    });
    serde_json::to_string_pretty(&report).map_err(|err| PactError::codec(err.to_string()))
}

pub(crate) fn file_project_tree_json_with_scope(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    scope: ProjectionTreeScope,
) -> PactResult<String> {
    let root_store = file_open_ledger(path)?;
    let target_branch = branch_id.unwrap_or(&root_store.current_branch_id);
    let root_store_json = serde_json::to_string(&root_store).map_err(|err| {
        PactError::codec(format!("failed to encode reconstructed root store: {err}"))
    })?;
    crate::store::ledger_project_branch_tree_json_with_scope(
        &root_store_json,
        target_branch,
        revision,
        scope,
    )
}

pub fn file_hydrate_components_json(
    path: &Path,
    component_ids: &[String],
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    if component_ids.is_empty() {
        return Err(PactError::invalid(
            "file hydrate components requires at least one component id",
        ));
    }

    let session = scan::PactReadSession::open(path)?;
    let validation = session.validation();
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot hydrate components from invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }

    let root_store = ledger_from_records(session.records())?;
    let target_branch = branch_id.unwrap_or(&root_store.current_branch_id);
    let branch = root_store
        .branches
        .get(target_branch)
        .ok_or_else(|| PactError::not_found(format!("branch {target_branch:?} not found")))?;
    let target_revision = revision.unwrap_or(branch.head_revision);
    let requested: BTreeSet<String> = component_ids.iter().cloned().collect();
    let mut found = BTreeSet::new();
    let (projected_tree, _) = crate::store::project_branch_archive_component_tree(
        &root_store,
        branch,
        Some(target_revision),
    )?;
    let mut tree = prune_component_tree(projected_tree.root(), &requested, &mut found)
        .map(|component| component.project_component_value())
        .unwrap_or(Value::Null);
    crate::payload_hydration::hydrate_payload_refs(&mut tree, &root_store.content)?;
    supplement_hydrated_component_records(
        &mut tree,
        &mut found,
        &requested,
        session.records(),
        &root_store.content,
    )?;
    let found_component_ids = component_ids
        .iter()
        .filter(|component_id| found.contains(*component_id))
        .cloned()
        .collect::<Vec<_>>();
    let missing_component_ids = component_ids
        .iter()
        .filter(|component_id| !found.contains(*component_id))
        .cloned()
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.hydrate_components.v1"),
        "branch_id": target_branch,
        "revision": target_revision,
        "component_ids": component_ids,
        "found_component_ids": found_component_ids,
        "missing_component_ids": missing_component_ids,
        "tree": tree,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode hydrated component slice: {err}")))
}

fn supplement_hydrated_component_records(
    tree: &mut Value,
    found: &mut BTreeSet<String>,
    requested: &BTreeSet<String>,
    records: &[scan::ScannedRecord],
    content_store: &ContentStore,
) -> PactResult<()> {
    let node_records = component_records_by_id(records);
    let parent_by_id = component_parent_ids_from_commits(records);
    for component_id in requested {
        if found.contains(component_id) {
            continue;
        }
        let Some(mut chain) =
            hydrated_component_record_chain(component_id, &node_records, &parent_by_id)
        else {
            continue;
        };
        crate::payload_hydration::hydrate_payload_refs(&mut chain, content_store)?;
        graft_hydrated_component_chain(tree, chain);
        found.insert(component_id.clone());
    }
    Ok(())
}

fn component_records_by_id(records: &[scan::ScannedRecord]) -> BTreeMap<String, ComponentRecord> {
    let mut nodes = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "node") {
        if let Some(node) = component_record_from_file_value(record.payload.clone()) {
            nodes.insert(node.id.clone(), node);
        }
    }
    nodes
}

fn component_parent_ids_from_commits(records: &[scan::ScannedRecord]) -> BTreeMap<String, String> {
    let mut parents = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        for op in commit.ops {
            match op {
                CommitRecordOp::InsertComponent {
                    parent_id,
                    node_ref,
                    ..
                } => {
                    parents.insert(node_ref, parent_id);
                }
                CommitRecordOp::AppendComponents {
                    parent_id,
                    node_refs,
                } => {
                    for node_ref in node_refs {
                        parents.insert(node_ref, parent_id.clone());
                    }
                }
                CommitRecordOp::ReplaceComponent { .. }
                | CommitRecordOp::DeleteComponent { .. }
                | CommitRecordOp::SetComponentAux { .. }
                | CommitRecordOp::UnsetComponentAux { .. }
                | CommitRecordOp::AttachContentRef { .. }
                | CommitRecordOp::ReplaceContentRange { .. } => {}
            }
        }
    }
    parents
}

fn component_record_from_file_value(mut value: Value) -> Option<ComponentRecord> {
    if let Some(object) = value.as_object_mut() {
        object.entry("offset").or_insert(Value::Number(0.into()));
        object
            .entry("created_at_ns")
            .or_insert(Value::Number(0.into()));
        object
            .entry("creation_index")
            .or_insert(Value::Number(0.into()));
    }
    serde_json::from_value::<ComponentRecord>(value).ok()
}

fn hydrated_component_record_chain(
    component_id: &str,
    records: &BTreeMap<String, ComponentRecord>,
    parent_by_id: &BTreeMap<String, String>,
) -> Option<Value> {
    let mut seen = BTreeSet::new();
    let mut current_id = component_id.to_string();
    let mut child = component_record_tree_value(
        records.get(component_id)?,
        parent_by_id.get(component_id).cloned(),
        Vec::new(),
    )?;

    while seen.insert(current_id.clone()) {
        let Some(parent_id) = parent_by_id
            .get(&current_id)
            .cloned()
            .or_else(|| {
                records
                    .get(&current_id)
                    .and_then(|record| record.parent_id.clone())
            })
            .filter(|parent_id| parent_id != &current_id)
        else {
            break;
        };
        let Some(parent_record) = records.get(&parent_id) else {
            break;
        };
        let parent_parent_id = parent_by_id
            .get(&parent_id)
            .cloned()
            .or_else(|| parent_record.parent_id.clone());
        child = component_record_tree_value(parent_record, parent_parent_id, vec![child])?;
        current_id = parent_id;
    }

    Some(child)
}

fn component_record_tree_value(
    record: &ComponentRecord,
    parent_id: Option<String>,
    children: Vec<Value>,
) -> Option<Value> {
    let mut value = serde_json::to_value(record).ok()?;
    let object = value.as_object_mut()?;
    if let Some(parent_id) = parent_id {
        object.insert("parent_id".to_string(), Value::String(parent_id));
    }
    object.insert("children".to_string(), Value::Array(children));
    Some(value)
}

fn graft_hydrated_component_chain(tree: &mut Value, chain: Value) {
    if tree.is_null() {
        *tree = chain;
        return;
    }
    let Some(chain_id) = chain.get("id").and_then(Value::as_str).map(str::to_string) else {
        return;
    };
    if let Some(existing) = find_value_node_mut(tree, &chain_id) {
        merge_hydrated_component_chain(existing, chain);
        return;
    }
    if let Some(children) = tree.get_mut("children").and_then(Value::as_array_mut) {
        children.push(chain);
    }
}

fn merge_hydrated_component_chain(existing: &mut Value, chain: Value) {
    let Some(incoming_children) = chain.get("children").and_then(Value::as_array) else {
        return;
    };
    let Some(existing_children) = existing.get_mut("children").and_then(Value::as_array_mut) else {
        return;
    };
    for child in incoming_children {
        let child_id = child.get("id").and_then(Value::as_str).map(str::to_string);
        if let Some(child_id) = child_id.as_deref()
            && let Some(existing_child) = existing_children
                .iter_mut()
                .find(|existing| existing.get("id").and_then(Value::as_str) == Some(child_id))
        {
            merge_hydrated_component_chain(existing_child, child.clone());
            continue;
        }
        existing_children.push(child.clone());
    }
}

fn find_value_node_mut<'a>(node: &'a mut Value, component_id: &str) -> Option<&'a mut Value> {
    if node.get("id").and_then(Value::as_str) == Some(component_id) {
        return Some(node);
    }
    let children = node.get_mut("children").and_then(Value::as_array_mut)?;
    for child in children {
        if let Some(found) = find_value_node_mut(child, component_id) {
            return Some(found);
        }
    }
    None
}

fn prune_component_tree(
    component: &Component,
    target_ids: &BTreeSet<String>,
    found_ids: &mut BTreeSet<String>,
) -> Option<Component> {
    if target_ids.contains(&component.id) {
        found_ids.insert(component.id.clone());
        return Some(component.clone());
    }

    let mut pruned_children = Vec::new();
    for child in &component.children {
        if let Some(pruned_child) = prune_component_tree(child, target_ids, found_ids) {
            pruned_children.push(pruned_child);
        }
    }

    if pruned_children.is_empty() {
        return None;
    }

    let mut pruned = component.clone();
    pruned.children = pruned_children;
    Some(pruned)
}

pub fn file_open_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    file_projection_json(path, branch_id, revision, blob_inline_limit, None, None)
}

pub(crate) fn file_head_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    let head = file_head(path, branch_id, revision)?;
    serde_json::to_string_pretty(&head)
        .map_err(|err| PactError::codec(format!("failed to encode .pact head: {err}")))
}

pub(crate) fn file_head_xml(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    let head = file_head(path, branch_id, revision)?;
    let transaction_attr = head
        .transaction_id
        .as_deref()
        .map(|value| format!(" transaction_id=\"{}\"", escape_xml_attr(value)))
        .unwrap_or_default();
    Ok(format!(
        "<pact_head schema=\"{}\" file_path=\"{}\" file_kind=\"{}\" root_id=\"{}\" branch_id=\"{}\" current_branch_id=\"{}\" is_current_branch=\"{}\" revision=\"{}\"{} branch_count=\"{}\" trace_count=\"{}\" record_count=\"{}\" node_count=\"{}\" />\n",
        escape_xml_attr(&head.schema),
        escape_xml_attr(&head.file_path),
        escape_xml_attr(&head.file_kind),
        escape_xml_attr(&head.root_id),
        escape_xml_attr(&head.branch_id),
        escape_xml_attr(&head.current_branch_id),
        head.is_current_branch,
        head.revision,
        transaction_attr,
        head.branch_count,
        head.trace_count,
        head.record_count,
        head.node_count,
    ))
}

fn file_head(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<PactFileHead> {
    let ledger = crate::pact_file::open_ledger(path)?;
    let root_id = ledger.root_id.clone();
    let current_branch_id = ledger.current_branch_id.clone();
    let target_branch_id = branch_id.unwrap_or(&current_branch_id);
    let branch = ledger
        .branches
        .get(target_branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {target_branch_id}")))?;
    let target_revision = revision.unwrap_or(branch.head_revision);
    if target_revision > branch.head_revision {
        return Err(PactError::invalid(format!(
            "requested revision {target_revision} is after branch {target_branch_id} head {}",
            branch.head_revision
        )));
    }

    let transaction_id = ledger
        .event_index
        .transaction_traces
        .values()
        .filter(|trace| trace.branch_id == target_branch_id)
        .filter_map(|trace| {
            let revision = trace.post_request_alteration_ref.revision?;
            (revision <= target_revision).then_some((revision, trace.transaction_id.clone()))
        })
        .max_by_key(|(revision, _)| *revision)
        .map(|(_, transaction_id)| transaction_id);
    let pact_ref = crate::store::PactRef {
        root_id: root_id.clone(),
        branch_id: target_branch_id.to_string(),
        revision: Some(target_revision),
        transaction_id: transaction_id.clone(),
        component_id: None,
        content_ref: None,
    };
    Ok(PactFileHead {
        schema: PACT_FILE_HEAD_SCHEMA.to_string(),
        file_path: path.display().to_string(),
        file_kind: "branch".to_string(),
        root_id,
        branch_id: target_branch_id.to_string(),
        current_branch_id: current_branch_id.clone(),
        is_current_branch: target_branch_id == current_branch_id,
        revision: target_revision,
        transaction_id,
        pact_ref,
        branch_count: ledger.branches.len(),
        trace_count: ledger.trace_count(),
        record_count: branch.current.tree.index.by_id.len() + ledger.trace_count() + 3,
        node_count: branch.current.tree.index.by_id.len(),
    })
}

pub fn file_projection_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
) -> PactResult<String> {
    let projection = file_projection(
        path,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
    )?;
    serde_json::to_string_pretty(&projection)
        .map_err(|err| PactError::codec(format!("failed to encode .pact projection: {err}")))
}

pub fn file_projection(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
) -> PactResult<PactFileProjection> {
    file_projection_with_scope(
        path,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
        ProjectionTreeScope::Head,
    )
}

pub fn file_projection_json_with_scope(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
    scope: ProjectionTreeScope,
) -> PactResult<String> {
    let projection = file_projection_with_scope(
        path,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
        scope,
    )?;
    serde_json::to_string_pretty(&projection)
        .map_err(|err| PactError::codec(format!("failed to encode .pact projection: {err}")))
}

pub fn file_projection_with_scope(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
    scope: ProjectionTreeScope,
) -> PactResult<PactFileProjection> {
    let root_store = crate::pact_file::open_ledger(path)?;
    let target_branch = branch_id.unwrap_or(&root_store.current_branch_id);
    let branch_projection = crate::store::ledger_project_branch_with_scope(
        &root_store,
        target_branch,
        revision,
        blob_inline_limit,
        scope,
    )?;
    Ok(PactFileProjection {
        schema: PACT_FILE_PROJECTION_SCHEMA.to_string(),
        branch_id: target_branch.to_string(),
        revision: branch_projection.head_revision,
        scope: scope.as_str().to_string(),
        filters: PactFileProjectionFilters {
            view: view.map(str::to_string),
            component_id: component_id.map(str::to_string),
        },
        branch: branch_projection,
        range: None,
        view_record: None,
        source_branch_id: None,
        projection_branch_id: None,
        projection_revision: None,
    })
}

pub fn parse_pact_range(range_spec: &str) -> PactResult<PactFileRange> {
    let Some((start, end)) = range_spec.split_once("..") else {
        return Err(PactError::invalid(format!(
            "invalid PACT range {range_spec:?}; expected start..end"
        )));
    };
    if start.trim().is_empty() || end.trim().is_empty() {
        return Err(PactError::invalid(format!(
            "open-ended PACT range {range_spec:?} is not supported yet; expected start..end"
        )));
    }
    let start = start
        .trim()
        .parse::<u64>()
        .map_err(|err| PactError::invalid(format!("invalid PACT range start {start:?}: {err}")))?;
    let end = end
        .trim()
        .parse::<u64>()
        .map_err(|err| PactError::invalid(format!("invalid PACT range end {end:?}: {err}")))?;
    if start > end {
        return Err(PactError::invalid(format!(
            "invalid PACT range {range_spec:?}; start must be <= end"
        )));
    }
    Ok(PactFileRange { start, end })
}

#[allow(dead_code)]
pub fn file_project_range_json(
    path: &Path,
    range_spec: &str,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
) -> PactResult<String> {
    file_project_range_json_with_scope(
        path,
        range_spec,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
        ProjectionTreeScope::Head,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn file_project_range(
    path: &Path,
    range_spec: &str,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
) -> PactResult<PactFileProjection> {
    file_project_range_with_scope(
        path,
        range_spec,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
        ProjectionTreeScope::Head,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn file_project_range_with_scope(
    path: &Path,
    range_spec: &str,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
    scope: ProjectionTreeScope,
) -> PactResult<PactFileProjection> {
    let range = parse_pact_range(range_spec)?;
    let mut projection = file_projection_with_scope(
        path,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
        scope,
    )?;
    apply_cycle_range_to_file_projection(&mut projection, &range);
    projection.range = Some(PactFileProjectionRange {
        kind: "cycle".to_string(),
        syntax: range_spec.to_string(),
        start: range.start,
        end: range.end,
    });
    Ok(projection)
}

#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn file_project_range_json_with_scope(
    path: &Path,
    range_spec: &str,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
    scope: ProjectionTreeScope,
) -> PactResult<String> {
    let projection = file_project_range_with_scope(
        path,
        range_spec,
        branch_id,
        revision,
        blob_inline_limit,
        view,
        component_id,
        scope,
    )?;
    serde_json::to_string_pretty(&projection)
        .map_err(|err| PactError::codec(format!("failed to encode ranged .pact projection: {err}")))
}

#[allow(clippy::too_many_arguments)]
pub fn file_stream_json(
    path: &Path,
    mode: &str,
    branch_id: Option<&str>,
    revision: Option<u64>,
    anchor_row_id: Option<&str>,
    anchor_seq: Option<u64>,
    direction: &str,
    limit: usize,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let normalized_mode = normalize_stream_mode(mode)?;
    let direction_key = direction.trim().to_lowercase();
    if normalized_mode == "message_surface" {
        if revision.is_some() {
            return Err(PactError::unsupported(
                "message_surface reads the indexed live head; use a pinned projection for historical revisions",
            ));
        }
        let snapshot = crate::pact_file::snapshot::Snapshot::open(
            path,
            Arc::new(crate::pact_file::backend::IoCounterSet::default()),
        )?;
        let stream = crate::pact_file::message_stream::window(
            &snapshot,
            &path.display().to_string(),
            branch_id.unwrap_or(&snapshot.metadata()?.current_branch_id),
            anchor_row_id,
            anchor_seq,
            &direction_key,
            limit,
            "pact_file_index",
        )?;
        return serde_json::to_string_pretty(&stream)
            .map_err(|err| PactError::codec(format!("failed to encode .pact stream: {err}")));
    }
    if normalized_mode == "current_surface"
        && revision.is_none()
        && matches!(
            direction_key.as_str(),
            "before" | "up" | "after" | "down" | "around"
        )
    {
        let stream = indexed_current_stream(
            path,
            branch_id,
            anchor_row_id,
            anchor_seq,
            &direction_key,
            limit,
        )?;
        return serde_json::to_string_pretty(&stream)
            .map_err(|err| PactError::codec(format!("failed to encode .pact stream: {err}")));
    }
    let head = file_head(path, branch_id, revision)?;
    let projection = file_projection(
        path,
        Some(&head.branch_id),
        Some(head.revision),
        blob_inline_limit,
        None,
        None,
    )?;
    let mut rows = stream_rows_from_file_projection(&projection);
    let source = "pact_file_index";
    let fallback_used = false;

    if normalized_mode == "activity_surface" {
        let ledger = crate::pact_file::open_ledger(path)?;
        let traces = ledger
            .event_index
            .transaction_traces
            .values()
            .filter(|trace| trace.branch_id == head.branch_id)
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| PactError::codec(format!("failed to encode Pact trace: {err}")))?;
        rows.extend(stream_rows_from_activity_entries(
            &activity_entries_from_traces(&traces),
        ));
    }

    let path_display = path.display().to_string();
    let stream = stream_window_value(
        rows,
        normalized_mode,
        source,
        fallback_used,
        head,
        Some(&path_display),
        anchor_row_id,
        anchor_seq,
        direction,
        limit,
    );
    serde_json::to_string_pretty(&stream)
        .map_err(|err| PactError::codec(format!("failed to encode .pact stream: {err}")))
}

fn indexed_current_stream(
    path: &Path,
    branch_id: Option<&str>,
    anchor_row_id: Option<&str>,
    anchor_seq: Option<u64>,
    direction: &str,
    limit: usize,
) -> PactResult<Value> {
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        path,
        Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )?;
    indexed_current_stream_from_snapshot(
        &snapshot,
        &path.display().to_string(),
        branch_id,
        anchor_row_id,
        anchor_seq,
        direction,
        limit,
        "pact_file_index",
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn indexed_current_stream_from_snapshot(
    snapshot: &crate::pact_file::snapshot::Snapshot,
    path_display: &str,
    branch_id: Option<&str>,
    anchor_row_id: Option<&str>,
    anchor_seq: Option<u64>,
    direction: &str,
    limit: usize,
    source: &str,
    include_ephemeral_active: bool,
) -> PactResult<Value> {
    let metadata = snapshot.metadata()?;
    let target_branch_id = branch_id.unwrap_or(&metadata.current_branch_id);
    let revision = snapshot
        .branch_head(target_branch_id)?
        .ok_or_else(|| PactError::not_found(format!("branch not found: {target_branch_id}")))?;
    let node_count = metadata
        .node_counts
        .get(target_branch_id)
        .copied()
        .ok_or_else(|| PactError::corrupt("Pact head metadata is missing branch node count"))?;
    let transaction_id = metadata.head_transaction_ids.get(target_branch_id).cloned();
    let head = PactFileHead {
        schema: PACT_FILE_HEAD_SCHEMA.to_string(),
        file_path: path_display.to_string(),
        file_kind: "branch".to_string(),
        root_id: metadata.root_id.clone(),
        branch_id: target_branch_id.to_string(),
        current_branch_id: metadata.current_branch_id.clone(),
        is_current_branch: target_branch_id == metadata.current_branch_id,
        revision,
        transaction_id: transaction_id.clone(),
        pact_ref: PactRef {
            root_id: metadata.root_id.clone(),
            branch_id: target_branch_id.to_string(),
            revision: Some(revision),
            transaction_id,
            component_id: None,
            content_ref: None,
        },
        branch_count: metadata.branch_count,
        trace_count: metadata.trace_count,
        record_count: node_count + metadata.trace_count + 3,
        node_count,
    };
    let range_start = crate::pact_file::index::IndexKey::MessageOrder {
        branch: target_branch_id.to_string(),
        order: 0,
    };
    let range_end = crate::pact_file::index::IndexKey::MessageOrder {
        branch: target_branch_id.to_string(),
        order: u64::MAX,
    };
    let (indexed_total_rows, _) = snapshot.index_range_window(&range_start, &range_end, 0, 0)?;
    let synthesize_active =
        include_ephemeral_active && snapshot.component(target_branch_id, "active")?.is_none();
    let total_rows = indexed_total_rows + usize::from(synthesize_active);
    let anchor_index = if let Some(row_id) = anchor_row_id {
        let component_id = row_id.strip_prefix("node:").unwrap_or(row_id);
        if synthesize_active && component_id == "active" {
            Some(indexed_total_rows)
        } else {
            snapshot
                .component(target_branch_id, component_id)?
                .and_then(|component| component.order)
                .map(|order| {
                    snapshot
                        .index_range_window(
                            &range_start,
                            &crate::pact_file::index::IndexKey::MessageOrder {
                                branch: target_branch_id.to_string(),
                                order,
                            },
                            0,
                            0,
                        )
                        .map(|(rank, _)| rank)
                })
                .transpose()?
                .filter(|index| *index < total_rows)
        }
    } else {
        None
    }
    .or_else(|| {
        anchor_seq
            .and_then(|seq| usize::try_from(seq).ok())
            .filter(|index| *index < total_rows)
    });
    let limit = limit.clamp(1, 500);
    let (start, end) = match (direction, anchor_index) {
        ("before" | "up", Some(index)) => (index.saturating_sub(limit), index),
        ("after" | "down", Some(index)) => {
            let start = (index + 1).min(total_rows);
            (start, (start + limit).min(total_rows))
        }
        ("around", Some(index)) => {
            let start = index.saturating_sub(limit / 2);
            (start, (start + limit).min(total_rows))
        }
        ("after" | "down", None) => (0, limit.min(total_rows)),
        _ => {
            let start = total_rows.saturating_sub(limit);
            (start, total_rows)
        }
    };
    let indexed_end = end.min(indexed_total_rows);
    let indexed_start = start.min(indexed_end);
    let (_, indexed_rows) = snapshot.index_range_window(
        &range_start,
        &range_end,
        indexed_start,
        indexed_end.saturating_sub(indexed_start),
    )?;
    snapshot.record_rows_read(indexed_rows.len());
    let rows = indexed_rows
        .into_iter()
        .enumerate()
        .map(|(index, (_, component_id))| {
            let seq = start.saturating_add(index);
            let component_id = String::from_utf8(component_id)
                .map_err(|_| PactError::corrupt("Pact message component id is not UTF-8"))?;
            let mut payload = snapshot
                .component(target_branch_id, &component_id)?
                .ok_or_else(|| {
                    PactError::corrupt("Pact message-order index references a missing component")
                })?
                .value;
            if let Some(object) = payload.as_object_mut() {
                object.insert(
                    "pact_depth".to_string(),
                    json!(indexed_total_rows.saturating_sub(seq).saturating_sub(1)),
                );
            }
            let payload = serde_json::from_value::<ComponentRecord>(payload.clone())
                .ok()
                .and_then(|record| serde_json::to_value(record).ok())
                .unwrap_or(payload);
            let mut value = stream_row_from_node_revision_value(
                Value::Number(revision.into()),
                &component_id,
                &payload,
            );
            if let Some(object) = value.as_object_mut() {
                object.insert("seq".to_string(), json!(seq));
                object.insert(
                    "cursor".to_string(),
                    json!({
                        "row_id": object.get("row_id").cloned().unwrap_or(Value::Null),
                        "seq": seq,
                        "revision": revision,
                        "cycle": revision,
                    }),
                );
            }
            Ok(value)
        })
        .collect::<PactResult<Vec<_>>>()?;
    let mut rows = rows;
    if synthesize_active && start <= indexed_total_rows && end > indexed_total_rows {
        let seq = indexed_total_rows;
        let payload = serde_json::to_value(ephemeral_active_component(&metadata.root_id))
            .map_err(|err| PactError::codec(format!("failed to encode active row: {err}")))?;
        let mut value =
            stream_row_from_node_revision_value(Value::Number(revision.into()), "active", &payload);
        if let Some(object) = value.as_object_mut() {
            object.insert("seq".to_string(), json!(seq));
            object.insert(
                "cursor".to_string(),
                json!({
                    "row_id": object.get("row_id").cloned().unwrap_or(Value::Null),
                    "seq": seq,
                    "revision": revision,
                    "cycle": revision,
                }),
            );
        }
        rows.push(value);
    }
    let before_cursor = rows
        .first()
        .and_then(|row| row.get("cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    let after_cursor = rows
        .last()
        .and_then(|row| row.get("cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "schema": "pact.stream.v1",
        "path": path_display,
        "surface": "context",
        "mode": "current_surface",
        "source": source,
        "fallback_used": false,
        "direction": direction,
        "head": head,
        "anchor": {"row_id": anchor_row_id, "seq": anchor_seq},
        "rows": rows,
        "cursors": {"before": before_cursor, "after": after_cursor},
        "position": {
            "start_index": start,
            "end_index": end.saturating_sub(1),
            "total_rows": total_rows,
            "estimated": false,
        },
        "has_before": start > 0,
        "has_after": end < total_rows,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_window_value(
    mut rows: Vec<Value>,
    normalized_mode: &str,
    source: &str,
    fallback_used: bool,
    head: PactFileHead,
    path_display: Option<&str>,
    anchor_row_id: Option<&str>,
    anchor_seq: Option<u64>,
    direction: &str,
    limit: usize,
) -> Value {
    for (seq, row) in rows.iter_mut().enumerate() {
        if let Some(object) = row.as_object_mut() {
            object.insert("seq".to_string(), Value::Number((seq as u64).into()));
            object.insert(
                "cursor".to_string(),
                json!({
                    "row_id": object.get("row_id").cloned().unwrap_or(Value::Null),
                    "seq": seq as u64,
                    "revision": head.revision,
                    "cycle": head.revision,
                }),
            );
        }
    }

    let total_rows = rows.len();
    let limit = limit.clamp(1, 500);
    let anchor_index = anchor_row_id
        .and_then(|id| {
            rows.iter()
                .position(|row| row.get("row_id").and_then(Value::as_str) == Some(id))
        })
        .or_else(|| {
            anchor_seq.and_then(|seq| {
                rows.iter()
                    .position(|row| row.get("seq").and_then(Value::as_u64) == Some(seq))
            })
        });
    let direction = direction.trim().to_lowercase();
    let (start, end) = match (direction.as_str(), anchor_index) {
        ("before" | "up", Some(index)) => {
            let start = index.saturating_sub(limit);
            (start, index)
        }
        ("after" | "down", Some(index)) => {
            let start = (index + 1).min(total_rows);
            (start, (start + limit).min(total_rows))
        }
        ("around", Some(index)) => {
            let before = limit / 2;
            let start = index.saturating_sub(before);
            (start, (start + limit).min(total_rows))
        }
        ("after" | "down", None) => (0, limit.min(total_rows)),
        _ => {
            let start = total_rows.saturating_sub(limit);
            (start, total_rows)
        }
    };
    let window_rows = rows[start..end].to_vec();
    let before_cursor = window_rows
        .first()
        .and_then(|row| row.get("cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    let after_cursor = window_rows
        .last()
        .and_then(|row| row.get("cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut stream = serde_json::Map::new();
    stream.insert("schema".to_string(), json!("pact.stream.v1"));
    if let Some(path) = path_display {
        stream.insert("path".to_string(), json!(path));
    }
    stream.insert("surface".to_string(), json!("context"));
    stream.insert("mode".to_string(), json!(normalized_mode));
    stream.insert("source".to_string(), json!(source));
    stream.insert("fallback_used".to_string(), json!(fallback_used));
    stream.insert("direction".to_string(), json!(direction));
    stream.insert("head".to_string(), json!(head));
    stream.insert(
        "anchor".to_string(),
        json!({
            "row_id": anchor_row_id,
            "seq": anchor_seq,
        }),
    );
    stream.insert("rows".to_string(), Value::Array(window_rows));
    stream.insert(
        "cursors".to_string(),
        json!({
            "before": before_cursor,
            "after": after_cursor,
        }),
    );
    stream.insert(
        "position".to_string(),
        json!({
            "start_index": start,
            "end_index": end.saturating_sub(1),
            "total_rows": total_rows,
            "estimated": false,
        }),
    );
    stream.insert("has_before".to_string(), json!(start > 0));
    stream.insert("has_after".to_string(), json!(end < total_rows));
    Value::Object(stream)
}

pub(crate) fn normalize_stream_mode(mode: &str) -> PactResult<&'static str> {
    match mode.trim() {
        "" | "current" | "current_surface" | "current-surface" | "context_head"
        | "context-head" | "head" => Ok("current_surface"),
        "message" | "messages" | "message_surface" | "message-surface" | "chat"
        | "chat_surface" | "chat-surface" => Ok("message_surface"),
        "activity" | "activity_surface" | "activity-surface" | "timeline" | "live"
        | "live_surface" | "live-surface" => Ok("activity_surface"),
        other => Err(PactError::invalid(format!(
            "unsupported stream mode {other:?}; expected current_surface, message_surface, activity_surface, or live_surface"
        ))),
    }
}

fn stream_rows_from_file_projection(projection: &PactFileProjection) -> Vec<Value> {
    stream_rows_from_pact_view(&projection.branch, projection.revision)
}

pub(crate) fn stream_rows_from_pact_view(view: &PactView, revision: u64) -> Vec<Value> {
    stream_rows_from_pact_view_with_active(view, revision, false)
}

pub(crate) fn stream_rows_from_held_pact_view(view: &PactView, revision: u64) -> Vec<Value> {
    stream_rows_from_pact_view_with_active(view, revision, true)
}

fn stream_rows_from_pact_view_with_active(
    view: &PactView,
    revision: u64,
    include_ephemeral_active: bool,
) -> Vec<Value> {
    let mut nodes = view.component_records();
    if include_ephemeral_active && !nodes.contains_key("active") {
        nodes.insert(
            "active".to_string(),
            ephemeral_active_component(&view.tree.root_id),
        );
    }
    let mut ordered_ids = Vec::new();
    let mut seen = BTreeSet::new();
    for component_id in &view.message_order {
        if nodes
            .get(component_id)
            .map(stream_node_record_is_message_turn)
            .unwrap_or(false)
            && seen.insert(component_id.clone())
        {
            ordered_ids.push(component_id.clone());
        }
    }
    let mut missing = nodes
        .iter()
        .filter(|(component_id, node)| {
            !seen.contains(*component_id) && stream_node_record_is_message_turn(node)
        })
        .map(|(component_id, node)| {
            (
                stream_node_record_order_key(component_id, node),
                component_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    missing.sort_by(|left, right| left.0.cmp(&right.0));
    ordered_ids.extend(missing.into_iter().map(|(_, component_id)| component_id));

    ordered_ids
        .into_iter()
        .filter_map(|component_id| {
            let node = nodes.get(&component_id)?;
            Some(stream_row_from_node_record(revision, &component_id, node))
        })
        .collect()
}

fn ephemeral_active_component(root_id: &str) -> ComponentRecord {
    let mut active = Component::message_turn_authoring(
        Some("user".to_string()),
        None,
        crate::components::AuthoringComponentOptions {
            local_id: Some("active".to_string()),
            created_at_ns: Some(0),
            ..Default::default()
        },
    );
    active.id = "active".to_string();
    active.parent_id = Some(root_id.to_string());
    active.pact_depth = Some(1);
    active.offset = 0;
    ComponentRecord::from_component_with_parent(&active, active.parent_id.clone())
}

fn stream_row_from_node_record(revision: u64, component_id: &str, node: &ComponentRecord) -> Value {
    let payload = serde_json::to_value(node).unwrap_or(Value::Null);
    stream_row_from_node_revision_value(Value::Number(revision.into()), component_id, &payload)
}

fn stream_row_from_node_revision_value(revision: Value, component_id: &str, node: &Value) -> Value {
    json!({
        "row_id": format!("node:{component_id}"),
        "kind": "node",
        "component_id": component_id,
        "node_type": node.get("node_type").or_else(|| node.get("componentType")).cloned().unwrap_or(Value::Null),
        "role": node.get("role").cloned().unwrap_or(Value::Null),
        "cycle": node_cycle(node),
        "revision": revision,
        "payload": node,
    })
}

fn stream_node_is_message_turn(node: &Value) -> bool {
    node.get("node_type")
        .or_else(|| node.get("componentType"))
        .and_then(Value::as_str)
        == Some("message_turn")
}

fn stream_node_record_is_message_turn(node: &ComponentRecord) -> bool {
    node.node_type == "message_turn"
}

fn stream_node_record_order_key(
    component_id: &str,
    node: &ComponentRecord,
) -> (i64, i64, u64, u64, String) {
    let offset = node.offset;
    let depth = node.pact_depth.unwrap_or(offset);
    let depth_rank = if depth < 0 {
        -1
    } else if depth == 0 {
        i64::MAX
    } else {
        i64::MAX - depth
    };
    (
        depth_rank,
        offset,
        node.created_at_ns,
        node.creation_index,
        component_id.to_string(),
    )
}

pub fn file_timeline_json(path: &Path) -> PactResult<String> {
    let session = scan::PactReadSession::open(path)?;
    if !session.errors().is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot read timeline from invalid .pact file: {}",
            session.errors().join("; ")
        )));
    }
    let traces = query_records(session.records(), "trace", None)?.matches;
    let mut rows = stream_rows_from_activity_entries(&activity_entries_from_traces(&traces));
    for (seq, row) in rows.iter_mut().enumerate() {
        if let Some(object) = row.as_object_mut() {
            object.insert("seq".to_string(), Value::Number((seq as u64).into()));
        }
    }
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.timeline.v1"),
        "path": path.display().to_string(),
        "row_count": rows.len(),
        "rows": rows,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact timeline: {err}")))
}

pub(crate) fn stream_rows_from_activity_entries(entries: &[Value]) -> Vec<Value> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let transaction_id = entry
                .get("transaction_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let event_index = entry
                .get("event_index")
                .and_then(Value::as_u64)
                .unwrap_or(index as u64);
            json!({
                "row_id": format!("activity:{transaction_id}:{event_index}"),
                "kind": "activity",
                "transaction_id": transaction_id,
                "payload": entry,
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PactForkMode {
    Hard,
    Soft,

    Delta { base_pinned: bool },
}

pub fn file_fork_json(
    source_path: &Path,
    output_path: Option<&Path>,
    mode: PactForkMode,
    branch_id: Option<&str>,
    revision: Option<u64>,
    parent_locator_hints_json: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let source_ledger = file_open_ledger(source_path)?;
    let source_branch_id = branch_id
        .map(str::to_string)
        .unwrap_or_else(|| source_ledger.current_branch_id.clone());
    let source_branch = source_ledger
        .branches
        .get(&source_branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {source_branch_id}")))?
        .clone();
    let fork_revision = revision.unwrap_or(source_branch.head_revision);
    if fork_revision > source_branch.head_revision {
        return Err(PactError::invalid(format!(
            "requested revision {fork_revision} is after branch {source_branch_id} head {}",
            source_branch.head_revision
        )));
    }

    let parent_ref = PactRef {
        root_id: source_ledger.root_id.clone(),
        branch_id: source_branch_id.clone(),
        revision: Some(fork_revision),
        transaction_id: None,
        component_id: None,
        content_ref: None,
    };

    match mode {
        PactForkMode::Soft => {
            let view_ref = match revision {
                Some(rev) => {
                    crate::view::open_read_only_pinned(&source_ledger, &source_branch_id, Some(rev))
                }
                None => crate::view::open_read_only_live(&source_ledger, &source_branch_id),
            }
            .map_err(|err| PactError::storage(format!("failed to open soft fork view: {err}")))?;
            serde_json::to_string_pretty(&json!({
                "schema": format!("{PACT_FILE_SCHEMA}.fork.soft.v1"),
                "mode": "soft",
                "source_path": source_path.display().to_string(),
                "source_branch_id": source_branch_id,
                "revision": fork_revision,
                "parent_ref": parent_ref,
                "view_ref": view_ref,
            }))
            .map_err(|err| PactError::codec(format!("failed to encode soft fork report: {err}")))
        }
        PactForkMode::Hard => {
            let output_path = output_path.ok_or_else(|| {
                PactError::invalid("hard fork requires an output path for the child .pact")
            })?;

            let locator_hints = parse_parent_locator_hints(parent_locator_hints_json)?;

            let child_root_id =
                mint_child_root_id(&source_ledger.root_id, &source_branch_id, fork_revision);
            let mut fork_tree = crate::store::project_branch_component_tree(
                &source_ledger,
                &source_branch,
                Some(fork_revision),
            )?;
            let new_root_id = child_root_id.clone();
            fork_tree.edit(|root| {
                root.id = new_root_id.clone();
                Ok(())
            })?;

            let mut child_tree_value = fork_tree.project_value();
            crate::payload_hydration::hydrate_payload_refs(
                &mut child_tree_value,
                &source_ledger.content,
            )?;
            let child_tree_json = serde_json::to_string(&child_tree_value)
                .map_err(|err| PactError::codec(format!("failed to encode forked tree: {err}")))?;
            let child_ledger_json = crate::store::new_ledger_json(
                &child_tree_json,
                Some(&source_branch_id),
                blob_inline_limit,
            )?;
            let mut child_ledger = decode_context_tree_ledger(&child_ledger_json)?;
            if child_ledger.root_id != child_root_id {
                return Err(PactError::internal(format!(
                    "child ledger root_id {} did not match minted child root id {child_root_id}",
                    child_ledger.root_id
                )));
            }
            child_ledger.parent = Some(crate::store::PactParentProvenance {
                parent_ref: parent_ref.clone(),
                locator_hints,
                aux: BTreeMap::new(),
            });

            let child_head_revision = child_ledger
                .branches
                .get(&source_branch_id)
                .map(|branch| branch.head_revision);
            let child_message_count = child_ledger
                .branches
                .get(&source_branch_id)
                .map(|branch| branch.current.message_order.len());

            let child_ledger_json = serde_json::to_string(&child_ledger)
                .map_err(|err| PactError::codec(format!("failed to encode child ledger: {err}")))?;
            let manifest_json =
                file_create_from_ledger_json(output_path, &child_ledger_json, blob_inline_limit)?;
            let manifest: Value = serde_json::from_str(&manifest_json).map_err(|err| {
                PactError::codec(format!("failed to decode child manifest: {err}"))
            })?;

            serde_json::to_string_pretty(&json!({
                "schema": format!("{PACT_FILE_SCHEMA}.fork.hard.v1"),
                "mode": "hard",
                "source_path": source_path.display().to_string(),
                "child_path": output_path.display().to_string(),
                "source_branch_id": source_branch_id,
                "revision": fork_revision,
                "parent_ref": parent_ref,
                "child_root_id": child_root_id,
                "child_branch_id": source_branch_id,
                "child_head": {
                    "branch_id": source_branch_id,
                    "head_revision": child_head_revision,
                    "message_count": child_message_count,
                },
                "manifest": manifest,
            }))
            .map_err(|err| PactError::codec(format!("failed to encode hard fork report: {err}")))
        }
        PactForkMode::Delta { base_pinned } => {
            let output_path = output_path.ok_or_else(|| {
                PactError::invalid("delta overlay fork requires an output path for the child .pact")
            })?;
            let mut locator_hints = parse_parent_locator_hints(parent_locator_hints_json)?;

            locator_hints.insert(
                0,
                crate::store::PactParentLocator {
                    kind: "path".to_string(),
                    value: source_path.display().to_string(),
                },
            );
            let base_mode = if base_pinned {
                crate::store::PactBaseMode::Pinned
            } else {
                crate::store::PactBaseMode::Following
            };
            delta::write_delta_overlay(
                source_path,
                output_path,
                base_mode,
                Some(&source_branch_id),
                Some(fork_revision),
                locator_hints,
                blob_inline_limit,
            )
        }
    }
}

fn mint_child_root_id(source_root_id: &str, branch_id: &str, revision: u64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or(0);
    let mut hasher = blake3::Hasher::new();
    hasher.update(source_root_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(branch_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(&revision.to_le_bytes());
    hasher.update(&nanos.to_le_bytes());
    let digest = hasher.finalize();
    let suffix: String = digest.as_bytes()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("root_fork_{suffix}")
}

fn parse_parent_locator_hints(
    hints_json: Option<&str>,
) -> PactResult<Vec<crate::store::PactParentLocator>> {
    let Some(hints_json) = hints_json else {
        return Ok(Vec::new());
    };
    let trimmed = hints_json.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<crate::store::PactParentLocator>>(trimmed)
        .map_err(|err| PactError::invalid(format!("invalid parent locator hints JSON: {err}")))
}

pub fn file_create_branch_at_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    new_branch_id: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let root_store = file_open_ledger(path)?;
    let source_branch_id = branch_id
        .map(str::to_string)
        .unwrap_or_else(|| root_store.current_branch_id.clone());
    let source_branch = root_store
        .branches
        .get(&source_branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {source_branch_id}")))?
        .clone();
    let target_revision = revision.unwrap_or(source_branch.head_revision);
    if target_revision > source_branch.head_revision {
        return Err(PactError::invalid(format!(
            "requested revision {target_revision} is after branch {source_branch_id} head {}",
            source_branch.head_revision
        )));
    }
    let fork_branch_id = new_branch_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("{source_branch_id}_fork_{target_revision}"));
    if root_store.branches.contains_key(&fork_branch_id) {
        return Err(PactError::invalid(format!(
            "branch already exists: {fork_branch_id}"
        )));
    }
    let fork_ref = PactRef {
        root_id: root_store.root_id.clone(),
        branch_id: source_branch_id.clone(),
        revision: Some(target_revision),
        transaction_id: None,
        component_id: None,
        content_ref: None,
    };
    let (forked_store, _) = root_store.fork_at_revision(
        &source_branch_id,
        Some(target_revision),
        Some(fork_branch_id.clone()),
        BTreeMap::new(),
        blob_inline_limit,
    )?;
    let forked_json = serde_json::to_string(&forked_store)
        .map_err(|err| PactError::codec(format!("failed to encode forked root store: {err}")))?;
    let manifest_json = file_create_from_ledger_json(path, &forked_json, blob_inline_limit)?;
    let manifest: Value = serde_json::from_str(&manifest_json)
        .map_err(|err| PactError::codec(format!("failed to decode fork manifest: {err}")))?;
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.fork_at.v1"),
        "path": path.display().to_string(),
        "source_branch_id": source_branch_id,
        "branch_id": fork_branch_id,
        "revision": target_revision,
        "fork_ref": fork_ref,
        "local_head_revision": forked_store.branches
            .get(&fork_branch_id)
            .map(|branch| branch.head_revision),
        "manifest": manifest,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode fork report: {err}")))
}

pub fn file_rewind_to_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: u64,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let mut root_store = file_open_ledger(path)?;
    let target_branch_id = branch_id
        .map(str::to_string)
        .unwrap_or_else(|| root_store.current_branch_id.clone());
    let branch = root_store
        .branches
        .get(&target_branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {target_branch_id}")))?
        .clone();
    if revision > branch.head_revision {
        return Err(PactError::invalid(format!(
            "requested revision {revision} is after branch {target_branch_id} head {}",
            branch.head_revision
        )));
    }
    let projected_view = root_store.project_branch_with_scope(
        &target_branch_id,
        Some(revision),
        blob_inline_limit,
        ProjectionTreeScope::Head,
    )?;
    let mut rewound = branch;
    rewound.head_revision = revision;
    rewound.current = projected_view;
    root_store
        .branches
        .insert(target_branch_id.clone(), rewound);
    root_store.current_branch_id = target_branch_id.clone();
    let root_store_json = serde_json::to_string(&root_store)
        .map_err(|err| PactError::codec(format!("failed to encode rewound root store: {err}")))?;
    let manifest_json = file_create_from_ledger_json(path, &root_store_json, blob_inline_limit)?;
    let manifest: Value = serde_json::from_str(&manifest_json)
        .map_err(|err| PactError::codec(format!("failed to decode rewind manifest: {err}")))?;
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.rewind_to.v1"),
        "path": path.display().to_string(),
        "branch_id": target_branch_id,
        "revision": revision,
        "manifest": manifest,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode rewind report: {err}")))
}

pub(crate) fn activity_entries_from_traces(traces: &[Value]) -> Vec<Value> {
    let mut entries = Vec::new();
    for trace in traces {
        let trace_id = trace
            .get("transaction_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let branch_id = trace
            .get("branch_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(events) = trace
            .get("aux")
            .and_then(|aux| aux.get("events"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for (index, event) in events.iter().enumerate() {
            entries.push(json!({
                "transaction_id": trace_id,
                "branch_id": branch_id,
                "event_index": index,
                "event": event,
            }));
        }
    }
    entries
}

fn apply_cycle_range_to_file_projection(
    projection: &mut PactFileProjection,
    range: &PactFileRange,
) {
    let mut keep_ids = BTreeSet::new();
    for (component_id, node) in projection.branch.component_records() {
        let keep = node_record_cycle(&node)
            .map(|cycle| range.contains(cycle))
            .unwrap_or(true);
        if keep {
            keep_ids.insert(component_id.clone());
        }
    }
    projection
        .branch
        .message_order
        .retain(|component_id| keep_ids.contains(component_id));
}

fn node_record_cycle(node: &ComponentRecord) -> Option<u64> {
    node.aux.get("cycle").and_then(Value::as_u64)
}

pub fn file_view_projection_json(
    path: &Path,
    view_id: &str,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
) -> PactResult<String> {
    let projection = file_view_projection(path, view_id, blob_inline_limit, view, component_id)?;
    serde_json::to_string_pretty(&projection)
        .map_err(|err| PactError::codec(format!("failed to encode .pact view projection: {err}")))
}

pub fn file_view_projection(
    path: &Path,
    view_id: &str,
    blob_inline_limit: usize,
    view: Option<&str>,
    component_id: Option<&str>,
) -> PactResult<PactFileProjection> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let bytes = if crate::pact_pack::is_packed_v1(&raw) {
        packed_v1_to_v0_bytes(&raw)?
    } else {
        raw
    };
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot project invalid .pact file view: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let view_record = find_view(&views, view_id)?;
    let target_branch = view_projection_branch(view_record);
    let target_revision = view_projection_revision(view_record);
    if !root_store.branches.contains_key(target_branch) {
        return Err(PactError::not_found(format!(
            "view {} targets missing branch {}",
            view_record.view_id, target_branch
        )));
    }
    let mut projection = file_projection(
        path,
        Some(target_branch),
        target_revision,
        blob_inline_limit,
        view,
        component_id,
    )?;
    projection.view_record = Some(view_record.clone());
    projection.source_branch_id = Some(view_record.source_branch_id.clone());
    projection.projection_branch_id = Some(target_branch.to_string());
    projection.projection_revision = Some(target_revision);
    Ok(projection)
}

pub fn file_view_query_json(
    path: &Path,
    view_id: &str,
    query: &str,
    key: Option<&str>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let projection = file_view_projection(path, view_id, blob_inline_limit, None, None)?;
    let projection = serde_json::to_value(projection)
        .map_err(|err| PactError::codec(format!("failed to encode view projection: {err}")))?;
    let matches = match query {
        "projection" => vec![projection],
        "messages" | "message-order" => projection
            .get("branch")
            .and_then(|branch| branch.get("message_order"))
            .cloned()
            .into_iter()
            .collect(),
        "component" | "components" => {
            filter_projection_array(projection.get("components"), key, "component_id")
        }
        "component-transaction" | "component-transactions" | "transaction" | "transactions" => {
            filter_projection_array_any(
                projection.get("component_transactions"),
                key,
                &["transaction_id", "component_id"],
            )
        }
        "view" => projection.get("view_record").cloned().into_iter().collect(),
        "refs" => {
            let mut refs = Vec::new();
            if let Some(view_record) = projection.get("view_record") {
                for field in ["touched_node_refs", "external_refs"] {
                    if let Some(items) = view_record.get(field).and_then(Value::as_array) {
                        refs.extend(items.iter().cloned());
                    }
                }
            }
            refs
        }
        other => {
            return Err(PactError::invalid(format!(
                "unsupported .pact view query: {other}"
            )));
        }
    };
    let result = PactFileQueryResult {
        schema: format!("{PACT_FILE_SCHEMA}.view.query"),
        query: query.to_string(),
        key: key.map(str::to_string),
        matches,
    };
    serde_json::to_string_pretty(&result)
        .map_err(|err| PactError::codec(format!("failed to encode .pact view query result: {err}")))
}

pub fn file_view_merge_preflight_json(path: &Path, view_id: &str) -> PactResult<String> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let bytes = if crate::pact_pack::is_packed_v1(&raw) {
        packed_v1_to_v0_bytes(&raw)?
    } else {
        raw
    };
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot inspect invalid .pact file view: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let view_record = find_view(&views, view_id)?;
    let source_head_revision = root_store
        .branches
        .get(&view_record.source_branch_id)
        .map(|branch| branch.head_revision);
    let view_head_revision = view_record
        .view_branch_id
        .as_deref()
        .and_then(|branch_id| root_store.branches.get(branch_id))
        .map(|branch| branch.head_revision);
    let changed_component_ids = collect_view_changed_components(view_record);
    let conflicts = collect_view_conflicts(view_record, source_head_revision);
    let report = PactViewMergePreflight {
        schema: format!("{PACT_FILE_SCHEMA}.view.merge_preflight"),
        view_id: view_record.view_id.clone(),
        mode: view_record.mode.clone(),
        source_branch_id: view_record.source_branch_id.clone(),
        view_branch_id: view_record.view_branch_id.clone(),
        source_head_revision,
        view_head_revision,
        mount_revision: view_record.source_revision,
        changed_component_ids,
        touched_node_refs: view_record.touched_node_refs.clone(),
        external_refs: view_record.external_refs.clone(),
        merge_ready: conflicts.is_empty() && view_record.view_branch_id.is_some(),
        conflicts,
    };
    serde_json::to_string_pretty(&report).map_err(|err| {
        PactError::codec(format!(
            "failed to encode .pact view merge preflight: {err}"
        ))
    })
}

pub fn file_view_merge_json(
    path: &Path,
    view_id: &str,
    policy: Option<&str>,
) -> PactResult<String> {
    let policy = policy.unwrap_or("replace-component");
    if !matches!(policy, "replace-component" | "manual-resolution") {
        return Err(PactError::invalid(format!(
            "unsupported .pact view merge policy: {policy}"
        )));
    }
    let bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot merge invalid .pact file view: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let view_record = find_view(&views, view_id)?.clone();
    let Some(view_branch_id) = view_record.view_branch_id.clone() else {
        return Err(PactError::invalid(format!(
            "view {} has no view_branch_id to merge",
            view_record.view_id
        )));
    };
    let preflight_json = file_view_merge_preflight_json(path, view_id)?;
    let preflight: PactViewMergePreflight = serde_json::from_str(&preflight_json)
        .map_err(|err| PactError::codec(format!("failed to decode view merge preflight: {err}")))?;
    if !preflight.conflicts.is_empty() && policy != "manual-resolution" {
        let report = PactViewMergeReport {
            schema: format!("{PACT_FILE_SCHEMA}.view.merge"),
            view_id: view_record.view_id,
            policy: policy.to_string(),
            source_branch_id: view_record.source_branch_id,
            view_branch_id,
            merged_component_ids: Vec::new(),
            conflicts: preflight.conflicts,
            applied: false,
        };
        return serde_json::to_string_pretty(&report).map_err(|err| {
            PactError::codec(format!("failed to encode .pact view merge report: {err}"))
        });
    }
    if policy == "manual-resolution" {
        let report = PactViewMergeReport {
            schema: format!("{PACT_FILE_SCHEMA}.view.merge"),
            view_id: view_record.view_id,
            policy: policy.to_string(),
            source_branch_id: view_record.source_branch_id,
            view_branch_id,
            merged_component_ids: Vec::new(),
            conflicts: preflight.conflicts,
            applied: false,
        };
        return serde_json::to_string_pretty(&report).map_err(|err| {
            PactError::codec(format!("failed to encode .pact view merge report: {err}"))
        });
    }

    let source_branch_id = view_record.source_branch_id.clone();
    let merged_component_ids = collect_view_changed_components(&view_record);
    let file = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    let report = PactViewMergeReport {
        schema: format!("{PACT_FILE_SCHEMA}.view.merge"),
        view_id: view_record.view_id,
        policy: policy.to_string(),
        source_branch_id,
        view_branch_id,
        merged_component_ids,
        conflicts: Vec::new(),
        applied: true,
    };
    serde_json::to_string_pretty(&report)
        .map_err(|err| PactError::codec(format!("failed to encode .pact view merge report: {err}")))
}

pub fn file_view_detach_json(
    path: &Path,
    view_id: &str,
    detached_branch_id: &str,
) -> PactResult<String> {
    if detached_branch_id.trim().is_empty() {
        return Err(PactError::invalid(
            "detached view import requires a branch id",
        ));
    }
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&raw) {
        let mut ledger = crate::pact_file::open_ledger(path)?;
        let views = v2_view_records(&ledger)?;
        let view_record = find_view(&views, view_id)?.clone();
        let source_view_branch = view_projection_branch(&view_record).to_string();
        if ledger.branches.contains_key(detached_branch_id) {
            return Err(PactError::invalid(format!(
                "detached branch already exists: {detached_branch_id}"
            )));
        }
        let mut branch = ledger
            .branches
            .get(&source_view_branch)
            .ok_or_else(|| {
                PactError::not_found(format!("view branch not found: {source_view_branch}"))
            })?
            .clone();
        branch.branch_id = detached_branch_id.to_string();
        branch.parent_branch_id = None;
        branch.fork_ref = None;
        branch.current.branch_id = detached_branch_id.to_string();
        branch.aux.insert(
            "detached_from_view".to_string(),
            json!({
                "view_id": view_record.view_id,
                "source_branch_id": view_record.source_branch_id,
                "view_branch_id": source_view_branch,
            }),
        );
        ledger
            .branches
            .insert(detached_branch_id.to_string(), branch);
        write_v2_ledger(path, &ledger, PublishMode::ReplaceExisting)?;
        return file_inspect_json(path);
    }
    let bytes = raw;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot detach invalid .pact file view: {}",
            validation.errors.join("; ")
        )));
    }
    let mut root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let view_record = find_view(&views, view_id)?.clone();
    let source_view_branch = view_projection_branch(&view_record).to_string();
    if root_store.branches.contains_key(detached_branch_id) {
        return Err(PactError::invalid(format!(
            "detached branch already exists: {detached_branch_id}"
        )));
    }
    let mut branch = root_store
        .branches
        .get(&source_view_branch)
        .ok_or_else(|| {
            PactError::not_found(format!("view branch not found: {source_view_branch}"))
        })?
        .clone();
    branch.branch_id = detached_branch_id.to_string();
    branch.parent_branch_id = None;
    branch.fork_ref = None;
    branch.current.branch_id = detached_branch_id.to_string();
    branch.aux.insert(
        "detached_from_view".to_string(),
        json!({
            "view_id": view_record.view_id,
            "source_branch_id": view_record.source_branch_id,
            "view_branch_id": source_view_branch,
        }),
    );
    root_store
        .branches
        .insert(detached_branch_id.to_string(), branch);
    let file = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    file_inspect_json(path)
}

pub fn file_view_rebase_json(
    path: &Path,
    view_id: &str,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&bytes) {
        let mut ledger = crate::pact_file::open_ledger(path)?;
        let views = v2_view_records(&ledger)?;
        let view_record = find_view(&views, view_id)?.clone();
        if view_record.mode != "mutable_live" {
            return Err(PactError::invalid(format!(
                "view {} is mode {}; only mutable_live views can rebase",
                view_record.view_id, view_record.mode
            )));
        }
        let view_branch_id = view_record.view_branch_id.clone().ok_or_else(|| {
            PactError::invalid(format!(
                "view {} has no view_branch_id to rebase",
                view_record.view_id
            ))
        })?;
        let source_branch = ledger
            .branches
            .get(&view_record.source_branch_id)
            .ok_or_else(|| {
                PactError::not_found(format!(
                    "source branch not found: {}",
                    view_record.source_branch_id
                ))
            })?
            .clone();
        let old_view_branch = ledger
            .branches
            .get(&view_branch_id)
            .ok_or_else(|| {
                PactError::not_found(format!("view branch not found: {view_branch_id}"))
            })?
            .clone();
        let previous_parent_revision = old_view_branch
            .fork_ref
            .as_ref()
            .and_then(|pact_ref| pact_ref.revision);
        let parent_revision = source_branch.head_revision;
        let applied = previous_parent_revision != Some(parent_revision);
        if applied {
            let aux = old_view_branch.aux;
            ledger.branches.remove(&view_branch_id);
            ledger.current_branch_id = view_record.source_branch_id.clone();
            let (mut rebased, _) = ledger.fork_at_revision(
                &view_record.source_branch_id,
                Some(parent_revision),
                Some(view_branch_id.clone()),
                aux,
                blob_inline_limit,
            )?;
            set_v2_view_records(&mut rebased, &views)?;
            write_v2_ledger(path, &rebased, PublishMode::ReplaceExisting)?;
        }
        let report = PactViewRebaseReport {
            schema: format!("{PACT_FILE_SCHEMA}.view.rebase"),
            view_id: view_record.view_id,
            source_branch_id: view_record.source_branch_id,
            view_branch_id,
            previous_parent_revision,
            parent_revision,
            applied,
        };
        return serde_json::to_string_pretty(&report).map_err(|err| {
            PactError::codec(format!("failed to encode .pact view rebase report: {err}"))
        });
    }
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot rebase invalid .pact file view: {}",
            validation.errors.join("; ")
        )));
    }
    let mut root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let view_record = find_view(&views, view_id)?.clone();
    if view_record.mode != "mutable_live" {
        return Err(PactError::invalid(format!(
            "view {} is mode {}; only mutable_live views can rebase",
            view_record.view_id, view_record.mode
        )));
    }
    let Some(view_branch_id) = view_record.view_branch_id.clone() else {
        return Err(PactError::invalid(format!(
            "view {} has no view_branch_id to rebase",
            view_record.view_id
        )));
    };
    let source_branch = root_store
        .branches
        .get(&view_record.source_branch_id)
        .ok_or_else(|| {
            PactError::not_found(format!(
                "source branch not found: {}",
                view_record.source_branch_id
            ))
        })?
        .clone();
    let old_view_branch = root_store
        .branches
        .get(&view_branch_id)
        .ok_or_else(|| PactError::not_found(format!("view branch not found: {view_branch_id}")))?
        .clone();
    let previous_parent_revision = old_view_branch
        .fork_ref
        .as_ref()
        .and_then(|pact_ref| pact_ref.revision);
    let parent_revision = source_branch.head_revision;
    if previous_parent_revision == Some(parent_revision) {
        let report = PactViewRebaseReport {
            schema: format!("{PACT_FILE_SCHEMA}.view.rebase"),
            view_id: view_record.view_id,
            source_branch_id: view_record.source_branch_id,
            view_branch_id,
            previous_parent_revision,
            parent_revision,
            applied: false,
        };
        return serde_json::to_string_pretty(&report).map_err(|err| {
            PactError::codec(format!("failed to encode .pact view rebase report: {err}"))
        });
    }

    let aux = old_view_branch.aux.clone();
    root_store.branches.remove(&view_branch_id);
    root_store.current_branch_id = view_record.source_branch_id.clone();
    let (root_store, _) = root_store.fork_at_revision(
        &view_record.source_branch_id,
        Some(parent_revision),
        Some(view_branch_id.clone()),
        aux,
        blob_inline_limit,
    )?;
    let file = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    let report = PactViewRebaseReport {
        schema: format!("{PACT_FILE_SCHEMA}.view.rebase"),
        view_id: view_record.view_id,
        source_branch_id: view_record.source_branch_id,
        view_branch_id,
        previous_parent_revision,
        parent_revision,
        applied: true,
    };
    serde_json::to_string_pretty(&report).map_err(|err| {
        PactError::codec(format!("failed to encode .pact view rebase report: {err}"))
    })
}

pub fn file_ledger_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    let validation = validation_from_scan_ref(&scanned);
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot reconstruct invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    serde_json::to_string(&root_store).map_err(|err| {
        PactError::codec(format!("failed to encode reconstructed root store: {err}"))
    })
}

fn file_is_packed_v1(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && crate::pact_pack::is_packed_v1(&magic)
}

pub fn file_query_json(path: &Path, query: &str, key: Option<&str>) -> PactResult<String> {
    if file_is_packed_v1(path) {
        let raw = fs::read(path)
            .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
        let bytes = packed_v1_to_v0_bytes(&raw)?;
        let scanned = scan_file(&bytes, true);
        if !scanned.errors.is_empty() {
            return Err(PactError::corrupt(format!(
                "cannot query invalid .pact file: {}",
                scanned.errors.join("; ")
            )));
        }
        let result = query_records(&scanned.records, query, key)?;
        return serde_json::to_string_pretty(&result).map_err(|err| {
            PactError::codec(format!("failed to encode .pact query result: {err}"))
        });
    }

    if query != "component-payload"
        && let Some(result) = try_fast_query_json(path, query, key)?
    {
        return serde_json::to_string_pretty(&result).map_err(|err| {
            PactError::codec(format!("failed to encode .pact query result: {err}"))
        });
    }
    let scanned = scan_file_cached(path, true)?;
    if !scanned.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot query invalid .pact file: {}",
            scanned.errors.join("; ")
        )));
    }
    let result = if query == "component-payload" {
        query_component_payload_records(&scanned.records, key)?
    } else {
        query_records(&scanned.records, query, key)?
    };
    serde_json::to_string_pretty(&result)
        .map_err(|err| PactError::codec(format!("failed to encode .pact query result: {err}")))
}

pub(crate) fn repair_file(
    source_path: &Path,
    repaired_path: &Path,
    truncate_corrupt_tail: bool,
) -> PactResult<PactFileRepairReport> {
    let parent = repaired_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let candidate = tempfile::Builder::new()
        .prefix(".pactree-repair-")
        .tempfile_in(parent)
        .map_err(PactError::Io)?;
    let candidate_path = candidate.path().to_path_buf();
    candidate.close().map_err(PactError::Io)?;
    let result = (|| {
        let mut report =
            build_repair_candidate(source_path, &candidate_path, truncate_corrupt_tail)?;
        if !report.repaired_validation.ok {
            return Err(PactError::corrupt(format!(
                "repaired Pact file failed validation: {}",
                report.repaired_validation.errors.join("; ")
            )));
        }
        file_open_ledger(&candidate_path)?;
        let candidate_bytes = fs::read(&candidate_path).map_err(|error| {
            PactError::storage(format!("failed to read repaired candidate: {error}"))
        })?;
        publish_bytes(repaired_path, &candidate_bytes, PublishMode::CreateNew)?;
        invalidate_scan_cache(repaired_path);
        report.repaired_path = repaired_path.display().to_string();
        Ok(report)
    })();
    invalidate_scan_cache(&candidate_path);
    fs::remove_file(&candidate_path).ok();
    result
}

fn build_repair_candidate(
    source_path: &Path,
    repaired_path: &Path,
    truncate_corrupt_tail: bool,
) -> PactResult<PactFileRepairReport> {
    let bytes = fs::read(source_path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&bytes) {
        let committed = crate::pact_file::committed_prefix(&bytes)?;
        if committed.len() != bytes.len() && !truncate_corrupt_tail {
            return Err(PactError::invalid(
                "repair requires explicit corrupt-tail truncation",
            ));
        }
        let source_ledger = crate::pact_file::open_ledger(source_path)?;
        let mut repaired_ledger = source_ledger.clone();
        if let Some(branch) = repaired_ledger
            .branches
            .get_mut(&repaired_ledger.current_branch_id)
        {
            branch
                .aux
                .insert("pact_file_healed".to_string(), Value::Bool(true));
        }
        let repaired_bytes = crate::pact_file::encode_canonical_file(&repaired_ledger)?;
        publish_bytes(repaired_path, &repaired_bytes, PublishMode::CreateNew)?;
        let repaired_validation = v2_repair_validation(
            &repaired_ledger,
            repaired_bytes.len(),
            repaired_bytes.len(),
            true,
            Vec::new(),
        );
        let had_tail = committed.len() != bytes.len();
        let source_validation = v2_repair_validation(
            &source_ledger,
            bytes.len(),
            committed.len(),
            !had_tail,
            had_tail
                .then(|| "uncommitted bytes follow the latest valid root slot".to_string())
                .into_iter()
                .collect(),
        );
        return Ok(PactFileRepairReport {
            schema: format!("{PACT_FILE_SCHEMA}.repair"),
            repaired_path: repaired_path.display().to_string(),
            source_validation,
            repaired_validation,
            truncated_tail: had_tail,
            rebuilt_indexes: true,
        });
    }
    let source_validation = validation_from_scan(scan_file(&bytes, true));
    if source_validation.ok {
        let scanned = scan_file(&bytes, true);
        match ledger_from_records(&scanned.records) {
            Ok(root_store) => {
                let views = view_records_from_records(&scanned.records)?;
                let repaired = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
                publish_bytes(repaired_path, &repaired, PublishMode::CreateNew)?;
            }
            Err(logical_error) => {
                let repaired_records = records_with_monotonic_commit_cycles(&scanned.records)?;
                let output_bytes = repack_records_preserving_history(&repaired_records)?;
                publish_bytes(repaired_path, &output_bytes, PublishMode::CreateNew)?;
                invalidate_scan_cache(repaired_path);
                let output_scan = scan_file(
                    &fs::read(repaired_path).map_err(|err| {
                        PactError::storage(format!("failed to read repaired .pact file: {err}"))
                    })?,
                    true,
                );
                let latest_commit_record = output_scan
                    .records
                    .iter()
                    .rev()
                    .find(|record| record.kind == "commit")
                    .ok_or_else(|| {
                        PactError::corrupt(format!(
                            "logical repair failed after {logical_error}: no commit records found"
                        ))
                    })?;
                let latest_commit: CommitRecord =
                    serde_json::from_value(latest_commit_record.payload.clone()).map_err(
                        |err| {
                            PactError::corrupt(format!(
                                "logical repair failed after {logical_error}: invalid latest commit record: {err}"
                            ))
                        },
                    )?;
                append_reindexed_head_record(repaired_path, &output_scan, &latest_commit)?;
            }
        }
        invalidate_scan_cache(repaired_path);
    } else if truncate_corrupt_tail && source_validation.manifest.last_valid_offset > 0 {
        publish_bytes(
            repaired_path,
            &bytes[..source_validation.manifest.last_valid_offset],
            PublishMode::CreateNew,
        )?;
        invalidate_scan_cache(repaired_path);
    } else {
        return Err(PactError::invalid(
            "repair requires either a semantically readable file or explicit corrupt-tail truncation",
        ));
    }
    mark_file_repaired(repaired_path)?;

    let repaired_bytes = fs::read(repaired_path)
        .map_err(|err| PactError::storage(format!("failed to read repaired .pact file: {err}")))?;
    let repaired_validation = validation_from_scan(scan_file(&repaired_bytes, true));
    Ok(PactFileRepairReport {
        schema: format!("{PACT_FILE_SCHEMA}.repair"),
        repaired_path: repaired_path.display().to_string(),
        source_validation,
        repaired_validation,
        truncated_tail: truncate_corrupt_tail,
        rebuilt_indexes: true,
    })
}

fn v2_repair_validation(
    ledger: &crate::store::ContextTreeLedger,
    file_bytes: usize,
    committed_bytes: usize,
    ok: bool,
    errors: Vec<String>,
) -> PactFileValidation {
    let latest_valid_branch_heads = ledger
        .branches
        .iter()
        .map(|(branch_id, branch)| (branch_id.clone(), branch.head_revision))
        .collect::<BTreeMap<_, _>>();
    let current = ledger.branches.get(&ledger.current_branch_id);
    let file_kind = current
        .and_then(|branch| branch.aux.get("pact_file_kind"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if ledger.branches.len() <= 1 {
                "branch".to_string()
            } else {
                "full_root".to_string()
            }
        });
    let mut capabilities = vec![
        "canonical-ledger".to_string(),
        "root-slots".to_string(),
        "query".to_string(),
        "projection".to_string(),
        "project_head".to_string(),
        "project_revision".to_string(),
        "repair".to_string(),
        format!("content_hash:{CONTENT_HASH_SCHEME}"),
    ];
    if file_kind == "slice" {
        capabilities.push("slice".to_string());
        capabilities.push("slice_open".to_string());
    }
    if current.is_some_and(|branch| {
        branch
            .aux
            .get("pact_file_healed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }) {
        capabilities.push("is_healed".to_string());
    }
    PactFileValidation {
        ok,
        status: if ok { "ok" } else { "invalid" }.to_string(),
        manifest: PactFileManifest {
            schema: crate::pact_file::layout::PACT_FILE_SCHEMA.to_string(),
            version: crate::pact_file::layout::FILE_VERSION,
            file_kind,
            capabilities,
            file_bytes,
            branch_count: ledger.branches.len(),
            node_count: current.map_or(0, |branch| branch.current.tree.index.by_id.len()),
            trace_count: ledger.trace_count(),
            component_count: current.map_or(0, |branch| branch.current.tree.index.by_id.len()),
            commit_count: current.map_or(0, |branch| branch.head_revision as usize + 1),
            head_count: ledger.branches.len(),
            cycle_count: current.map_or(0, |branch| branch.head_revision as usize + 1),
            last_valid_offset: committed_bytes,
            ..PactFileManifest::default()
        },
        latest_valid_record_sequence: current.map(|branch| branch.head_revision),
        latest_valid_transaction_id: current.and_then(|branch| {
            ledger
                .event_index
                .transaction_traces
                .values()
                .filter(|trace| trace.branch_id == branch.branch_id)
                .max_by_key(|trace| trace.post_response_ref.as_ref().and_then(|r| r.revision))
                .map(|trace| trace.transaction_id.clone())
        }),
        latest_valid_branch_heads,
        recoverability: if ok {
            "fully_recoverable"
        } else {
            "partially_recoverable"
        }
        .to_string(),
        errors,
    }
}

pub fn file_repair_json(
    source_path: &Path,
    repaired_path: &Path,
    truncate_corrupt_tail: bool,
) -> PactResult<String> {
    serde_json::to_string_pretty(&repair_file(
        source_path,
        repaired_path,
        truncate_corrupt_tail,
    )?)
    .map_err(|err| PactError::codec(format!("failed to encode .pact repair report: {err}")))
}

fn mark_file_repaired(path: &Path) -> PactResult<()> {
    let bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read repaired .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Ok(());
    }
    if scanned.manifest.commit_count > 0 {
        return Ok(());
    }
    let mut root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    for branch in root_store.branches.values_mut() {
        branch
            .aux
            .insert("pact_file_healed".to_string(), Value::Bool(true));
    }
    let file = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    Ok(())
}

pub fn file_slice_json(
    source_path: &Path,
    output_path: &Path,
    spec_json: &str,
) -> PactResult<String> {
    let spec: PactFileSliceSpec = serde_json::from_str(spec_json)
        .map_err(|err| PactError::invalid(format!("invalid .pact slice spec JSON: {err}")))?;
    let raw = fs::read(source_path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&raw) {
        let mut ledger = crate::pact_file::open_ledger(source_path)?;
        apply_slice_spec(&mut ledger, &spec)?;
        write_v2_ledger(output_path, &ledger, PublishMode::CreateNew)?;
        return file_validate_json(output_path);
    }
    if crate::pact_pack::is_packed_v1(&raw) {
        let branch = spec.branch_id.as_deref().unwrap_or("main");
        if branch != "main" {
            return Err(PactError::not_found(format!(
                "packed-v1 files expose only the main branch, not {branch}"
            )));
        }
        let container = crate::pact_pack::PackedV1::read(&raw)?;
        let target_revision = spec
            .end_cycle
            .or(spec.start_cycle)
            .unwrap_or(container.head_rev);
        let tree = container.project_revision(target_revision)?;
        let root_store_json = crate::store::new_ledger_json(
            &tree.to_string(),
            Some("main"),
            crate::components::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
        )?;
        file_create_from_ledger_json(
            output_path,
            &root_store_json,
            crate::components::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
        )?;
        invalidate_scan_cache(output_path);
        return file_validate_json(output_path);
    }
    let bytes = if crate::pact_pack::is_packed_v1(&raw) {
        packed_v1_to_v0_bytes(&raw)?
    } else {
        raw
    };
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot slice invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let mut root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    apply_slice_spec(&mut root_store, &spec)?;
    let slice = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(output_path, &slice, PublishMode::CreateNew)?;
    invalidate_scan_cache(output_path);
    file_validate_json(output_path)
}

pub fn file_import_check_json(path: &Path) -> PactResult<String> {
    let session = scan::PactReadSession::open(path)?;
    let validation = session.validation();
    if validation.ok {
        ledger_from_records(session.records())?;
    }
    serde_json::to_string_pretty(&validation)
        .map_err(|err| PactError::codec(format!("failed to encode .pact import check: {err}")))
}

pub fn file_import_json(target_path: &Path, slice_path: &Path) -> PactResult<String> {
    import_or_merge_file_json(target_path, slice_path, true)
}

pub fn file_merge_json(target_path: &Path, slice_path: &Path) -> PactResult<String> {
    import_or_merge_file_json(target_path, slice_path, false)
}

pub fn file_bundle_verify_json(path: &Path) -> PactResult<String> {
    let session = scan::PactReadSession::open(path)?;
    if !session.errors().is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot verify bundle for invalid .pact file: {}",
            session.errors().join("; ")
        )));
    }
    let _ = session;
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_BUNDLE_SCHEMA}.verify"),
        "pact_path": path.display().to_string(),
        "ok": true,
        "required_count": 0,
        "bundle_item_count": 0,
        "inline_item_count": 0,
        "missing": [],
        "invalid": [],
        "extras": [],
    }))
    .map_err(|err| {
        PactError::codec(format!(
            "failed to encode .pact bundle verify report: {err}"
        ))
    })
}

pub fn file_bundle_create_json(path: &Path) -> PactResult<String> {
    let bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot create bundle for invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let bundle = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(path, &bundle, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(path);
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_BUNDLE_SCHEMA}.create"),
        "pact_path": path.display().to_string(),
        "bundle_item_count": 0,
    }))
    .map_err(|err| {
        PactError::codec(format!(
            "failed to encode .pact bundle create report: {err}"
        ))
    })
}

pub fn file_bundle_export_full_json(path: &Path, output_path: &Path) -> PactResult<String> {
    let bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    let validation = validation_from_scan(scanned.clone());
    if !validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot export full file from invalid .pact file: {}",
            validation.errors.join("; ")
        )));
    }
    let root_store = ledger_from_records(&scanned.records)?;
    let views = view_records_from_records(&scanned.records)?;
    let export = PactFileBuilder::from_ledger_with_views(&root_store, &views)?;
    publish_bytes(output_path, &export, PublishMode::CreateNew)?;
    invalidate_scan_cache(output_path);
    let preserved_content_records =
        append_missing_file_level_content_records(output_path, &scanned.records)?;
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_BUNDLE_SCHEMA}.export_full"),
        "source_path": path.display().to_string(),
        "output_path": output_path.display().to_string(),
        "output_file_bytes": fs::metadata(output_path).map(|meta| meta.len()).unwrap_or(0),
        "preserved_file_level_content_records": preserved_content_records,
    }))
    .map_err(|err| {
        PactError::codec(format!(
            "failed to encode .pact bundle export report: {err}"
        ))
    })
}

pub fn file_bundle_prune_json(path: &Path) -> PactResult<String> {
    let session = scan::PactReadSession::open(path)?;
    if !session.errors().is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot prune bundle for invalid .pact file: {}",
            session.errors().join("; ")
        )));
    }
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_BUNDLE_SCHEMA}.prune"),
        "pact_path": path.display().to_string(),
        "before_count": 0,
        "after_count": 0,
        "removed_count": 0,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact bundle prune report: {err}")))
}

fn import_or_merge_file_json(
    target_path: &Path,
    slice_path: &Path,
    detach_on_mismatch: bool,
) -> PactResult<String> {
    let target_bytes = fs::read(target_path)
        .map_err(|err| PactError::storage(format!("failed to read target .pact file: {err}")))?;
    let source_bytes = fs::read(slice_path)
        .map_err(|err| PactError::storage(format!("failed to read source .pact file: {err}")))?;
    if crate::pact_file::is_v2_file(&target_bytes) && crate::pact_file::is_v2_file(&source_bytes) {
        let mut target_root = crate::pact_file::open_ledger(target_path)?;
        let source_root = crate::pact_file::open_ledger(slice_path)?;
        let mut target_views = v2_view_records(&target_root)?;
        let source_views = v2_view_records(&source_root)?;
        let source_root_id = source_root.root_id.clone();
        let same_root = target_root.root_id == source_root_id;
        if !same_root && !detach_on_mismatch {
            return Err(PactError::invalid(format!(
                "root mismatch: target {} source {}",
                target_root.root_id, source_root_id
            )));
        }
        merge_content_store(&mut target_root.content, &source_root.content)?;
        let mut branch_map = BTreeMap::new();
        let mut imported_branch_ids = Vec::new();
        for (source_branch_id, mut branch) in source_root.branches {
            let target_branch_id =
                if same_root && !target_root.branches.contains_key(&source_branch_id) {
                    source_branch_id.clone()
                } else {
                    unique_branch_id(&target_root, &format!("imported_{source_branch_id}"))
                };
            if target_branch_id != source_branch_id || !same_root {
                branch.branch_id = target_branch_id.clone();
                branch.root_id = target_root.root_id.clone();
                branch.parent_branch_id = None;
                branch.fork_ref = None;
                branch.base_ref.root_id = target_root.root_id.clone();
                branch.base_ref.branch_id = target_branch_id.clone();
                branch.current.tree.root_id = target_root.root_id.clone();
                branch.current.branch_id = target_branch_id.clone();
                branch.aux.insert(
                    "imported_from".to_string(),
                    json!({
                        "source_path": slice_path.display().to_string(),
                        "source_root_id": source_root_id,
                        "source_branch_id": source_branch_id,
                    }),
                );
            }
            imported_branch_ids.push(target_branch_id.clone());
            branch_map.insert(source_branch_id, target_branch_id.clone());
            target_root.branches.insert(target_branch_id, branch);
        }
        let mut imported_views = PactViewRecords::default();
        for mut view in source_views.views {
            view.root_id = target_root.root_id.clone();
            if let Some(mapped) = branch_map.get(&view.source_branch_id) {
                view.source_branch_id = mapped.clone();
            }
            if let Some(view_branch_id) = view.view_branch_id.as_deref()
                && let Some(mapped) = branch_map.get(view_branch_id)
            {
                view.view_branch_id = Some(mapped.clone());
            }
            imported_views.views.push(view);
        }
        let merged_view_count = imported_views.views.len();
        merge_view_records(&mut target_views, imported_views);
        set_v2_view_records(&mut target_root, &target_views)?;
        write_v2_ledger(target_path, &target_root, PublishMode::ReplaceExisting)?;
        let report = PactFileImportReport {
            schema: format!("{PACT_FILE_SCHEMA}.import"),
            source_path: slice_path.display().to_string(),
            target_path: target_path.display().to_string(),
            imported_branch_ids,
            merged_component_count: 0,
            merged_view_count,
            applied: true,
        };
        return serde_json::to_string_pretty(&report).map_err(|err| {
            PactError::codec(format!("failed to encode .pact import report: {err}"))
        });
    }
    let target_scan = scan_file(&target_bytes, true);
    let target_validation = validation_from_scan(target_scan.clone());
    if !target_validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot import into invalid target .pact file: {}",
            target_validation.errors.join("; ")
        )));
    }
    let source_scan = scan_file(&source_bytes, true);
    let source_validation = validation_from_scan(source_scan.clone());
    if !source_validation.ok {
        return Err(PactError::corrupt(format!(
            "cannot import invalid source .pact file: {}",
            source_validation.errors.join("; ")
        )));
    }

    let mut target_root = ledger_from_records(&target_scan.records)?;
    let source_root = ledger_from_records(&source_scan.records)?;
    let mut target_views = view_records_from_records(&target_scan.records)?;
    let source_views = view_records_from_records(&source_scan.records)?;

    let source_root_id = source_root.root_id.clone();
    let same_root = target_root.root_id == source_root_id;
    if !same_root && !detach_on_mismatch {
        return Err(PactError::invalid(format!(
            "root mismatch: target {} source {}",
            target_root.root_id, source_root_id
        )));
    }
    merge_content_store(&mut target_root.content, &source_root.content)?;

    let mut branch_map = BTreeMap::new();
    let mut imported_branch_ids = Vec::new();
    for (source_branch_id, mut branch) in source_root.branches {
        let target_branch_id = if same_root && !target_root.branches.contains_key(&source_branch_id)
        {
            source_branch_id.clone()
        } else {
            unique_branch_id(&target_root, &format!("imported_{source_branch_id}"))
        };
        if target_branch_id != source_branch_id || !same_root {
            branch.branch_id = target_branch_id.clone();
            branch.root_id = target_root.root_id.clone();
            branch.parent_branch_id = None;
            branch.fork_ref = None;
            branch.base_ref.root_id = target_root.root_id.clone();
            branch.base_ref.branch_id = target_branch_id.clone();
            branch.current.tree.root_id = target_root.root_id.clone();
            branch.current.branch_id = target_branch_id.clone();
            branch.aux.insert(
                "imported_from".to_string(),
                json!({
                    "source_path": slice_path.display().to_string(),
                    "source_root_id": source_root_id.clone(),
                    "source_branch_id": source_branch_id,
                }),
            );
        }
        imported_branch_ids.push(target_branch_id.clone());
        branch_map.insert(source_branch_id, target_branch_id.clone());
        target_root.branches.insert(target_branch_id, branch);
    }

    let merged_component_count = 0;

    let mut imported_views = PactViewRecords::default();
    for mut view in source_views.views {
        view.root_id = target_root.root_id.clone();
        if let Some(mapped) = branch_map.get(&view.source_branch_id) {
            view.source_branch_id = mapped.clone();
        }
        if let Some(view_branch_id) = view.view_branch_id.as_deref()
            && let Some(mapped) = branch_map.get(view_branch_id)
        {
            view.view_branch_id = Some(mapped.clone());
        }
        imported_views.views.push(view);
    }
    let merged_view_count = imported_views.views.len();
    merge_view_records(&mut target_views, imported_views);

    let file = PactFileBuilder::from_ledger_with_views(&target_root, &target_views)?;
    publish_bytes(target_path, &file, PublishMode::ReplaceExisting)?;
    invalidate_scan_cache(target_path);
    let report = PactFileImportReport {
        schema: format!("{PACT_FILE_SCHEMA}.import"),
        source_path: slice_path.display().to_string(),
        target_path: target_path.display().to_string(),
        imported_branch_ids,
        merged_component_count,
        merged_view_count,
        applied: true,
    };
    serde_json::to_string_pretty(&report)
        .map_err(|err| PactError::codec(format!("failed to encode .pact import report: {err}")))
}

fn merge_content_store(
    target: &mut crate::store::ContentStore,
    source: &crate::store::ContentStore,
) -> PactResult<()> {
    for (content_id, record) in &source.records {
        if let Some(existing) = target.records.get(content_id) {
            if existing != record {
                return Err(PactError::invalid(format!(
                    "content id collision while merging slice: {content_id}"
                )));
            }
            continue;
        }
        target.records.insert(content_id.clone(), record.clone());
    }
    target.rebuild_hash_index();
    Ok(())
}

fn decode_context_tree_ledger(root_store_json: &str) -> PactResult<ContextTreeLedger> {
    let value: serde_json::Value = serde_json::from_str(root_store_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT root store JSON: {err}")))?;
    crate::store::reject_removed_patch_history(&value)?;
    let store: ContextTreeLedger = serde_json::from_value(value)
        .map_err(|err| PactError::invalid(format!("invalid PACT root store JSON: {err}")))?;
    if store.schema != crate::store::ROOT_STORE_SCHEMA {
        return Err(PactError::invalid(format!(
            "unsupported PACT root store schema: {}",
            store.schema
        )));
    }
    Ok(store)
}

pub fn file_message_order_report(path: &Path) -> PactResult<Value> {
    use crate::pact_file::index::{IndexKey, message_order_key, read_index_value};
    use crate::transaction_trace::{TraceOp, TraceOpKind};

    let bytes = std::fs::read(path)?;
    let selected = crate::pact_file::root_slot::select_root_slot(&bytes)?;
    let root = selected.slot.index_root_address;
    let addresses =
        crate::pact_file::index::generation_addresses(&bytes, root, selected.slot.generation)?;
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        path,
        Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )?;

    struct Tally {
        pre_rank: u64,
        rank: u64,
        neither: u64,
        pruned: u64,
        rows: u64,
        carried: Vec<Value>,
    }

    let tally = |branch: &str, revision: u64, ops: &[TraceOp]| -> PactResult<Tally> {
        let mut tally = Tally {
            pre_rank: 0,
            rank: 0,
            neither: 0,
            pruned: 0,
            rows: 0,
            carried: Vec::new(),
        };
        let mut rank = 0_u64;
        for (ordinal, op) in ops.iter().enumerate() {
            let TraceOpKind::InsertComponent { component, .. } = &op.kind else {
                continue;
            };
            let is_message = component
                .get("componentType")
                .or_else(|| component.get("type"))
                .and_then(Value::as_str)
                == Some("message_turn");
            if !is_message {
                continue;
            }
            tally.rows += 1;
            let pre_rank = revision
                .saturating_mul(1 << 16)
                .saturating_add(ordinal as u64);
            let rank_key = message_order_key(revision, rank)?;
            let holds = |order: u64| -> PactResult<bool> {
                Ok(read_index_value(
                    &bytes,
                    root,
                    &IndexKey::MessageOrder {
                        branch: branch.to_string(),
                        order,
                    },
                )?
                .is_some_and(|value| value == op.component_id.as_bytes()))
            };
            let current = read_index_value(
                &bytes,
                root,
                &IndexKey::Component {
                    branch: branch.to_string(),
                    component: op.component_id.clone(),
                },
            )?
            .is_some();
            match (current, holds(pre_rank)?, holds(rank_key)?) {
                (false, _, _) => tally.pruned += 1,
                (true, true, _) => tally.pre_rank += 1,
                (true, false, true) => tally.rank += 1,
                (true, false, false) => tally.neither += 1,
            }
            if ordinal >= 1 << 16 {
                tally.carried.push(json!({
                    "revision": revision,
                    "ordinal": ordinal,
                    "rank": rank,
                    "component_id": op.component_id,
                }));
            }
            rank += 1;
        }
        Ok(tally)
    };

    let mut transactions = 0_u64;
    let mut message_rows = 0_u64;
    let mut pre_rank_hits = 0_u64;
    let mut rank_hits = 0_u64;
    let mut neither = 0_u64;
    let mut pruned = 0_u64;
    let mut carried = Vec::new();
    for address in addresses {
        let generation = crate::pact_file::codec::decode_record_generation_at(&bytes, address)?;
        let decoded = crate::pact_file::codec::decode_transaction(&bytes, address, generation)?;
        let transaction = decoded.transaction;
        transactions += 1;
        let revision = transaction.trace.to_rev;
        let result = if let Some(checkpoint) = transaction.head_checkpoint.as_ref() {
            let ledger = crate::pact_file::head_checkpoint::hydrate_ledger_typed_from_pages(
                decoded.marker.index_root_address,
                checkpoint,
                &snapshot,
            )?;
            let typed_ops =
                crate::pact_file::index::baseline_component_ops_by_branch_from_ledger(&ledger)?
                    .into_iter()
                    .find(|branch| branch.branch_id == transaction.branch_id)
                    .map(|branch| branch.ops)
                    .unwrap_or_default();
            let typed = tally(&transaction.branch_id, revision, &typed_ops)?;
            if typed.neither == 0 {
                typed
            } else {
                let hydrated;
                let value = if crate::pact_file::head_checkpoint::is_compact_ledger(checkpoint) {
                    hydrated = crate::pact_file::head_checkpoint::hydrate_ledger(
                        &bytes,
                        decoded.marker.index_root_address,
                        checkpoint,
                    )?;
                    &hydrated
                } else {
                    checkpoint
                };
                let diff_ops = crate::pact_file::index::baseline_component_ops(value)?;
                let diffed = tally(&transaction.branch_id, revision, &diff_ops)?;
                if diffed.neither < typed.neither {
                    diffed
                } else {
                    typed
                }
            }
        } else {
            tally(&transaction.branch_id, revision, &transaction.trace.ops)?
        };
        message_rows += result.rows;
        pre_rank_hits += result.pre_rank;
        rank_hits += result.rank;
        neither += result.neither;
        pruned += result.pruned;
        carried.extend(result.carried);
    }
    Ok(json!({
        "schema": "pact.message_order_report.v1",
        "transactions": transactions,
        "message_rows": message_rows,
        "carried_rows": carried.len(),
        "head_index_matches": {"pre_rank": pre_rank_hits, "rank": rank_hits, "neither": neither, "pruned": pruned},
        "carried": carried,
    }))
}
