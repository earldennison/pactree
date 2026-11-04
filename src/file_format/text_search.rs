use super::corpus::{
    SelectedComponent, SelectorStreamOptions, make_selector_stream_options, stream_selected_nodes,
};
use super::*;
use crate::error::{PactError, PactResult};
use regex::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::ops::ControlFlow;

struct PactGrepSurface<'a> {
    component_id: String,
    element_kind: String,
    field: &'static str,
    text: Cow<'a, str>,
    visibility: NodeVisibility,
}

#[derive(Clone, Debug)]
struct TextSearchNodeMeta {
    parent_id: Option<String>,
    element_kind: String,
    role: Option<String>,
    cycle: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextSearchScope {
    All,
    Head,
}

enum TextMatcher {
    Literal(String),
    Regex(Regex),
}

impl TextMatcher {
    fn new(query: &PactTextSearchQuery) -> PactResult<Self> {
        if query.text.is_empty() {
            return Err(PactError::invalid(
                "text-search requires a non-empty search string".to_string(),
            ));
        }
        if query.regex {
            RegexBuilder::new(&query.text)
                .case_insensitive(query.case_insensitive)
                .build()
                .map(TextMatcher::Regex)
                .map_err(|err| PactError::invalid(format!("invalid text-search regex: {err}")))
        } else if query.case_insensitive {
            RegexBuilder::new(&regex::escape(&query.text))
                .case_insensitive(true)
                .build()
                .map(TextMatcher::Regex)
                .map_err(|err| PactError::invalid(format!("invalid text-search literal: {err}")))
        } else {
            Ok(TextMatcher::Literal(query.text.clone()))
        }
    }

    fn find_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        match self {
            TextMatcher::Literal(needle) => text
                .match_indices(needle)
                .map(|(start, value)| (start, start + value.len()))
                .collect(),
            TextMatcher::Regex(regex) => regex
                .find_iter(text)
                .filter(|matched| matched.start() != matched.end())
                .map(|matched| (matched.start(), matched.end()))
                .collect(),
        }
    }
}

pub fn file_text_search_json(path: &Path, mut query: PactTextSearchQuery) -> PactResult<String> {
    query.scope = normalize_text_search_scope(&query.scope)?.to_string();
    query.limit = query.limit.clamp(1, 500);
    query.preview_chars = query.preview_chars.clamp(1, 4096);

    let scanned = scan_file_cached(path, true)?;
    if !scanned.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot text-search invalid .pact file: {}",
            scanned.errors.join("; ")
        )));
    }
    let result = text_search_records(&scanned.records, query)?;
    serde_json::to_string_pretty(&result).map_err(|err| {
        PactError::codec(format!("failed to encode .pact text-search result: {err}"))
    })
}

fn text_search_records(
    records: &[ScannedRecord],
    query: PactTextSearchQuery,
) -> PactResult<PactTextSearchResult> {
    let scope = parse_text_search_scope(&query.scope)?;
    let matcher = TextMatcher::new(&query)?;
    let root_store = ledger_from_records(records)?;
    let branch_id = root_store.current_branch_id.clone();
    let branch = root_store
        .branches
        .get(&branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    let node_index = text_search_node_index_from_records(records);
    let current_records = branch.current.component_records();
    let current_ids = current_records.keys().cloned().collect::<BTreeSet<_>>();
    let mut birth_by_node: BTreeMap<String, NodeBirth> = branch
        .current
        .tree
        .first_seen
        .iter()
        .map(|(component_id, revision)| {
            (
                component_id.clone(),
                NodeBirth {
                    revision: Some(*revision),
                    cycle: node_cycle_for_id(&current_records, component_id),
                },
            )
        })
        .collect();
    let mut commit_branch_by_node = BTreeMap::new();
    collect_commit_metadata(records, &mut birth_by_node, &mut commit_branch_by_node);
    let mut content = TextContentLookup::new(records);
    let mut hits = Vec::new();

    let (source, selector_stats) = if let Some(selector) = query.selector.as_deref() {
        let options: SelectorStreamOptions = make_selector_stream_options(None, 1000, "newest")?;
        let mut stats = PactSelectorStreamStats {
            records_seen: 0,
            records_temporal_rejected: 0,
            records_selector_rejected: 0,
            payloads_touched: 0,
            source: "stream".to_string(),
            fallback_used: false,
        };
        let mut payloads_touched: usize = 0;
        stream_selected_nodes(records, selector, &options, &mut stats, |selected| {
            search_selected_node(
                selected,
                &query,
                &matcher,
                &current_ids,
                &node_index,
                &birth_by_node,
                &commit_branch_by_node,
                &branch_id,
                branch,
                &mut content,
                &mut hits,
                &mut payloads_touched,
            )
            .unwrap_or(ControlFlow::Continue(()))
        })?;
        stats.payloads_touched = stats.payloads_touched.saturating_add(payloads_touched);
        (format!("selector_stream:{}", stats.source), Some(stats))
    } else {
        match scope {
            TextSearchScope::All => {
                let mut searched_current_ids = BTreeSet::new();
                for node in current_records.values() {
                    searched_current_ids.insert(node.id.clone());
                    search_node(
                        node,
                        &query,
                        &matcher,
                        &current_ids,
                        &node_index,
                        &birth_by_node,
                        &commit_branch_by_node,
                        &branch_id,
                        branch,
                        &mut content,
                        &mut hits,
                    )?;
                    if hits.len() >= query.limit {
                        break;
                    }
                }
                for record in records.iter().filter(|record| record.kind == "node") {
                    let Some(node) = text_search_node_record_from_value(record.payload.clone())
                    else {
                        continue;
                    };
                    if searched_current_ids.contains(&node.id) {
                        continue;
                    }
                    search_node(
                        &node,
                        &query,
                        &matcher,
                        &current_ids,
                        &node_index,
                        &birth_by_node,
                        &commit_branch_by_node,
                        &branch_id,
                        branch,
                        &mut content,
                        &mut hits,
                    )?;
                    if hits.len() >= query.limit {
                        break;
                    }
                }
            }
            TextSearchScope::Head => {
                for node in current_records.values() {
                    search_node(
                        node,
                        &query,
                        &matcher,
                        &current_ids,
                        &node_index,
                        &birth_by_node,
                        &commit_branch_by_node,
                        &branch_id,
                        branch,
                        &mut content,
                        &mut hits,
                    )?;
                    if hits.len() >= query.limit {
                        break;
                    }
                }
            }
        }
        ("record_scan".to_string(), None)
    };

    Ok(PactTextSearchResult {
        schema: format!("{PACT_FILE_SCHEMA}.text_search.v1"),
        query,
        hits,
        source,
        fallback_used: false,
        selector_stats,
    })
}

#[allow(clippy::too_many_arguments)]
fn search_selected_node(
    selected: &SelectedComponent,
    query: &PactTextSearchQuery,
    matcher: &TextMatcher,
    current_ids: &BTreeSet<String>,
    node_index: &BTreeMap<String, TextSearchNodeMeta>,
    birth_by_node: &BTreeMap<String, NodeBirth>,
    commit_branch_by_node: &BTreeMap<String, String>,
    default_branch_id: &str,
    branch: &PactBranch,
    content: &mut TextContentLookup<'_>,
    hits: &mut Vec<PactTextSearchHit>,
    payloads_touched: &mut usize,
) -> PactResult<ControlFlow<()>> {
    let node = &selected.node;
    let component_id = node.id.clone();
    let visibility = NodeVisibility {
        resident_in_current_surface: selected.resident_in_current_surface
            || current_ids.contains(&component_id),
        archived_in_pact: selected.archived_in_pact || node_index.contains_key(&component_id),
    };
    let surfaces = grep_surfaces_for_node(node, content, visibility.clone());
    if !surfaces.is_empty() {
        *payloads_touched = payloads_touched.saturating_add(1);
    }
    for surface in surfaces {
        for (byte_start, byte_end) in matcher.find_ranges(&surface.text) {
            let match_range = text_range(&surface.text, byte_start, byte_end)?;
            let lifecycle = node_lifecycle(node, &component_id, birth_by_node, None);
            hits.push(PactTextSearchHit {
                component_id: surface.component_id.clone(),
                branch_id: commit_branch_by_node
                    .get(&component_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        if !selected.branch_id.is_empty() {
                            selected.branch_id.clone()
                        } else {
                            default_branch_id.to_string()
                        }
                    }),
                cycle: selected.cycle.or_else(|| node_cycle_from_record(node)),
                element_kind: surface.element_kind.clone(),
                field: surface.field.to_string(),
                match_range,
                exact_text_preview: preview_around_match(
                    &surface.text,
                    byte_start,
                    byte_end,
                    query.preview_chars,
                )?,
                node_text_len_bytes: surface.text.len().min(u32::MAX as usize) as u32,
                node_text_len_chars: surface.text.chars().count().min(u32::MAX as usize) as u32,
                message_container: message_container_for_node(&component_id, branch, node_index),
                lifecycle,
                visibility: surface.visibility.clone(),
            });
            if hits.len() >= query.limit {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
    Ok(ControlFlow::Continue(()))
}

#[allow(clippy::too_many_arguments)]
fn search_node(
    node: &ComponentRecord,
    query: &PactTextSearchQuery,
    matcher: &TextMatcher,
    current_ids: &BTreeSet<String>,
    node_index: &BTreeMap<String, TextSearchNodeMeta>,
    birth_by_node: &BTreeMap<String, NodeBirth>,
    commit_branch_by_node: &BTreeMap<String, String>,
    default_branch_id: &str,
    branch: &PactBranch,
    content: &mut TextContentLookup<'_>,
    hits: &mut Vec<PactTextSearchHit>,
) -> PactResult<()> {
    let component_id = node.id.clone();
    let visibility = NodeVisibility {
        resident_in_current_surface: current_ids.contains(&component_id),
        archived_in_pact: node_index.contains_key(&component_id),
    };
    let surfaces = grep_surfaces_for_node(node, content, visibility.clone());
    for surface in surfaces {
        for (byte_start, byte_end) in matcher.find_ranges(&surface.text) {
            let match_range = text_range(&surface.text, byte_start, byte_end)?;
            let lifecycle = node_lifecycle(node, &component_id, birth_by_node, None);
            hits.push(PactTextSearchHit {
                component_id: surface.component_id.clone(),
                branch_id: commit_branch_by_node
                    .get(&component_id)
                    .cloned()
                    .unwrap_or_else(|| default_branch_id.to_string()),
                cycle: node_cycle_from_record(node),
                element_kind: surface.element_kind.clone(),
                field: surface.field.to_string(),
                match_range,
                exact_text_preview: preview_around_match(
                    &surface.text,
                    byte_start,
                    byte_end,
                    query.preview_chars,
                )?,
                node_text_len_bytes: surface.text.len().min(u32::MAX as usize) as u32,
                node_text_len_chars: surface.text.chars().count().min(u32::MAX as usize) as u32,
                message_container: message_container_for_node(&component_id, branch, node_index),
                lifecycle,
                visibility: surface.visibility.clone(),
            });
            if hits.len() >= query.limit {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn normalize_text_search_scope(scope: &str) -> PactResult<&'static str> {
    match scope.trim() {
        "" | "all" | "archive" | "archived" | "pact" => Ok("all"),
        "head" | "current" | "current_surface" | "current-surface" | "context_head"
        | "context-head" => Ok("head"),
        "activity" | "activity_surface" | "activity-surface" | "timeline" => {
            Err(PactError::invalid(
                "activity_surface is not a text-search scope; use --scope all or --scope head"
                    .to_string(),
            ))
        }
        other => Err(PactError::invalid(format!(
            "unsupported text-search scope {other:?}; expected all or head"
        ))),
    }
}

fn parse_text_search_scope(scope: &str) -> PactResult<TextSearchScope> {
    match normalize_text_search_scope(scope)? {
        "all" => Ok(TextSearchScope::All),
        "head" => Ok(TextSearchScope::Head),
        _ => unreachable!(),
    }
}

fn collect_commit_metadata(
    records: &[ScannedRecord],
    birth_by_node: &mut BTreeMap<String, NodeBirth>,
    branch_by_node: &mut BTreeMap<String, String>,
) {
    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        for component_id in commit_node_refs(&commit) {
            branch_by_node
                .entry(component_id.clone())
                .or_insert_with(|| commit.branch_id.clone());
            birth_by_node
                .entry(component_id.clone())
                .or_insert(NodeBirth {
                    revision: Some(commit.revision),
                    cycle: Some(commit.cycle),
                });
        }
    }
}

fn commit_node_refs(commit: &CommitRecord) -> Vec<String> {
    let mut ids = Vec::new();
    for op in &commit.ops {
        match op {
            CommitRecordOp::InsertComponent { node_ref, .. }
            | CommitRecordOp::ReplaceComponent { node_ref, .. } => ids.push(node_ref.clone()),
            CommitRecordOp::AppendComponents { node_refs, .. } => ids.extend(node_refs.clone()),
            CommitRecordOp::DeleteComponent { component_id }
            | CommitRecordOp::SetComponentAux { component_id, .. }
            | CommitRecordOp::UnsetComponentAux { component_id, .. }
            | CommitRecordOp::AttachContentRef { component_id, .. }
            | CommitRecordOp::ReplaceContentRange { component_id, .. } => {
                ids.push(component_id.clone())
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn grep_surfaces_for_node<'a>(
    node: &'a ComponentRecord,
    content: &mut TextContentLookup<'_>,
    visibility: NodeVisibility,
) -> Vec<PactGrepSurface<'a>> {
    let mut surfaces = Vec::new();
    let element_kind = node.kind.as_deref().unwrap_or(&node.node_type).to_string();
    if let Some(text) = node.content.as_deref() {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "content",
            Cow::Borrowed(text),
            visibility.clone(),
        ));
    } else if let Some(content_id) = node.content_ref.as_deref()
        && let Some(text) = content.get(content_id)
    {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "content_ref",
            Cow::Owned(text),
            visibility.clone(),
        ));
    }
    if let Some(value) = &node.parameters {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "parameters",
            Cow::Owned(json_text(value)),
            visibility.clone(),
        ));
    } else if let Some(content_id) = node
        .parameters_ref
        .as_ref()
        .and_then(ref_id_from_refish_value)
        && let Some(text) = content.get(&content_id)
    {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "parameters_ref",
            Cow::Owned(text),
            visibility.clone(),
        ));
    }
    if let Some(value) = &node.result {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "result",
            Cow::Owned(json_text(value)),
            visibility.clone(),
        ));
    } else if let Some(content_id) = node.result_ref.as_ref().and_then(ref_id_from_refish_value)
        && let Some(text) = content.get(&content_id)
    {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "result_ref",
            Cow::Owned(text),
            visibility.clone(),
        ));
    }
    if let Some(text) = node.error_message.as_deref() {
        surfaces.push(grep_surface(
            node,
            element_kind.clone(),
            "error_message",
            Cow::Borrowed(text),
            visibility.clone(),
        ));
    } else if let Some(content_id) = node.error_ref.as_ref().and_then(ref_id_from_refish_value)
        && let Some(text) = content.get(&content_id)
    {
        surfaces.push(grep_surface(
            node,
            element_kind,
            "error_ref",
            Cow::Owned(text),
            visibility,
        ));
    }
    surfaces
}

fn grep_surface<'a>(
    node: &ComponentRecord,
    element_kind: String,
    field: &'static str,
    text: Cow<'a, str>,
    visibility: NodeVisibility,
) -> PactGrepSurface<'a> {
    PactGrepSurface {
        component_id: node.id.clone(),
        element_kind,
        field,
        text,
        visibility,
    }
}

fn json_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

struct TextContentLookup<'a> {
    records: &'a [ScannedRecord],
    memo: BTreeMap<String, Option<String>>,
    chunk_fallback: Option<BTreeMap<String, String>>,
}

impl<'a> TextContentLookup<'a> {
    fn new(records: &'a [ScannedRecord]) -> Self {
        Self {
            records,
            memo: BTreeMap::new(),
            chunk_fallback: None,
        }
    }

    fn get(&mut self, content_id: &str) -> Option<String> {
        if let Some(text) = self.memo.get(content_id) {
            return text.clone();
        }
        let text = self.decode_content_record(content_id).or_else(|| {
            self.chunk_fallback
                .get_or_insert_with(|| content_texts_from_records(self.records))
                .get(content_id)
                .cloned()
        });
        self.memo.insert(content_id.to_string(), text.clone());
        text
    }

    fn decode_content_record(&self, content_id: &str) -> Option<String> {
        self.records
            .iter()
            .filter(|record| record.kind == "content")
            .find_map(|record| {
                let payload: ContentPayload =
                    serde_json::from_value(record.payload.clone()).ok()?;
                if payload.id != content_id {
                    return None;
                }
                decode_content_payload_text(&payload).ok()
            })
    }
}

fn text_range(text: &str, byte_start: usize, byte_end: usize) -> PactResult<TextRange> {
    if !text.is_char_boundary(byte_start) || !text.is_char_boundary(byte_end) {
        return Err(PactError::internal(
            "text-search match did not land on UTF-8 boundaries".to_string(),
        ));
    }
    Ok(TextRange {
        byte_start: byte_start.min(u32::MAX as usize) as u32,
        byte_end: byte_end.min(u32::MAX as usize) as u32,
        char_start: text[..byte_start].chars().count().min(u32::MAX as usize) as u32,
        char_end: text[..byte_end].chars().count().min(u32::MAX as usize) as u32,
    })
}

fn preview_around_match(
    text: &str,
    byte_start: usize,
    byte_end: usize,
    preview_chars: usize,
) -> PactResult<String> {
    if !text.is_char_boundary(byte_start) || !text.is_char_boundary(byte_end) {
        return Err(PactError::internal(
            "text-search preview range did not land on UTF-8 boundaries".to_string(),
        ));
    }
    let char_start = text[..byte_start].chars().count();
    let char_end = text[..byte_end].chars().count();
    let match_chars = char_end.saturating_sub(char_start);
    let side = preview_chars.saturating_sub(match_chars) / 2;
    let preview_start_char = char_start.saturating_sub(side);
    let preview_end_char = (char_end + side).min(text.chars().count());
    Ok(text
        .chars()
        .skip(preview_start_char)
        .take(preview_end_char.saturating_sub(preview_start_char))
        .collect())
}

fn node_lifecycle(
    node: &ComponentRecord,
    component_id: &str,
    birth_by_node: &BTreeMap<String, NodeBirth>,
    last_seen_revision: Option<u64>,
) -> PactNodeLifecycle {
    let born = birth_by_node
        .get(component_id)
        .cloned()
        .unwrap_or(NodeBirth {
            revision: None,
            cycle: node_cycle_from_record(node),
        });
    PactNodeLifecycle {
        born,
        last_seen_as_this: last_seen_revision.map(|revision| NodeSeen {
            revision: Some(revision),
            cycle: node_cycle_from_record(node),
        }),
    }
}

fn node_cycle_for_id(nodes: &BTreeMap<String, ComponentRecord>, component_id: &str) -> Option<u64> {
    nodes.get(component_id).and_then(node_cycle_from_record)
}

fn node_cycle_from_record(node: &ComponentRecord) -> Option<u64> {
    node.aux
        .get("cycle")
        .and_then(Value::as_u64)
        .or_else(|| node.aux.get("pact_cycle").and_then(Value::as_u64))
}

fn message_container_for_node(
    component_id: &str,
    branch: &PactBranch,
    node_index: &BTreeMap<String, TextSearchNodeMeta>,
) -> Option<MessageContainerHit> {
    let mut current_id = Some(component_id.to_string());
    let mut seen = BTreeSet::new();
    while let Some(id) = current_id.as_deref() {
        if !seen.insert(id.to_string()) {
            return None;
        }
        let current_node = branch.current.component_record(id);
        let indexed_node = node_index.get(id);
        let kind = current_node
            .as_ref()
            .map(|node| node.kind.as_deref().unwrap_or(&node.node_type))
            .or_else(|| indexed_node.map(|meta| meta.element_kind.as_str()))?;
        if kind == "message_turn" {
            return Some(MessageContainerHit {
                component_id: id.to_string(),
                role: current_node
                    .as_ref()
                    .and_then(|node| node.role.clone())
                    .or_else(|| indexed_node.and_then(|meta| meta.role.clone())),
                cycle: current_node
                    .as_ref()
                    .and_then(node_cycle_from_record)
                    .or_else(|| indexed_node.and_then(|meta| meta.cycle)),
                token_count: None,
                tokenizer_ref: None,
                token_count_estimated: false,
            });
        }
        current_id = current_node
            .and_then(|node| node.parent_id)
            .or_else(|| indexed_node.and_then(|meta| meta.parent_id.clone()));
    }
    None
}

fn text_search_node_index_from_records(
    records: &[ScannedRecord],
) -> BTreeMap<String, TextSearchNodeMeta> {
    let mut nodes = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "node") {
        if let Some(node) = text_search_node_record_from_value(record.payload.clone()) {
            let cycle = node_cycle_from_record(&node);
            nodes.insert(
                node.id.clone(),
                TextSearchNodeMeta {
                    parent_id: node.parent_id,
                    element_kind: node.kind.unwrap_or(node.node_type),
                    role: node.role,
                    cycle,
                },
            );
        }
    }
    nodes
}

fn text_search_node_record_from_value(mut value: Value) -> Option<ComponentRecord> {
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
