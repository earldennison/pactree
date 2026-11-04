use super::query::cycle_ranges_from_records;
use super::*;
use crate::error::{PactError, PactResult};
use crate::selector::{
    DepthAnchorSelector, ParsedSelectorControls, SelectorIndexSeed, depth_anchor_selector,
    match_node_value, parse_selector_controls, selector_index_seed, selector_requires_structure,
};
use std::ops::ControlFlow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SelectorOrder {
    Newest,
    Oldest,
}

impl SelectorOrder {
    fn from_str(value: &str) -> PactResult<Self> {
        match value.trim() {
            "" | "newest" | "newest_first" | "newest-first" => Ok(Self::Newest),
            "oldest" | "oldest_first" | "oldest-first" => Ok(Self::Oldest),
            other => Err(PactError::invalid(format!(
                "unsupported selector order {other:?}; expected 'newest' or 'oldest'"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemporalPoint {
    Current,
    Relative(i64),
    Cycle(u64),
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemporalRange {
    raw: String,
    start: TemporalPoint,
    end: Option<TemporalPoint>,
}

#[derive(Clone, Debug)]
pub(super) struct ParsedCorpusSelector {
    temporal: TemporalRange,

    structural_selector: String,

    structural_selector_without_controls: String,
    controls: ParsedSelectorControls,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedSnapshotSearchSelector {
    pub(crate) structural_selector: String,
    pub(crate) structural_selector_with_controls: String,
    pub(crate) current_only: bool,
    pub(crate) has_result_controls: bool,
}

pub(crate) fn parse_snapshot_search_selector(
    selector: &str,
) -> PactResult<ParsedSnapshotSearchSelector> {
    let parsed = parse_corpus_selector(selector)?;
    Ok(ParsedSnapshotSearchSelector {
        structural_selector: parsed.structural_selector_without_controls,
        structural_selector_with_controls: parsed.structural_selector,
        current_only: matches!(parsed.temporal.start, TemporalPoint::Current)
            && parsed.temporal.end.is_none(),
        has_result_controls: parsed.controls.limit.is_some()
            || parsed.controls.offset != 0
            || parsed.controls.mode.is_some(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CorpusSelectorPlan {
    NodeLocal,
    SnapshotWindow,
}

#[derive(Clone, Debug)]
pub(super) struct SelectorStreamOptions {
    pub(super) branch_id: Option<String>,
    pub(super) limit: usize,
    pub(super) order: SelectorOrder,
}

#[derive(Clone, Debug)]
pub(super) struct SelectedComponent {
    pub(super) node: ComponentRecord,
    pub(super) cycle: Option<u64>,
    pub(super) revision: Option<u64>,
    pub(super) branch_id: String,
    pub(super) resident_in_current_surface: bool,
    pub(super) archived_in_pact: bool,
    pub(super) time_ref: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct SelectorStreamReport {
    pub(super) hits: Vec<PactSelectorHit>,
    pub(super) stats: PactSelectorStreamStats,
    pub(super) temporal: PactTemporalSelectorReport,
}

pub fn file_select_json(path: &Path, query: PactFileSelectorQuery) -> PactResult<String> {
    if let Some(result) = try_indexed_current_select(path, query.clone())? {
        return serde_json::to_string_pretty(&result).map_err(|err| {
            PactError::codec(format!("failed to encode .pact selector result: {err}"))
        });
    }
    #[cfg(test)]
    crate::tests::instrumentation::FILE_SELECTOR_FULL_SCAN_COUNT
        .with(|count| count.set(count.get() + 1));
    file_select_json_full_scan(path, query)
}

pub(crate) fn file_select_json_full_scan(
    path: &Path,
    query: PactFileSelectorQuery,
) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    if !scanned.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot run selector on invalid .pact file: {}",
            scanned.errors.join("; ")
        )));
    }
    let result = select_records(&scanned.records, query)?;
    serde_json::to_string_pretty(&result)
        .map_err(|err| PactError::codec(format!("failed to encode .pact selector result: {err}")))
}

fn try_indexed_current_select(
    path: &Path,
    mut query: PactFileSelectorQuery,
) -> PactResult<Option<PactFileSelectorResult>> {
    query.limit = query.limit.clamp(1, 1000);
    let order = SelectorOrder::from_str(&query.order)?;
    query.order = order.as_str().to_string();
    let parsed = parse_corpus_selector(&query.selector)?;
    if !matches!(parsed.temporal.start, TemporalPoint::Current)
        || parsed.temporal.end.is_some()
        || parsed
            .controls
            .mode
            .as_deref()
            .is_some_and(|mode| mode != "all")
    {
        return Ok(None);
    }
    if let Some(anchor) = depth_anchor_selector(&parsed.structural_selector_without_controls)? {
        return indexed_depth_anchor_select(path, query, &parsed, &anchor);
    }
    let Some(seed) = selector_index_seed(&parsed.structural_selector_without_controls)? else {
        return Ok(None);
    };
    if matches!(&seed, SelectorIndexSeed::Type(value) if matches!(value.as_str(), "b" | "c" | "seg"))
    {
        return Ok(None);
    }
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )?;
    let metadata = snapshot.metadata()?;
    let branch_id = query
        .branch_id
        .clone()
        .unwrap_or_else(|| metadata.current_branch_id.clone());
    if metadata.composition_branches.contains(&branch_id) {
        return Ok(None);
    }
    let current_count = metadata
        .node_counts
        .get(&branch_id)
        .copied()
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    let (match_count, _) = indexed_seed_components(&snapshot, &branch_id, &seed, 0, 0)?;
    let skipped_matches = parsed.controls.offset.min(match_count);
    let hit_limit = query.limit.min(parsed.controls.limit.unwrap_or(usize::MAX));
    let hit_count = hit_limit.min(match_count.saturating_sub(skipped_matches));
    let forward_offset = match order {
        SelectorOrder::Newest => match_count
            .saturating_sub(skipped_matches)
            .saturating_sub(hit_count),
        SelectorOrder::Oldest => skipped_matches,
    };
    let (_, mut components) =
        indexed_seed_components(&snapshot, &branch_id, &seed, forward_offset, hit_count)?;
    if matches!(order, SelectorOrder::Newest) {
        components.reverse();
    }
    let hits = components
        .into_iter()
        .map(|component| indexed_selector_hit(component.value, &branch_id))
        .collect::<PactResult<Vec<_>>>()?;
    let exhausted_matches = match_count.saturating_sub(skipped_matches) < hit_limit;
    let (indexed_component_count, ranked_records_seen) = if let Some(stop) = hits.last() {
        let (indexed_count, rank) =
            snapshot.component_count_and_rank(&branch_id, &stop.component_id)?;
        let missing_root = usize::from(indexed_count < current_count);
        if current_count.saturating_sub(indexed_count) > 1 {
            return Ok(None);
        }
        let corpus_rank = rank
            + usize::from(
                missing_root == 1 && metadata.root_id.as_str() < stop.component_id.as_str(),
            );
        let seen = match order {
            SelectorOrder::Newest => current_count.saturating_sub(corpus_rank),
            SelectorOrder::Oldest => corpus_rank.saturating_add(1),
        };
        (indexed_count, seen)
    } else {
        (current_count.saturating_sub(1), current_count)
    };
    if current_count.saturating_sub(indexed_component_count) > 1 {
        return Ok(None);
    }
    let records_seen = if exhausted_matches {
        current_count
    } else {
        ranked_records_seen
    };
    let matched_seen = if hits.is_empty() {
        match_count
    } else {
        skipped_matches.saturating_add(hits.len())
    };
    let stats = PactSelectorStreamStats {
        records_seen,
        records_temporal_rejected: current_count,
        records_selector_rejected: records_seen.saturating_sub(matched_seen),
        payloads_touched: 0,
        source: "stream".to_string(),
        fallback_used: false,
    };
    Ok(Some(PactFileSelectorResult {
        schema: format!("{PACT_FILE_SCHEMA}.selector.v1"),
        query,
        temporal: PactTemporalSelectorReport {
            raw: parsed.temporal.raw.clone(),
            normalized: normalize_temporal(&parsed.temporal),
            cycles: Vec::new(),
        },
        source: "stream".to_string(),
        fallback_used: false,
        hits,
        selector_stats: Some(stats),
    }))
}

const UNRANKED_TURN_CONTAINER_TYPES: [&str; 3] =
    ["turn", "active_message", "active_provider_message"];

fn indexed_depth_anchor_select(
    path: &Path,
    query: PactFileSelectorQuery,
    parsed: &ParsedCorpusSelector,
    anchor: &DepthAnchorSelector,
) -> PactResult<Option<PactFileSelectorResult>> {
    let Ok(depth) = usize::try_from(anchor.depth) else {
        return Ok(None);
    };
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )?;
    let metadata = snapshot.metadata()?;
    let branch_id = query
        .branch_id
        .clone()
        .unwrap_or_else(|| metadata.current_branch_id.clone());
    if metadata.composition_branches.contains(&branch_id) {
        return Ok(None);
    }
    let current_count = metadata
        .node_counts
        .get(&branch_id)
        .copied()
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    for component_type in UNRANKED_TURN_CONTAINER_TYPES {
        let (count, _) = snapshot.components_by_type(&branch_id, component_type, 0, 0)?;
        if count > 0 {
            return Ok(None);
        }
    }
    let derived_depth = metadata.derived_depth_branches.contains(&branch_id);
    let range_start = crate::pact_file::index::IndexKey::MessageOrder {
        branch: branch_id.clone(),
        order: 0,
    };
    let range_end = crate::pact_file::index::IndexKey::MessageOrder {
        branch: branch_id.clone(),
        order: u64::MAX,
    };
    let (total_rows, _) = snapshot.index_range_window(&range_start, &range_end, 0, 0)?;
    let mut selected = Vec::new();
    if let Some(position) = total_rows
        .checked_sub(1)
        .and_then(|last| last.checked_sub(depth))
    {
        let (_, rows) = snapshot.index_range_window(&range_start, &range_end, position, 1)?;
        let Some((_, component_id)) = rows.into_iter().next() else {
            return Err(PactError::corrupt(
                "Pact message-order window returned no row inside its count",
            ));
        };
        let component_id = String::from_utf8(component_id)
            .map_err(|_| PactError::corrupt("Pact message component id is not UTF-8"))?;
        let Some(turn) = snapshot.component(&branch_id, &component_id)? else {
            return Err(PactError::corrupt(
                "Pact message-order index references a missing component",
            ));
        };
        let value = &turn.value;
        let is_root_child = value
            .get("parent_id")
            .and_then(Value::as_str)
            .is_some_and(|parent| parent == metadata.root_id);
        let archived = value
            .get("aux")
            .and_then(Value::as_object)
            .is_some_and(|aux| aux.contains_key("deleted_at"));
        let recorded_depth = value
            .get("pact_depth")
            .and_then(Value::as_i64)
            .and_then(|depth| i32::try_from(depth).ok());
        if !is_root_child || archived || (!derived_depth && recorded_depth != Some(anchor.depth)) {
            return Ok(None);
        }
        selected.push(turn.value);
    }
    let match_count = selected.len();
    let skipped_matches = parsed.controls.offset.min(match_count);
    let hit_limit = query.limit.min(parsed.controls.limit.unwrap_or(usize::MAX));
    let hits = selected
        .into_iter()
        .skip(skipped_matches)
        .take(hit_limit)
        .map(|value| indexed_selector_hit(value, &branch_id))
        .collect::<PactResult<Vec<_>>>()?;
    let stats = PactSelectorStreamStats {
        records_seen: current_count,
        records_temporal_rejected: current_count,
        records_selector_rejected: current_count.saturating_sub(hits.len()),
        payloads_touched: 0,
        source: "stream".to_string(),
        fallback_used: false,
    };
    Ok(Some(PactFileSelectorResult {
        schema: format!("{PACT_FILE_SCHEMA}.selector.v1"),
        query,
        temporal: PactTemporalSelectorReport {
            raw: parsed.temporal.raw.clone(),
            normalized: normalize_temporal(&parsed.temporal),
            cycles: Vec::new(),
        },
        source: "stream".to_string(),
        fallback_used: false,
        hits,
        selector_stats: Some(stats),
    }))
}

fn indexed_selector_hit(value: Value, branch_id: &str) -> PactResult<PactSelectorHit> {
    let object = value
        .as_object()
        .ok_or_else(|| PactError::corrupt("indexed Pact component is not an object"))?;
    let component_id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PactError::corrupt("indexed Pact component has no string id"))?
        .to_string();
    let node_type = object
        .get("componentType")
        .and_then(Value::as_str)
        .ok_or_else(|| PactError::corrupt("indexed Pact component has no string componentType"))?;
    let element_kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(node_type)
        .to_string();
    let parent_id = object
        .get("parent_id")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| PactError::corrupt("indexed Pact parent_id is not a string"))
        })
        .transpose()?;
    Ok(PactSelectorHit {
        component_id,
        branch_id: branch_id.to_string(),
        cycle: None,
        revision: None,
        time_ref: None,
        element_kind,
        parent_id,
        resident_in_current_surface: true,
        archived_in_pact: true,
    })
}

fn indexed_seed_components(
    snapshot: &crate::pact_file::snapshot::Snapshot,
    branch_id: &str,
    seed: &SelectorIndexSeed,
    offset: usize,
    limit: usize,
) -> PactResult<(usize, Vec<crate::pact_file::index::IndexedComponent>)> {
    match seed {
        SelectorIndexSeed::Type(component_type) => {
            snapshot.components_by_type(branch_id, component_type, offset, limit)
        }
        SelectorIndexSeed::Attribute { key, value } => {
            snapshot.components_by_aux(branch_id, key, &Value::String(value.clone()), offset, limit)
        }
        SelectorIndexSeed::Id(component_id) => {
            let component = snapshot.component(branch_id, component_id)?;
            let total = usize::from(component.is_some());
            let components = if offset == 0 && limit > 0 {
                component.into_iter().collect()
            } else {
                Vec::new()
            };
            Ok((total, components))
        }
    }
}

pub(super) fn select_records(
    records: &[ScannedRecord],
    mut query: PactFileSelectorQuery,
) -> PactResult<PactFileSelectorResult> {
    query.limit = query.limit.clamp(1, 1000);
    let order = SelectorOrder::from_str(&query.order)?;
    query.order = order.as_str().to_string();
    let options = SelectorStreamOptions {
        branch_id: query.branch_id.clone(),
        limit: query.limit,
        order,
    };
    let report = run_selector_stream(records, &query.selector, &options)?;
    let source = report.stats.source.clone();
    let fallback_used = report.stats.fallback_used;
    Ok(PactFileSelectorResult {
        schema: format!("{PACT_FILE_SCHEMA}.selector.v1"),
        query,
        temporal: report.temporal,
        source,
        fallback_used,
        hits: report.hits,
        selector_stats: Some(report.stats),
    })
}

pub(super) fn stream_selected_nodes<F>(
    records: &[ScannedRecord],
    selector: &str,
    options: &SelectorStreamOptions,
    stats: &mut PactSelectorStreamStats,
    callback: F,
) -> PactResult<CorpusSelectorPlan>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    let parsed = parse_corpus_selector(selector)?;
    let plan = compile_corpus_selector_plan(&parsed.structural_selector)?;
    stats.source = match plan {
        CorpusSelectorPlan::NodeLocal => "stream",
        CorpusSelectorPlan::SnapshotWindow => "snapshot_window",
    }
    .to_string();
    for_each_selected_node(records, &parsed, &plan, options, stats, callback)?;
    Ok(plan)
}

pub(super) fn make_selector_stream_options(
    branch_id: Option<String>,
    limit: usize,
    order: &str,
) -> PactResult<SelectorStreamOptions> {
    let order = SelectorOrder::from_str(order)?;
    Ok(SelectorStreamOptions {
        branch_id,
        limit: limit.clamp(1, 1000),
        order,
    })
}

fn run_selector_stream(
    records: &[ScannedRecord],
    selector: &str,
    options: &SelectorStreamOptions,
) -> PactResult<SelectorStreamReport> {
    let parsed = parse_corpus_selector(selector)?;
    let plan = compile_corpus_selector_plan(&parsed.structural_selector)?;
    let source_label = match plan {
        CorpusSelectorPlan::NodeLocal => "stream",
        CorpusSelectorPlan::SnapshotWindow => "snapshot_window",
    };
    let mut stats = PactSelectorStreamStats {
        records_seen: 0,
        records_temporal_rejected: 0,
        records_selector_rejected: 0,
        payloads_touched: 0,
        source: source_label.to_string(),
        fallback_used: false,
    };
    let root_store = ledger_from_records(records)?;
    let effective_branch_id = options
        .branch_id
        .as_deref()
        .unwrap_or(&root_store.current_branch_id);
    let temporal = build_temporal_report(records, &parsed, effective_branch_id)?;
    let mut hits = Vec::new();
    for_each_selected_node(records, &parsed, &plan, options, &mut stats, |selected| {
        let hit = selector_hit_from_selected(selected);
        hits.push(hit);
        if hits.len() >= options.limit {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })?;
    Ok(SelectorStreamReport {
        hits,
        stats,
        temporal,
    })
}

pub(super) fn for_each_selected_node<F>(
    records: &[ScannedRecord],
    parsed: &ParsedCorpusSelector,
    plan: &CorpusSelectorPlan,
    options: &SelectorStreamOptions,
    stats: &mut PactSelectorStreamStats,
    mut callback: F,
) -> PactResult<()>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    let root_store = ledger_from_records(records)?;
    let current_branch_id = options
        .branch_id
        .clone()
        .unwrap_or_else(|| root_store.current_branch_id.clone());

    let current_branch = root_store
        .branches
        .get(&current_branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {current_branch_id}")))?;
    let cycles =
        resolve_temporal_cycles(records, &current_branch_id, &parsed.temporal, options.order)?;
    let temporal_includes_current_surface = matches!(
        parsed.temporal.start,
        TemporalPoint::Current | TemporalPoint::History
    );
    if cycles.is_empty() && !temporal_includes_current_surface {
        return Ok(());
    }
    let current_head_cycle = head_cycle_for_branch(records, &current_branch_id);
    let current_records = current_branch.current.component_records();
    let current_ids: BTreeSet<String> = current_records.keys().cloned().collect();
    let archived_ids: BTreeSet<String> = records
        .iter()
        .filter(|record| record.kind == "node")
        .filter_map(|record| record.payload.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let commit_branch_by_node = commit_branch_by_node(records);
    let revision_by_node = revision_by_node(records);
    let revision_by_node_cycle = revision_by_node_cycle(records);
    let revision_lookup = RevisionLookup {
        by_node_cycle: &revision_by_node_cycle,
        by_node: &revision_by_node,
    };

    match plan {
        CorpusSelectorPlan::NodeLocal => {
            iterate_node_local(
                records,
                parsed,
                options,
                &cycles,
                &current_ids,
                &archived_ids,
                &commit_branch_by_node,
                &revision_lookup,
                current_head_cycle,
                &current_branch_id,
                current_branch,
                stats,
                &mut callback,
            )?;
        }
        CorpusSelectorPlan::SnapshotWindow => {
            iterate_snapshot_window(
                records,
                parsed,
                options,
                &cycles,
                &current_ids,
                &archived_ids,
                &commit_branch_by_node,
                &revision_lookup,
                current_head_cycle,
                &current_branch_id,
                current_branch,
                stats,
                &mut callback,
            )?;
        }
    }
    Ok(())
}

struct RevisionLookup<'a> {
    by_node_cycle: &'a BTreeMap<(String, u64), u64>,
    by_node: &'a BTreeMap<String, u64>,
}

impl RevisionLookup<'_> {
    fn revision_for(&self, component_id: &str, cycle: Option<u64>) -> Option<u64> {
        if let Some(cycle) = cycle
            && let Some(value) = self.by_node_cycle.get(&(component_id.to_string(), cycle))
        {
            return Some(*value);
        }
        self.by_node.get(component_id).copied()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StreamControlState {
    skipped: usize,
    emitted: usize,
}

impl StreamControlState {
    fn should_emit(&mut self, controls: &ParsedSelectorControls) -> bool {
        if self.skipped < controls.offset {
            self.skipped += 1;
            return false;
        }
        if let Some(limit) = controls.limit
            && self.emitted >= limit
        {
            return false;
        }
        self.emitted += 1;
        true
    }

    fn is_exhausted(&self, controls: &ParsedSelectorControls, query_limit: usize) -> bool {
        let control_limit = controls.limit.unwrap_or(usize::MAX);
        self.emitted >= query_limit.min(control_limit)
    }
}

#[allow(clippy::too_many_arguments)]
fn iterate_node_local<F>(
    records: &[ScannedRecord],
    parsed: &ParsedCorpusSelector,
    options: &SelectorStreamOptions,
    cycles: &[u64],
    current_ids: &BTreeSet<String>,
    archived_ids: &BTreeSet<String>,
    commit_branch_by_node: &BTreeMap<String, String>,
    revision_lookup: &RevisionLookup,
    current_head_cycle: Option<u64>,
    current_branch_id: &str,
    current_branch: &PactBranch,
    stats: &mut PactSelectorStreamStats,
    callback: &mut F,
) -> PactResult<()>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    let cycle_set: BTreeSet<u64> = cycles.iter().copied().collect();
    let include_current = match &parsed.temporal.start {
        TemporalPoint::Current | TemporalPoint::History => true,
        _ => current_head_cycle
            .map(|head| cycle_set.contains(&head))
            .unwrap_or(false),
    };
    let temporal_includes_history = matches!(parsed.temporal.start, TemporalPoint::History);
    let mut emitted: BTreeSet<(String, Option<u64>, Option<u64>)> = BTreeSet::new();
    let mut controls_state = StreamControlState::default();

    let mut by_cycle: BTreeMap<Option<u64>, Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if record.kind != "node" {
            continue;
        }
        let Some((id, cycle)) = node_record_id_and_cycle(record) else {
            continue;
        };
        let source_branch = commit_branch_by_node
            .get(&id)
            .cloned()
            .unwrap_or_else(|| current_branch_id.to_string());
        if source_branch != current_branch_id {
            continue;
        }
        match cycle {
            Some(c) => {
                if cycle_set.contains(&c) {
                    by_cycle.entry(Some(c)).or_default().push(index);
                } else {
                    stats.records_temporal_rejected += 1;
                }
            }
            None => {
                if temporal_includes_history {
                    by_cycle.entry(None).or_default().push(index);
                } else {
                    stats.records_temporal_rejected += 1;
                }
            }
        }
    }

    let current_records = current_branch.current.component_records();
    let mut current_by_cycle: BTreeMap<Option<u64>, Vec<&ComponentRecord>> = BTreeMap::new();
    if include_current {
        for node in current_records.values() {
            let node_cycle = node_cycle_from_record(node).or(current_head_cycle);
            if let Some(cycle) = node_cycle
                && !temporal_includes_history
                && !cycle_set.is_empty()
                && !cycle_set.contains(&cycle)
            {
                continue;
            }
            current_by_cycle.entry(node_cycle).or_default().push(node);
        }
    }

    let include_no_cycle_bucket = temporal_includes_history || current_by_cycle.contains_key(&None);
    let mut effective_cycle_set = cycle_set.clone();

    if include_current && (effective_cycle_set.is_empty() || temporal_includes_history) {
        for key in current_by_cycle.keys().copied().flatten() {
            effective_cycle_set.insert(key);
        }
    }
    let cycle_order =
        ordered_cycle_keys(&effective_cycle_set, options.order, include_no_cycle_bucket);

    for cycle_key in cycle_order {
        let processed = match options.order {
            SelectorOrder::Newest => process_cycle_bucket_newest(
                cycle_key,
                records,
                parsed,
                options,
                current_head_cycle,
                current_branch_id,
                current_ids,
                archived_ids,
                commit_branch_by_node,
                revision_lookup,
                &by_cycle,
                &current_by_cycle,
                stats,
                &mut emitted,
                &mut controls_state,
                callback,
            )?,
            SelectorOrder::Oldest => process_cycle_bucket_oldest(
                cycle_key,
                records,
                parsed,
                options,
                current_head_cycle,
                current_branch_id,
                current_ids,
                archived_ids,
                commit_branch_by_node,
                revision_lookup,
                &by_cycle,
                &current_by_cycle,
                stats,
                &mut emitted,
                &mut controls_state,
                callback,
            )?,
        };
        if matches!(processed, ControlFlow::Break(())) {
            return Ok(());
        }
        if controls_state.is_exhausted(&parsed.controls, options.limit) {
            return Ok(());
        }
    }
    Ok(())
}

fn ordered_cycle_keys(
    cycle_set: &BTreeSet<u64>,
    order: SelectorOrder,
    include_no_cycle_bucket: bool,
) -> Vec<Option<u64>> {
    let mut keys: Vec<Option<u64>> = cycle_set.iter().copied().map(Some).collect();
    keys.sort();
    if matches!(order, SelectorOrder::Newest) {
        keys.reverse();
    }
    if include_no_cycle_bucket {
        keys.push(None);
    }
    keys
}

#[allow(clippy::too_many_arguments)]
fn process_cycle_bucket_newest<F>(
    cycle_key: Option<u64>,
    records: &[ScannedRecord],
    parsed: &ParsedCorpusSelector,
    options: &SelectorStreamOptions,
    current_head_cycle: Option<u64>,
    current_branch_id: &str,
    current_ids: &BTreeSet<String>,
    archived_ids: &BTreeSet<String>,
    commit_branch_by_node: &BTreeMap<String, String>,
    revision_lookup: &RevisionLookup,
    by_cycle: &BTreeMap<Option<u64>, Vec<usize>>,
    current_by_cycle: &BTreeMap<Option<u64>, Vec<&ComponentRecord>>,
    stats: &mut PactSelectorStreamStats,
    emitted: &mut BTreeSet<(String, Option<u64>, Option<u64>)>,
    controls_state: &mut StreamControlState,
    callback: &mut F,
) -> PactResult<ControlFlow<()>>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    if Some(current_head_cycle).flatten() == cycle_key
        && let Some(nodes) = current_by_cycle.get(&cycle_key)
    {
        for node in nodes.iter().rev() {
            let item = NodeStreamItem {
                node: (*node).clone(),
                cycle: cycle_key,
                from_current_surface: true,
                source_branch_id: current_branch_id.to_string(),
            };
            if matches!(
                emit_stream_item(
                    item,
                    parsed,
                    options,
                    current_head_cycle,
                    current_ids,
                    archived_ids,
                    revision_lookup,
                    stats,
                    emitted,
                    controls_state,
                    callback,
                )?,
                ControlFlow::Break(())
            ) {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
    if let Some(indices) = by_cycle.get(&cycle_key) {
        for &record_index in indices.iter().rev() {
            let record = &records[record_index];
            let Some(node) = corpus_node_from_record_value(record.payload.clone()) else {
                continue;
            };
            let item = NodeStreamItem {
                cycle: cycle_key.or_else(|| node_cycle_from_record(&node)),
                from_current_surface: false,
                source_branch_id: commit_branch_by_node
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_else(|| current_branch_id.to_string()),
                node,
            };
            if matches!(
                emit_stream_item(
                    item,
                    parsed,
                    options,
                    current_head_cycle,
                    current_ids,
                    archived_ids,
                    revision_lookup,
                    stats,
                    emitted,
                    controls_state,
                    callback,
                )?,
                ControlFlow::Break(())
            ) {
                return Ok(ControlFlow::Break(()));
            }
        }
    }

    if Some(current_head_cycle).flatten() != cycle_key
        && let Some(nodes) = current_by_cycle.get(&cycle_key)
    {
        for node in nodes.iter() {
            let item = NodeStreamItem {
                node: (*node).clone(),
                cycle: cycle_key,
                from_current_surface: true,
                source_branch_id: current_branch_id.to_string(),
            };
            if matches!(
                emit_stream_item(
                    item,
                    parsed,
                    options,
                    current_head_cycle,
                    current_ids,
                    archived_ids,
                    revision_lookup,
                    stats,
                    emitted,
                    controls_state,
                    callback,
                )?,
                ControlFlow::Break(())
            ) {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
    Ok(ControlFlow::Continue(()))
}

#[allow(clippy::too_many_arguments)]
fn process_cycle_bucket_oldest<F>(
    cycle_key: Option<u64>,
    records: &[ScannedRecord],
    parsed: &ParsedCorpusSelector,
    options: &SelectorStreamOptions,
    current_head_cycle: Option<u64>,
    current_branch_id: &str,
    current_ids: &BTreeSet<String>,
    archived_ids: &BTreeSet<String>,
    commit_branch_by_node: &BTreeMap<String, String>,
    revision_lookup: &RevisionLookup,
    by_cycle: &BTreeMap<Option<u64>, Vec<usize>>,
    current_by_cycle: &BTreeMap<Option<u64>, Vec<&ComponentRecord>>,
    stats: &mut PactSelectorStreamStats,
    emitted: &mut BTreeSet<(String, Option<u64>, Option<u64>)>,
    controls_state: &mut StreamControlState,
    callback: &mut F,
) -> PactResult<ControlFlow<()>>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    if let Some(indices) = by_cycle.get(&cycle_key) {
        for &record_index in indices.iter() {
            let record = &records[record_index];
            let Some(node) = corpus_node_from_record_value(record.payload.clone()) else {
                continue;
            };
            let item = NodeStreamItem {
                cycle: cycle_key.or_else(|| node_cycle_from_record(&node)),
                from_current_surface: false,
                source_branch_id: commit_branch_by_node
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_else(|| current_branch_id.to_string()),
                node,
            };
            if matches!(
                emit_stream_item(
                    item,
                    parsed,
                    options,
                    current_head_cycle,
                    current_ids,
                    archived_ids,
                    revision_lookup,
                    stats,
                    emitted,
                    controls_state,
                    callback,
                )?,
                ControlFlow::Break(())
            ) {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
    if let Some(nodes) = current_by_cycle.get(&cycle_key) {
        for node in nodes.iter() {
            let item = NodeStreamItem {
                node: (*node).clone(),
                cycle: cycle_key,
                from_current_surface: true,
                source_branch_id: current_branch_id.to_string(),
            };
            if matches!(
                emit_stream_item(
                    item,
                    parsed,
                    options,
                    current_head_cycle,
                    current_ids,
                    archived_ids,
                    revision_lookup,
                    stats,
                    emitted,
                    controls_state,
                    callback,
                )?,
                ControlFlow::Break(())
            ) {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
    Ok(ControlFlow::Continue(()))
}

#[allow(clippy::too_many_arguments)]
fn emit_stream_item<F>(
    item: NodeStreamItem,
    parsed: &ParsedCorpusSelector,
    options: &SelectorStreamOptions,
    current_head_cycle: Option<u64>,
    current_ids: &BTreeSet<String>,
    archived_ids: &BTreeSet<String>,
    revision_lookup: &RevisionLookup,
    stats: &mut PactSelectorStreamStats,
    emitted: &mut BTreeSet<(String, Option<u64>, Option<u64>)>,
    controls_state: &mut StreamControlState,
    callback: &mut F,
) -> PactResult<ControlFlow<()>>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    stats.records_seen += 1;
    let node_value = serde_json::to_value(&item.node).unwrap_or(Value::Null);
    let selector_matches =
        match_node_value(&node_value, &parsed.structural_selector_without_controls)?;
    if !selector_matches {
        stats.records_selector_rejected += 1;
        return Ok(ControlFlow::Continue(()));
    }
    let revision = revision_lookup.revision_for(&item.node.id, item.cycle);
    let dedup_key = (item.node.id.clone(), item.cycle, revision);
    if !emitted.insert(dedup_key) {
        return Ok(ControlFlow::Continue(()));
    }
    if !controls_state.should_emit(&parsed.controls) {
        return Ok(ControlFlow::Continue(()));
    }
    let resident_in_current_surface = item.from_current_surface
        || (current_head_cycle == item.cycle && current_ids.contains(&item.node.id));
    let archived_in_pact = archived_ids.contains(&item.node.id);
    let selected = SelectedComponent {
        cycle: item.cycle,
        revision,
        branch_id: item.source_branch_id.clone(),
        resident_in_current_surface,
        archived_in_pact,
        time_ref: time_ref_for_cycle(item.cycle, current_head_cycle),
        node: item.node,
    };
    let outcome = callback(&selected);
    if matches!(outcome, ControlFlow::Break(())) {
        return Ok(ControlFlow::Break(()));
    }
    if controls_state.is_exhausted(&parsed.controls, options.limit) {
        return Ok(ControlFlow::Break(()));
    }
    Ok(ControlFlow::Continue(()))
}

fn node_record_id_and_cycle(record: &ScannedRecord) -> Option<(String, Option<u64>)> {
    let object = record.payload.as_object()?;
    let id = object.get("id").and_then(Value::as_str)?.to_string();
    let cycle = object
        .get("aux")
        .and_then(Value::as_object)
        .and_then(|aux| {
            aux.get("cycle")
                .and_then(Value::as_u64)
                .or_else(|| aux.get("pact_cycle").and_then(Value::as_u64))
        });
    Some((id, cycle))
}

#[allow(clippy::too_many_arguments)]
fn iterate_snapshot_window<F>(
    records: &[ScannedRecord],
    parsed: &ParsedCorpusSelector,
    options: &SelectorStreamOptions,
    cycles: &[u64],
    current_ids: &BTreeSet<String>,
    archived_ids: &BTreeSet<String>,
    commit_branch_by_node: &BTreeMap<String, String>,
    revision_lookup: &RevisionLookup,
    current_head_cycle: Option<u64>,
    current_branch_id: &str,
    current_branch: &PactBranch,
    stats: &mut PactSelectorStreamStats,
    callback: &mut F,
) -> PactResult<()>
where
    F: FnMut(&SelectedComponent) -> ControlFlow<()>,
{
    let cycles_in_order = order_cycles(cycles, options.order);
    let mut emitted: BTreeSet<(String, Option<u64>, Option<u64>)> = BTreeSet::new();
    let mut controls_state = StreamControlState::default();

    let temporal_includes_current_surface = matches!(
        parsed.temporal.start,
        TemporalPoint::Current | TemporalPoint::History
    );
    let root_store = ledger_from_records(records)?;
    let projection_branch = root_store
        .branches
        .get(current_branch_id)
        .unwrap_or(current_branch);

    if cycles_in_order.is_empty() && temporal_includes_current_surface {
        let tree =
            crate::store::project_branch_component_tree(&root_store, projection_branch, None)
                .map_err(|err| {
                    PactError::storage(format!("failed to project current branch: {err}"))
                })?;
        stats.payloads_touched = stats.payloads_touched.saturating_add(1);
        let matches = crate::selector::select_components_tree(
            &tree,
            &parsed.structural_selector_without_controls,
            true,
        )
        .map_err(|err| {
            PactError::selector(format!(
                "selector {:?} failed against current branch: {err}",
                parsed.structural_selector
            ))
        })?;
        for matched in matches {
            stats.records_seen += 1;
            let revision = revision_lookup.revision_for(&matched.id, current_head_cycle);
            let dedup_key = (matched.id.clone(), current_head_cycle, revision);
            if !emitted.insert(dedup_key) {
                continue;
            }
            if !controls_state.should_emit(&parsed.controls) {
                continue;
            }
            let node = current_branch
                .current
                .component_record(&matched.id)
                .unwrap_or_else(|| component_record_from_selected(&tree, &matched));
            let selected = SelectedComponent {
                cycle: current_head_cycle,
                revision,
                branch_id: commit_branch_by_node
                    .get(&matched.id)
                    .cloned()
                    .unwrap_or_else(|| current_branch_id.to_string()),
                resident_in_current_surface: current_ids.contains(&matched.id),
                archived_in_pact: archived_ids.contains(&matched.id),
                time_ref: time_ref_for_cycle(current_head_cycle, current_head_cycle),
                node,
            };
            match callback(&selected) {
                ControlFlow::Break(_) => return Ok(()),
                ControlFlow::Continue(_) => {}
            }
            if controls_state.is_exhausted(&parsed.controls, options.limit) {
                return Ok(());
            }
        }
        return Ok(());
    }

    for cycle in cycles_in_order {
        let (tree, _) = crate::store::project_branch_archive_component_tree(
            &root_store,
            projection_branch,
            Some(cycle),
        )
        .map_err(|err| {
            PactError::storage(format!(
                "failed to project branch {current_branch_id:?} archive at cycle {cycle}: {err}"
            ))
        })?;
        stats.payloads_touched = stats.payloads_touched.saturating_add(1);
        let matches = crate::selector::select_components_tree(
            &tree,
            &parsed.structural_selector_without_controls,
            true,
        )
        .map_err(|err| {
            PactError::selector(format!(
                "selector {:?} failed against cycle {cycle}: {err}",
                parsed.structural_selector
            ))
        })?;
        for matched in matches {
            stats.records_seen += 1;
            let node = component_record_from_selected(&tree, &matched);
            if let Some(node_cycle) = node_cycle_from_record(&node)
                && node_cycle != cycle
                && !component_subtree_has_cycle(&tree, &matched.id, cycle)
            {
                continue;
            }
            let revision = revision_lookup.revision_for(&matched.id, Some(cycle));
            let dedup_key = (matched.id.clone(), Some(cycle), revision);
            if !emitted.insert(dedup_key) {
                continue;
            }
            if !controls_state.should_emit(&parsed.controls) {
                continue;
            }
            let resident_in_current_surface =
                current_head_cycle == Some(cycle) && current_ids.contains(&matched.id);
            let archived_in_pact = archived_ids.contains(&matched.id);
            let selected = SelectedComponent {
                cycle: Some(cycle),
                revision,
                branch_id: commit_branch_by_node
                    .get(&matched.id)
                    .cloned()
                    .unwrap_or_else(|| current_branch_id.to_string()),
                resident_in_current_surface,
                archived_in_pact,
                time_ref: time_ref_for_cycle(Some(cycle), current_head_cycle),
                node,
            };
            match callback(&selected) {
                ControlFlow::Break(_) => return Ok(()),
                ControlFlow::Continue(_) => {}
            }
            if controls_state.is_exhausted(&parsed.controls, options.limit) {
                return Ok(());
            }
        }
    }
    Ok(())
}

struct NodeStreamItem {
    node: ComponentRecord,
    cycle: Option<u64>,
    from_current_surface: bool,
    source_branch_id: String,
}

fn order_cycles(cycles: &[u64], order: SelectorOrder) -> Vec<u64> {
    let mut sorted: Vec<u64> = cycles.to_vec();
    sorted.sort();
    if matches!(order, SelectorOrder::Newest) {
        sorted.reverse();
    }
    sorted
}

fn component_record_from_selected(
    tree: &crate::tree::ComponentTree,
    selected: &crate::selector::SelectedComponent,
) -> ComponentRecord {
    if let Some(component) = tree.component(&selected.id) {
        return ComponentRecord::from_component_with_parent(component, selected.parent_id.clone());
    }
    synth_node_from_selected(selected)
}

fn component_subtree_has_cycle(
    tree: &crate::tree::ComponentTree,
    component_id: &str,
    cycle: u64,
) -> bool {
    let Some(component) = tree.component(component_id) else {
        return false;
    };
    component_has_cycle(component, cycle)
}

fn component_has_cycle(component: &crate::components::Component, cycle: u64) -> bool {
    let record =
        ComponentRecord::from_component_with_parent(component, component.parent_id.clone());
    if node_cycle_from_record(&record) == Some(cycle) {
        return true;
    }
    component
        .children
        .iter()
        .any(|child| component_has_cycle(child, cycle))
}

fn synth_node_from_selected(selected: &crate::selector::SelectedComponent) -> ComponentRecord {
    ComponentRecord {
        id: selected.id.clone(),
        node_type: selected.component_type.clone(),
        kind: selected.kind.clone(),
        role: selected.role.clone(),
        parent_id: selected.parent_id.clone(),
        offset: selected.offset as i64,
        pact_depth: selected.pact_depth.map(|d| d as i64),
        created_at_ns: 0,
        creation_index: 0,
        ttl: selected.ttl,
        cad: selected.cad,
        lifecycle: crate::components::ComponentLifecycle::default(),
        key: selected.key.clone(),
        tags: selected.tags.clone(),
        child_ids: Vec::new(),

        xml_attrs: selected.aux.clone().into_iter().collect(),
        aux: BTreeMap::new(),
        content: selected.content.clone(),
        content_ref: selected
            .content_ref
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_string),
        content_preview: None,
        tool_call_id: selected.tool_call_id.clone(),
        tool_name: selected.tool_name.clone(),
        parameters: selected.parameters.clone(),
        parameters_ref: None,
        success: selected.success,
        result: selected.result.clone(),
        result_ref: None,
        error_message: selected.error_message.clone(),
        error_ref: None,
        content_capture: None,
        source_ref: None,
        content_status: None,
    }
}

fn selector_hit_from_selected(selected: &SelectedComponent) -> PactSelectorHit {
    PactSelectorHit {
        component_id: selected.node.id.clone(),
        branch_id: selected.branch_id.clone(),
        cycle: selected.cycle,
        revision: selected.revision,
        time_ref: selected.time_ref.clone(),
        element_kind: selected
            .node
            .kind
            .clone()
            .unwrap_or_else(|| selected.node.node_type.clone()),
        parent_id: selected.node.parent_id.clone(),
        resident_in_current_surface: selected.resident_in_current_surface,
        archived_in_pact: selected.archived_in_pact,
    }
}

fn build_temporal_report(
    records: &[ScannedRecord],
    parsed: &ParsedCorpusSelector,
    branch_id: &str,
) -> PactResult<PactTemporalSelectorReport> {
    let cycles =
        resolve_temporal_cycles(records, branch_id, &parsed.temporal, SelectorOrder::Newest)?;
    Ok(PactTemporalSelectorReport {
        raw: parsed.temporal.raw.clone(),
        normalized: normalize_temporal(&parsed.temporal),
        cycles,
    })
}

fn normalize_temporal(temporal: &TemporalRange) -> String {
    let start = render_point(&temporal.start);
    if let Some(end) = temporal.end.as_ref() {
        format!("{start}..{}", render_point(end))
    } else {
        start
    }
}

fn render_point(point: &TemporalPoint) -> String {
    match point {
        TemporalPoint::Current => "@t0".to_string(),
        TemporalPoint::Relative(n) => format!("@t{n}"),
        TemporalPoint::Cycle(n) => format!("@c{n}"),
        TemporalPoint::History => "@history".to_string(),
    }
}

fn time_ref_for_cycle(cycle: Option<u64>, current_head_cycle: Option<u64>) -> Option<String> {
    let cycle = cycle?;
    if Some(cycle) == current_head_cycle {
        Some("@t0".to_string())
    } else {
        Some(format!("@c{cycle}"))
    }
}

pub(super) fn selector_reaches_prefork_history(selector: &str) -> PactResult<bool> {
    let parsed = parse_corpus_selector(selector)?;
    let reaches = matches!(parsed.temporal.start, TemporalPoint::History)
        || matches!(parsed.temporal.end, Some(TemporalPoint::History));
    Ok(reaches)
}

fn parse_corpus_selector(input: &str) -> PactResult<ParsedCorpusSelector> {
    let trimmed = input.trim_start();
    let (temporal, rest) = parse_temporal_prefix(trimmed)?;
    let structural = rest.trim().to_string();
    if structural.is_empty() {
        return Err(PactError::invalid(
            "selector requires a structural selector after the temporal prefix",
        ));
    }
    let (clean_structural, controls) = parse_selector_controls(&structural)?;
    if clean_structural.is_empty() {
        return Err(PactError::invalid(
            "selector requires a structural selector after the temporal prefix",
        ));
    }
    Ok(ParsedCorpusSelector {
        temporal,
        structural_selector: structural,
        structural_selector_without_controls: clean_structural,
        controls,
    })
}

fn parse_temporal_prefix(input: &str) -> PactResult<(TemporalRange, &str)> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('@') {
        return Ok((
            TemporalRange {
                raw: "@t0".to_string(),
                start: TemporalPoint::Current,
                end: None,
            },
            trimmed,
        ));
    }
    if let Some(rest) = trimmed.strip_prefix("@history") {
        if peek_temporal_continuation(rest) {
            return Err(PactError::invalid(format!(
                "@history cannot participate in a temporal range; got {trimmed:?}"
            )));
        }
        return Ok((
            TemporalRange {
                raw: "@history".to_string(),
                start: TemporalPoint::History,
                end: None,
            },
            rest,
        ));
    }
    if let Some(rest) = trimmed.strip_prefix("@*") {
        if peek_temporal_continuation(rest) {
            return Err(PactError::invalid(format!(
                "@* cannot participate in a temporal range; got {trimmed:?}"
            )));
        }
        return Ok((
            TemporalRange {
                raw: "@*".to_string(),
                start: TemporalPoint::History,
                end: None,
            },
            rest,
        ));
    }
    let (start, after_start) = parse_temporal_point(trimmed)?;
    if let Some(after_separator) = strip_temporal_separator(after_start) {
        let (end, after_end) = parse_temporal_point(after_separator)?;
        if !same_temporal_axis(&start, &end) {
            return Err(PactError::invalid(format!(
                "temporal range mixes axes which is not supported: {trimmed:?}"
            )));
        }
        let raw = trimmed[..trimmed.len() - after_end.len()].to_string();
        return Ok((
            TemporalRange {
                raw,
                start,
                end: Some(end),
            },
            after_end,
        ));
    }
    let raw = trimmed[..trimmed.len() - after_start.len()].to_string();
    Ok((
        TemporalRange {
            raw,
            start,
            end: None,
        },
        after_start,
    ))
}

fn parse_temporal_point(input: &str) -> PactResult<(TemporalPoint, &str)> {
    if let Some(rest) = input.strip_prefix("@t") {
        if let Some(rest) = rest.strip_prefix("-") {
            let (digits, remainder) = take_digits(rest);
            if digits.is_empty() {
                return Err(PactError::invalid(format!(
                    "missing digits in temporal point {input:?}"
                )));
            }
            let value: i64 = digits
                .parse()
                .map_err(|_| PactError::invalid(format!("invalid relative temporal {input:?}")))?;
            return Ok((TemporalPoint::Relative(-value), remainder));
        }
        if let Some(rest) = rest.strip_prefix("+") {
            let _ = rest;
            return Err(PactError::invalid(format!(
                "positive relative temporal aliases are not accepted; use @t-N instead of {input:?}"
            )));
        }
        let (digits, remainder) = take_digits(rest);
        if digits.is_empty() {
            return Err(PactError::invalid(format!(
                "missing digits in temporal point {input:?}"
            )));
        }
        let value: u64 = digits
            .parse()
            .map_err(|_| PactError::invalid(format!("invalid temporal point {input:?}")))?;
        if value == 0 {
            return Ok((TemporalPoint::Current, remainder));
        }
        return Err(PactError::invalid(format!(
            "positive relative temporal aliases are not accepted; use @t-{value} or @t0 instead of {input:?}"
        )));
    }
    if let Some(rest) = input.strip_prefix("@c") {
        let (digits, remainder) = take_digits(rest);
        if digits.is_empty() {
            return Err(PactError::invalid(format!(
                "missing digits in absolute cycle {input:?}"
            )));
        }
        let value: u64 = digits
            .parse()
            .map_err(|_| PactError::invalid(format!("invalid cycle {input:?}")))?;
        return Ok((TemporalPoint::Cycle(value), remainder));
    }
    Err(PactError::invalid(format!(
        "unrecognized temporal token at start of {input:?}; expected @t0, @t-N, @cN, @history, or @*"
    )))
}

fn strip_temporal_separator(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();
    if let Some(rest) = trimmed.strip_prefix("..") {
        return Some(rest);
    }
    if let Some(rest) = trimmed.strip_prefix(':') {
        return Some(rest);
    }
    None
}

fn peek_temporal_continuation(after: &str) -> bool {
    strip_temporal_separator(after).is_some()
}

fn take_digits(input: &str) -> (&str, &str) {
    let end = input
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .last()
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    (&input[..end], &input[end..])
}

fn same_temporal_axis(start: &TemporalPoint, end: &TemporalPoint) -> bool {
    matches!(
        (start, end),
        (TemporalPoint::Cycle(_), TemporalPoint::Cycle(_))
            | (TemporalPoint::Relative(_), TemporalPoint::Relative(_))
            | (TemporalPoint::Current, TemporalPoint::Current)
            | (TemporalPoint::Relative(_), TemporalPoint::Current)
            | (TemporalPoint::Current, TemporalPoint::Relative(_))
    )
}

fn compile_corpus_selector_plan(structural_selector: &str) -> PactResult<CorpusSelectorPlan> {
    if selector_requires_structure(structural_selector)? {
        Ok(CorpusSelectorPlan::SnapshotWindow)
    } else {
        Ok(CorpusSelectorPlan::NodeLocal)
    }
}

fn resolve_temporal_cycles(
    records: &[ScannedRecord],
    branch_id: &str,
    temporal: &TemporalRange,
    order: SelectorOrder,
) -> PactResult<Vec<u64>> {
    let mut known_cycles = cycle_inventory_for_branch(records, branch_id);
    let head_cycle = head_cycle_for_branch(records, branch_id);
    if let Some(head) = head_cycle
        && !known_cycles.contains(&head)
    {
        known_cycles.push(head);
    }
    known_cycles.sort();
    known_cycles.dedup();

    let sealed_descending: Vec<u64> = {
        let mut sealed: Vec<u64> = known_cycles
            .iter()
            .copied()
            .filter(|c| Some(*c) != head_cycle)
            .collect();
        sealed.sort_by(|a, b| b.cmp(a));
        sealed
    };

    let mut cycles: Vec<u64> = match (&temporal.start, &temporal.end) {
        (TemporalPoint::History, None) => known_cycles.clone(),
        (TemporalPoint::Current, None) => head_cycle.map(|c| vec![c]).unwrap_or_default(),
        (TemporalPoint::Cycle(n), None) => {
            if known_cycles.contains(n) {
                vec![*n]
            } else {
                Vec::new()
            }
        }
        (TemporalPoint::Cycle(a), Some(TemporalPoint::Cycle(b))) => {
            let (lo, hi) = if a <= b { (*a, *b) } else { (*b, *a) };
            known_cycles
                .iter()
                .copied()
                .filter(|c| *c >= lo && *c <= hi)
                .collect()
        }
        (TemporalPoint::Relative(n), None) => {
            relative_to_cycles(*n, *n, head_cycle, &sealed_descending)
        }
        (TemporalPoint::Relative(a), Some(TemporalPoint::Relative(b))) => {
            relative_to_cycles(*a, *b, head_cycle, &sealed_descending)
        }
        (TemporalPoint::Current, Some(TemporalPoint::Relative(b))) => {
            relative_to_cycles(0, *b, head_cycle, &sealed_descending)
        }
        (TemporalPoint::Relative(a), Some(TemporalPoint::Current)) => {
            relative_to_cycles(*a, 0, head_cycle, &sealed_descending)
        }
        (TemporalPoint::Current, Some(TemporalPoint::Current)) => {
            head_cycle.map(|c| vec![c]).unwrap_or_default()
        }
        (TemporalPoint::History, Some(_)) | (_, Some(TemporalPoint::History)) => {
            return Err(PactError::invalid(
                "@history cannot be part of a temporal range",
            ));
        }
        (start, Some(end)) => {
            return Err(PactError::invalid(format!(
                "temporal range mixes axes which is not supported: {start:?}..{end:?}"
            )));
        }
    };

    cycles.sort();
    cycles.dedup();
    if matches!(order, SelectorOrder::Newest) {
        cycles.reverse();
    }
    Ok(cycles)
}

fn relative_to_cycles(
    a: i64,
    b: i64,
    head_cycle: Option<u64>,
    sealed_descending: &[u64],
) -> Vec<u64> {
    let mut result = Vec::new();
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    for n in lo..=hi {
        if let Some(cycle) = relative_to_cycle(n, head_cycle, sealed_descending) {
            result.push(cycle);
        }
    }
    result.sort();
    result.dedup();
    result
}

fn relative_to_cycle(n: i64, head_cycle: Option<u64>, sealed_descending: &[u64]) -> Option<u64> {
    if n == 0 {
        return head_cycle;
    }
    if n > 0 {
        return None;
    }
    let index = ((-n) - 1) as usize;
    sealed_descending.get(index).copied()
}

fn cycle_inventory_for_branch(records: &[ScannedRecord], branch_id: &str) -> Vec<u64> {
    let mut cycles: Vec<u64> = Vec::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        if commit.branch_id == branch_id {
            cycles.push(commit.cycle);
        }
    }
    for record in records.iter().filter(|record| record.kind == "head") {
        let Ok(head) = serde_json::from_value::<HeadRecord>(record.payload.clone()) else {
            continue;
        };
        if head.branch_id == branch_id {
            cycles.push(head.head_cycle);
        }
    }
    if cycles.is_empty() {
        cycles = cycle_ranges_from_records(records)
            .iter()
            .map(|range| range.cycle)
            .collect();
    }
    cycles.sort();
    cycles.dedup();
    cycles
}

fn head_cycle_for_branch(records: &[ScannedRecord], branch_id: &str) -> Option<u64> {
    let mut latest: Option<(u64, u64)> = None;
    for record in records.iter().filter(|record| record.kind == "head") {
        if let Ok(head) = serde_json::from_value::<HeadRecord>(record.payload.clone())
            && head.branch_id == branch_id
        {
            let key = (head.head_revision, head.head_cycle);
            if latest.map(|(rev, _)| key.0 >= rev).unwrap_or(true) {
                latest = Some(key);
            }
        }
    }
    if let Some((_, cycle)) = latest {
        return Some(cycle);
    }
    let mut highest = None;
    for record in records.iter().filter(|record| record.kind == "commit") {
        if let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone())
            && commit.branch_id == branch_id
            && highest.map(|c: u64| commit.cycle > c).unwrap_or(true)
        {
            highest = Some(commit.cycle);
        }
    }
    highest
}

fn commit_branch_by_node(records: &[ScannedRecord]) -> BTreeMap<String, String> {
    let mut by_node = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        for op in &commit.ops {
            for id in commit_op_node_ids(op) {
                by_node
                    .entry(id)
                    .or_insert_with(|| commit.branch_id.clone());
            }
        }
    }
    by_node
}

fn revision_by_node(records: &[ScannedRecord]) -> BTreeMap<String, u64> {
    let mut by_node = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        for op in &commit.ops {
            for id in commit_op_node_ids(op) {
                by_node
                    .entry(id)
                    .and_modify(|existing: &mut u64| {
                        if commit.revision > *existing {
                            *existing = commit.revision;
                        }
                    })
                    .or_insert(commit.revision);
            }
        }
    }
    by_node
}

fn revision_by_node_cycle(records: &[ScannedRecord]) -> BTreeMap<(String, u64), u64> {
    let mut by_node_cycle = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        for op in &commit.ops {
            for id in commit_op_node_ids(op) {
                by_node_cycle
                    .entry((id, commit.cycle))
                    .and_modify(|existing: &mut u64| {
                        if commit.revision > *existing {
                            *existing = commit.revision;
                        }
                    })
                    .or_insert(commit.revision);
            }
        }
    }
    by_node_cycle
}

fn commit_op_node_ids(op: &CommitRecordOp) -> Vec<String> {
    match op {
        CommitRecordOp::InsertComponent { node_ref, .. }
        | CommitRecordOp::ReplaceComponent { node_ref, .. } => vec![node_ref.clone()],
        CommitRecordOp::AppendComponents { node_refs, .. } => node_refs.clone(),
        CommitRecordOp::DeleteComponent { component_id }
        | CommitRecordOp::SetComponentAux { component_id, .. }
        | CommitRecordOp::UnsetComponentAux { component_id, .. }
        | CommitRecordOp::AttachContentRef { component_id, .. }
        | CommitRecordOp::ReplaceContentRange { component_id, .. } => vec![component_id.clone()],
    }
}

fn node_cycle_from_record(node: &ComponentRecord) -> Option<u64> {
    node.aux
        .get("cycle")
        .and_then(Value::as_u64)
        .or_else(|| node.aux.get("pact_cycle").and_then(Value::as_u64))
}

fn corpus_node_from_record_value(mut value: Value) -> Option<ComponentRecord> {
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

#[cfg(test)]
mod tests;
