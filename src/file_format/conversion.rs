use super::*;
use serde::de::{Error as _, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};

mod unsupported_pre_v01;

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
struct RawExportedRecord {
    sequence: u64,
    kind: String,
    payload_bytes: usize,
    payload: Box<RawValue>,
}

struct FilteredExportedRecords(Vec<ScannedRecord>);

impl<'de> Deserialize<'de> for FilteredExportedRecords {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RecordsVisitor;

        impl<'de> Visitor<'de> for RecordsVisitor {
            type Value = FilteredExportedRecords;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an array of exported Pact records")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut records = Vec::new();
                let mut previous_sequence = None;
                while let Some(record) = sequence.next_element::<RawExportedRecord>()? {
                    if previous_sequence.is_some_and(|previous| previous >= record.sequence) {
                        return Err(A::Error::custom(
                            "exported record sequences must be strictly increasing",
                        ));
                    }
                    previous_sequence = Some(record.sequence);
                    let payload = if record.kind == "trace" {
                        let trace: FilteredPublicTrace =
                            serde_json::from_str(record.payload.get()).map_err(A::Error::custom)?;
                        serde_json::to_value(crate::store::PactTransactionTrace::from(trace))
                            .map_err(A::Error::custom)?
                    } else {
                        serde_json::from_str(record.payload.get()).map_err(A::Error::custom)?
                    };
                    records.push(ScannedRecord {
                        kind: record.kind,
                        payload,
                        payload_bytes: record.payload_bytes,
                        sequence: record.sequence,
                    });
                }
                Ok(FilteredExportedRecords(records))
            }
        }

        deserializer.deserialize_seq(RecordsVisitor)
    }
}

#[derive(Deserialize)]
struct FilteredPublicTrace {
    #[serde(default = "default_transaction_trace_schema")]
    schema: String,
    root_id: String,
    branch_id: String,
    transaction_id: String,
    authored_ref: crate::store::PactRef,
    pre_request_ref: crate::store::PactRef,
    post_request_alteration_ref: crate::store::PactRef,
    #[serde(default)]
    request_ref: Option<crate::store::PactRef>,
    #[serde(default)]
    response_ref: Option<crate::store::PactRef>,
    #[serde(default)]
    post_response_alteration_ref: Option<crate::store::PactRef>,
    #[serde(default)]
    post_response_ref: Option<crate::store::PactRef>,
    #[serde(default)]
    alteration_ids: Vec<String>,
    #[serde(default)]
    commit: Option<crate::store::PactCommit>,
    #[serde(default)]
    replay_trace: Option<FilteredReplayTrace>,
    #[serde(default)]
    aux: BTreeMap<String, Value>,
}

impl From<FilteredPublicTrace> for crate::store::PactTransactionTrace {
    fn from(trace: FilteredPublicTrace) -> Self {
        Self {
            schema: trace.schema,
            root_id: trace.root_id,
            branch_id: trace.branch_id,
            transaction_id: trace.transaction_id,
            authored_ref: trace.authored_ref,
            pre_request_ref: trace.pre_request_ref,
            post_request_alteration_ref: trace.post_request_alteration_ref,
            request_ref: trace.request_ref,
            response_ref: trace.response_ref,
            post_response_alteration_ref: trace.post_response_alteration_ref,
            post_response_ref: trace.post_response_ref,
            alteration_ids: trace.alteration_ids,
            commit: trace.commit,
            replay_trace: trace.replay_trace.map(Into::into),
            aux: trace.aux,
        }
    }
}

fn default_transaction_trace_schema() -> String {
    "pact.transaction_trace.v1".to_string()
}

#[derive(Deserialize)]
struct FilteredReplayTrace {
    from_rev: u64,
    to_rev: u64,
    #[serde(default, alias = "node_deltas")]
    ops: FilteredTraceOps,
}

impl From<FilteredReplayTrace> for crate::transaction_trace::TransactionTrace {
    fn from(trace: FilteredReplayTrace) -> Self {
        Self {
            from_rev: trace.from_rev,
            to_rev: trace.to_rev,
            ops: trace.ops.0,
        }
    }
}

#[derive(Default)]
struct FilteredTraceOps(Vec<crate::transaction_trace::TraceOp>);

impl<'de> Deserialize<'de> for FilteredTraceOps {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OpsVisitor;

        impl<'de> Visitor<'de> for OpsVisitor {
            type Value = FilteredTraceOps;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an array of transaction trace operations")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut ops = Vec::new();
                while let Some(op) = sequence.next_element::<crate::transaction_trace::TraceOp>()? {
                    if !is_derived_projection_op(&op.kind) {
                        ops.push(op);
                    }
                }
                Ok(FilteredTraceOps(ops))
            }
        }

        deserializer.deserialize_seq(OpsVisitor)
    }
}

fn is_derived_projection_op(kind: &crate::transaction_trace::TraceOpKind) -> bool {
    use crate::transaction_trace::TraceOpKind;
    const DERIVED_FIELDS: &[&str] = &[
        "depth",
        "pact_depth",
        "child_ids",
        "visible_in_head",
        "first_seen",
        "last_seen",
        "deleted_at",
        "pact_lifecycle",
    ];
    const DERIVED_ATTRS: &[&str] = &[
        "visible_in_head",
        "first_seen",
        "last_seen",
        "deleted_at",
        "pact_lifecycle",
    ];
    match kind {
        TraceOpKind::SetField { key, .. } | TraceOpKind::UnsetField { key } => {
            DERIVED_FIELDS.contains(&key.as_str())
        }
        TraceOpKind::SetAttr { key, .. } | TraceOpKind::UnsetAttr { key } => {
            DERIVED_ATTRS.contains(&key.as_str())
        }
        _ => false,
    }
}

struct SemanticFingerprint {
    playback_rows: Vec<Value>,
    playback_digest: String,
    content_count: usize,
    content_digest: String,
}

pub fn file_convert_exported_records_json(
    exported_records_json: &str,
    output_path: &Path,
) -> PactResult<String> {
    file_convert_exported_records_json_owned(exported_records_json.to_string(), output_path)
}

pub fn file_convert_exported_records_json_owned(
    exported_records_json: String,
    output_path: &Path,
) -> PactResult<String> {
    file_convert_exported_records_reader(std::io::Cursor::new(exported_records_json), output_path)
}

pub fn file_convert_exported_records_reader(
    reader: impl Read,
    output_path: &Path,
) -> PactResult<String> {
    if output_path.exists() {
        return Err(PactError::invalid(format!(
            "conversion output already exists: {}",
            output_path.display()
        )));
    }

    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let FilteredExportedRecords(records) = FilteredExportedRecords::deserialize(&mut deserializer)
        .map_err(|error| PactError::codec(format!("invalid exported record JSON: {error}")))?;
    deserializer
        .end()
        .map_err(|error| PactError::codec(format!("invalid exported record JSON: {error}")))?;
    convert_pre_v01_records(records, output_path)
}

pub fn file_convert_pre_v01_file(source_path: &Path, output_path: &Path) -> PactResult<String> {
    if output_path.exists() {
        return Err(PactError::invalid(format!(
            "conversion output already exists: {}",
            output_path.display()
        )));
    }
    convert_pre_v01_records(
        unsupported_pre_v01::read_pre_v01_records(source_path)?,
        output_path,
    )
}

fn convert_pre_v01_records(records: Vec<ScannedRecord>, output_path: &Path) -> PactResult<String> {
    if records.is_empty() {
        return Err(PactError::invalid("exported record list is empty"));
    }

    let mut ledger = ledger_from_records_with_commits(&records, false)?;
    align_authoritative_trace_heads(&mut ledger)?;
    let source = semantic_fingerprint(&records, &ledger)?;
    drop(records);
    let candidate_ledger = normalize_imported_ledger(ledger)?;
    let candidate = semantic_fingerprint_from_ledger(&candidate_ledger)?;
    verify_fingerprints(&source, &candidate)?;
    let staged = write_cold_v2_baseline(output_path, &candidate_ledger)?;
    let cold_bytes = fs::read(staged.path())?;
    let physical_layout = crate::pact_file::layout_metrics::measure_committed_layout(&cold_bytes)?;
    let cold_ledger = crate::pact_file::open_ledger_bytes(&cold_bytes)?;
    let cold = semantic_fingerprint_from_ledger(&cold_ledger)?;
    verify_fingerprints(&source, &cold)?;
    drop(cold_ledger);
    drop(cold_bytes);

    let output_bytes = staged.as_file().metadata()?.len();
    crate::atomic_output::publish_staged(staged, output_path, PublishMode::CreateNew)?;
    let reopened_fingerprint = match fs::read(output_path)
        .map_err(PactError::from)
        .and_then(|bytes| crate::pact_file::open_ledger_bytes(&bytes))
        .and_then(|ledger| semantic_fingerprint_from_ledger(&ledger))
    {
        Ok(fingerprint) => {
            if let Err(error) = verify_fingerprints(&source, &fingerprint) {
                let _ = fs::remove_file(output_path);
                return Err(error);
            }
            fingerprint
        }
        Err(error) => {
            let _ = fs::remove_file(output_path);
            return Err(error);
        }
    };

    serde_json::to_string_pretty(&json!({
        "schema": "pact.converter.exported_records_to_v2.v1",
        "output_path": output_path.display().to_string(),
        "output_bytes": output_bytes,
        "output_file_bytes": output_bytes,
        "physical_layout": physical_layout,
        "playback_row_count": source.playback_rows.len(),
        "content_record_count": source.content_count,
        "source_playback_digest": source.playback_digest,
        "output_playback_digest": reopened_fingerprint.playback_digest,
        "source_content_digest": source.content_digest,
        "output_content_digest": reopened_fingerprint.content_digest,
        "verified": true,
    }))
    .map_err(|error| PactError::codec(format!("encode conversion report: {error}")))
}

fn align_authoritative_trace_heads(ledger: &mut crate::store::ContextTreeLedger) -> PactResult<()> {
    let mut heads = BTreeMap::<String, u64>::new();
    let mut traces = ledger
        .event_index
        .transaction_traces
        .values()
        .collect::<Vec<_>>();
    traces.sort_by_key(|trace| {
        (
            trace.branch_id.as_str(),
            trace
                .replay_trace
                .as_ref()
                .map_or(u64::MAX, |replay| replay.from_rev),
        )
    });
    for trace in traces {
        let Some(replay) = &trace.replay_trace else {
            continue;
        };
        let head = heads.entry(trace.branch_id.clone()).or_default();
        if replay.from_rev > *head {
            return Err(PactError::corrupt(format!(
                "authoritative trace chain for branch {} skips revision {}",
                trace.branch_id, *head
            )));
        }
        *head = (*head).max(replay.to_rev);
    }
    for (branch_id, head) in heads {
        let branch = ledger.branches.get_mut(&branch_id).ok_or_else(|| {
            PactError::corrupt(format!(
                "authoritative trace references missing branch {branch_id}"
            ))
        })?;
        branch.head_revision = head;
        branch.current.head_revision = head;
    }
    Ok(())
}

fn write_cold_v2_baseline(
    output_path: &Path,
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<tempfile::NamedTempFile> {
    if ledger.branches.len() != 1 {
        return Err(PactError::invalid(
            "cold conversion currently requires exactly one branch",
        ));
    }
    let bytes = crate::pact_file::encode_canonical_file(ledger)?;
    let parent = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.as_file_mut().set_len(0)?;
    staged.as_file_mut().seek(SeekFrom::Start(0))?;
    staged.as_file_mut().write_all(&bytes)?;
    staged.as_file_mut().sync_all()?;
    Ok(staged)
}

fn normalize_imported_ledger(
    ledger: crate::store::ContextTreeLedger,
) -> PactResult<crate::store::ContextTreeLedger> {
    crate::pact_file::replay_transactions(canonical_transactions_from_ledger_inner(ledger, true)?)
}

fn canonical_transactions_from_ledger_inner(
    mut ledger: crate::store::ContextTreeLedger,
    normalize_imported: bool,
) -> PactResult<Vec<crate::pact_file::transaction::PactTransaction>> {
    let inline_limit = 1024;
    let branch_id = ledger.current_branch_id.clone();
    let native_history_mismatch = !normalize_imported
        && ledger.branches.iter().any(|(branch_id, branch)| {
            ledger
                .event_index
                .transaction_traces
                .values()
                .filter(|trace| &trace.branch_id == branch_id)
                .filter_map(|trace| trace.replay_trace.as_ref().map(|trace| trace.to_rev))
                .max()
                .is_some_and(|history_head| history_head != branch.head_revision)
        });
    if ledger.event_index.transaction_traces.is_empty() || native_history_mismatch {
        let branch = ledger
            .branches
            .get(&branch_id)
            .ok_or_else(|| PactError::corrupt("baseline ledger current branch is missing"))?;
        let revision = branch.head_revision;
        let root_id = ledger.root_id.clone();
        let metadata = crate::pact_file::index::HeadMetadata::from_ledger(&ledger);
        let mut value = serde_json::to_value(&ledger)
            .map_err(|error| PactError::codec(format!("encode baseline ledger: {error}")))?;
        crate::pact_file::canonicalize_component_depth(&mut value);
        return Ok(vec![
            crate::pact_file::transaction::PactTransaction::genesis_with_metadata(
                format!("genesis_{root_id}"),
                root_id,
                branch_id,
                revision,
                value,
                Some(metadata),
            ),
        ]);
    }
    if ledger.branches.len() != 1 {
        return Err(PactError::invalid(
            "one-shot conversion currently requires exactly one branch with history",
        ));
    }
    let final_metadata = crate::pact_file::index::HeadMetadata::from_ledger(&ledger);
    let mut initial = ledger.project_branch(&branch_id, Some(0), inline_limit)?;
    if normalize_imported {
        normalize_imported_initial_placeholder(&mut initial)?;
    }
    let source_branch = ledger
        .branches
        .get(&branch_id)
        .ok_or_else(|| PactError::corrupt("converted ledger current branch is missing"))?;
    let source_head_revision = source_branch.head_revision;
    let source_metadata = ConversionBranchMetadata {
        base_ref: source_branch.base_ref.clone(),
        first_seen: source_branch.current.tree.first_seen.clone(),
        lifecycle_seen: source_branch.current.lifecycle_seen.clone(),
    };
    let root_id = ledger.root_id.clone();
    let mut history = std::mem::take(&mut ledger.event_index.transaction_traces)
        .into_values()
        .collect::<Vec<_>>();
    history.sort_by_key(|trace| {
        trace
            .replay_trace
            .as_ref()
            .map_or(u64::MAX, |trace| trace.from_rev)
    });
    ledger.event_index = Default::default();
    let mut content = std::mem::take(&mut ledger.content);
    let branch = ledger
        .branches
        .get_mut(&branch_id)
        .ok_or_else(|| PactError::corrupt("conversion baseline branch is missing"))?;
    branch.transaction_traces.clear();
    branch.current = initial;
    branch.head_revision = 0;
    branch.lifecycle_tick = 0;
    branch.base_ref.revision = Some(0);
    branch.base_ref.transaction_id = None;
    branch.base_ref.component_id = None;
    branch.base_ref.content_ref = None;
    let mut baseline_value = serde_json::to_value(&ledger)
        .map_err(|error| PactError::codec(format!("encode conversion baseline: {error}")))?;
    crate::pact_file::canonicalize_component_depth(&mut baseline_value);
    let genesis = crate::pact_file::transaction::PactTransaction::genesis_with_metadata(
        format!("genesis_{root_id}"),
        root_id,
        branch_id.clone(),
        0,
        baseline_value,
        None,
    );
    let mut transactions = vec![genesis];
    let mut expected_revision = 0_u64;
    let mut persisted_content_ids = BTreeSet::new();
    for mut public_trace in history {
        let imported_trace = public_trace.replay_trace.take().ok_or_else(|| {
            PactError::invalid(format!(
                "transaction {} has no replay trace",
                public_trace.transaction_id
            ))
        })?;
        let replay_trace = if normalize_imported {
            normalize_imported_replay_trace(imported_trace, &mut content, inline_limit)?
        } else {
            imported_trace
        };
        if public_trace.branch_id != branch_id
            || replay_trace.from_rev != expected_revision
            || replay_trace.to_rev != expected_revision.saturating_add(1)
        {
            return Err(PactError::invalid(format!(
                "transaction {} is not contiguous on branch {} at revision {}",
                public_trace.transaction_id, branch_id, expected_revision
            )));
        }
        let new_content = content
            .records
            .iter()
            .filter(|(content_id, _)| !persisted_content_ids.contains(*content_id))
            .map(|(_, record)| record.clone())
            .collect::<Vec<_>>();
        persisted_content_ids.extend(new_content.iter().map(|record| record.id.clone()));
        let delta = conversion_ledger_delta(&source_metadata, &replay_trace);
        let transaction = crate::pact_file::transaction::PactTransaction::delta(
            public_trace.transaction_id.clone(),
            public_trace.branch_id.clone(),
            conversion_boundary(&public_trace),
            replay_trace,
            public_trace,
            new_content,
            delta,
        );
        expected_revision = transaction.trace.to_rev;
        transactions.push(transaction);
    }
    if expected_revision != source_head_revision {
        return Err(PactError::invalid(format!(
            "converted history ends at revision {expected_revision}, source head is {}",
            source_head_revision
        )));
    }
    if let Some(last) = transactions.last_mut() {
        last.head_metadata = Some(final_metadata);
    }
    Ok(transactions)
}

pub(super) fn normalize_imported_replay_trace(
    mut imported: crate::transaction_trace::TransactionTrace,
    content: &mut crate::store::ContentStore,
    inline_limit: usize,
) -> PactResult<crate::transaction_trace::TransactionTrace> {
    let mut normalized = Vec::with_capacity(imported.ops.len());
    for mut op in imported.ops.drain(..) {
        if is_derived_projection_op(&op.kind) {
            continue;
        }
        match &mut op.kind {
            crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } => {
                normalize_imported_active_placeholder(&op.component_id, component);
                crate::store::externalize_large_content_in_value(component, content, inline_limit);
                *component = crate::transaction_trace::normalize_for_transaction_trace(component);
                normalized.push(op);
            }
            crate::transaction_trace::TraceOpKind::SetContent {
                content: inline,
                content_ref,
                content_preview,
            } => {
                let mut value = json!({
                    "content": inline.take(),
                    "content_ref": content_ref.take(),
                    "content_preview": content_preview.take(),
                });
                crate::store::externalize_large_content_in_value(&mut value, content, inline_limit);
                normalized.push(crate::transaction_trace::TraceOp::new(
                    op.component_id,
                    crate::transaction_trace::TraceOpKind::SetContent {
                        content: value
                            .get("content")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        content_ref: value
                            .get("content_ref")
                            .filter(|value| !value.is_null())
                            .cloned(),
                        content_preview: value
                            .get("content_preview")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
                ));
            }
            _ => normalized.push(op),
        }
    }
    imported.ops = normalized;
    imported.validate()?;
    Ok(imported)
}

fn normalize_imported_active_placeholder(component_id: &str, component: &mut Value) {
    let Some(object) = component.as_object_mut() else {
        return;
    };
    let is_placeholder = component_id == "active"
        && object.get("componentType").and_then(Value::as_str) == Some("message_turn")
        && object
            .get("created_at_ns")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            == 0
        && object
            .get("children")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        && object
            .get("content")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
    if is_placeholder {
        object.insert(
            "componentType".to_string(),
            Value::String("active_message".to_string()),
        );
        object.insert(
            "type".to_string(),
            Value::String("active_message".to_string()),
        );
    }
}

fn normalize_imported_initial_placeholder(view: &mut crate::store::PactView) -> PactResult<()> {
    let Some(active) = view.tree.component("active") else {
        return Ok(());
    };
    let mut value = active.project_component_value();
    normalize_imported_active_placeholder("active", &mut value);
    if value.get("componentType").and_then(Value::as_str) != Some("active_message") {
        return Ok(());
    }
    let normalized = crate::Component::from_component_value(&value).map_err(|error| {
        PactError::invalid(format!("invalid normalized active placeholder: {error}"))
    })?;
    *view
        .tree
        .component_mut("active")
        .ok_or_else(|| PactError::internal("active placeholder disappeared during conversion"))? =
        normalized;
    if let Some(ids) = view.tree.index.by_type.get_mut("message_turn") {
        ids.retain(|id| id != "active");
    }
    view.tree
        .index
        .by_type
        .entry("active_message".to_string())
        .or_default()
        .push("active".to_string());
    view.message_order.retain(|id| id != "active");
    Ok(())
}

struct ConversionBranchMetadata {
    base_ref: crate::store::PactRef,
    first_seen: BTreeMap<String, u64>,
    lifecycle_seen: BTreeMap<String, u64>,
}

fn conversion_ledger_delta(
    source: &ConversionBranchMetadata,
    trace: &crate::transaction_trace::TransactionTrace,
) -> crate::pact_file::transaction::LedgerDelta {
    let mut first_seen_additions = BTreeMap::new();
    let mut lifecycle_seen_additions = BTreeMap::new();
    let mut lifecycle_seen_removals = Vec::new();
    for op in &trace.ops {
        match op.kind {
            crate::transaction_trace::TraceOpKind::InsertComponent { .. } => {
                if let Some(revision) = source.first_seen.get(&op.component_id) {
                    first_seen_additions.insert(op.component_id.clone(), *revision);
                }
                if let Some(tick) = source.lifecycle_seen.get(&op.component_id) {
                    lifecycle_seen_additions.insert(op.component_id.clone(), *tick);
                }
            }
            crate::transaction_trace::TraceOpKind::DeleteComponent => {
                lifecycle_seen_removals.push(op.component_id.clone());
            }
            _ => {}
        }
    }
    let base_ref = source.base_ref.clone();
    crate::pact_file::transaction::LedgerDelta {
        lifecycle_tick: trace.to_rev,
        lifecycle_birth_tick: trace.to_rev,
        base_ref,
        first_seen_additions,
        lifecycle_seen_additions,
        lifecycle_seen_removals,
        record_canonical_trace: true,
        record_public_trace: true,
        derived_depth: false,
    }
}

fn conversion_boundary(trace: &crate::store::PactTransactionTrace) -> crate::trace::BoundaryKind {
    trace
        .aux
        .get("boundary")
        .and_then(Value::as_str)
        .and_then(|boundary| {
            serde_json::from_value::<crate::trace::BoundaryKind>(Value::String(
                boundary.to_string(),
            ))
            .ok()
        })
        .unwrap_or(crate::trace::BoundaryKind::Adhoc)
}

fn semantic_fingerprint(
    records: &[ScannedRecord],
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<SemanticFingerprint> {
    let traces = query_records(records, "trace", None)?.matches;
    semantic_fingerprint_from_traces(&traces, ledger)
}

fn semantic_fingerprint_from_ledger(
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<SemanticFingerprint> {
    let mut traces = ledger
        .event_index
        .transaction_traces
        .values()
        .collect::<Vec<_>>();
    traces.sort_by_key(|trace| {
        (
            trace.branch_id.as_str(),
            trace
                .replay_trace
                .as_ref()
                .map_or(u64::MAX, |replay| replay.from_rev),
        )
    });
    let traces = traces
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PactError::codec(format!("encode playback traces: {error}")))?;
    semantic_fingerprint_from_traces(&traces, ledger)
}

fn semantic_fingerprint_from_traces(
    traces: &[Value],
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<SemanticFingerprint> {
    let mut playback_rows =
        stream_rows_from_activity_entries(&activity_entries_from_traces(traces));
    for (sequence, row) in playback_rows.iter_mut().enumerate() {
        if let Some(object) = row.as_object_mut() {
            object.insert("seq".to_string(), Value::Number((sequence as u64).into()));
        }
    }
    let playback_bytes = serde_json::to_vec(&playback_rows)
        .map_err(|error| PactError::codec(format!("encode playback fingerprint: {error}")))?;
    let playback_digest = blake3::hash(&playback_bytes).to_hex().to_string();

    let mut content_hasher = blake3::Hasher::new();
    for (id, record) in &ledger.content.records {
        hash_field(&mut content_hasher, id.as_bytes());
        hash_field(&mut content_hasher, record.kind.as_bytes());
        hash_field(&mut content_hasher, record.hash.as_bytes());
        hash_field(&mut content_hasher, &record.bytes.to_le_bytes());
        hash_field(
            &mut content_hasher,
            &record.chars.unwrap_or_default().to_le_bytes(),
        );
        let content = decoded_content_bytes(record)?;
        if content.len() != record.bytes {
            return Err(PactError::corrupt(format!(
                "decoded content length mismatch for {id}: declared {}, decoded {}",
                record.bytes,
                content.len()
            )));
        }
        hash_field(&mut content_hasher, &content);
    }

    Ok(SemanticFingerprint {
        playback_rows,
        playback_digest,
        content_count: ledger.content.records.len(),
        content_digest: content_hasher.finalize().to_hex().to_string(),
    })
}

fn decoded_content_bytes(record: &crate::store::ContentRecord) -> PactResult<Vec<u8>> {
    if let Some(inline) = &record.storage.inline {
        return Ok(inline.as_bytes().to_vec());
    }
    if let Some(inline_bytes) = &record.storage.inline_bytes {
        return Ok(inline_bytes.clone());
    }
    Err(PactError::invalid(format!(
        "content {} is not file-local and cannot be verified byte-for-byte",
        record.id
    )))
}

fn hash_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn verify_fingerprints(
    source: &SemanticFingerprint,
    output: &SemanticFingerprint,
) -> PactResult<()> {
    if source.playback_rows != output.playback_rows {
        return Err(PactError::corrupt(
            "converted playback rows differ from the source export",
        ));
    }
    if source.content_count != output.content_count
        || source.content_digest != output.content_digest
    {
        return Err(PactError::corrupt(format!(
            "converted decoded content differs from the source export: source count/digest {}/{}, output {}/{}",
            source.content_count,
            source.content_digest,
            output.content_count,
            output.content_digest,
        )));
    }
    Ok(())
}
