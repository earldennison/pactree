use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

use super::segment::RecordLocator;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

const MAGIC: &[u8; 8] = b"PHCPV1\0\0";
const HEADER_BYTES: usize = 48;
const MAX_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;
const COMPACT_LEDGER_SCHEMA: &str = "pact.compact_head_ledger.v1";

#[derive(Serialize)]
struct CompactLedgerEnvelope<'a> {
    schema: &'static str,
    ledger: CompactLedger<'a>,
}

#[derive(Serialize)]
struct CompactLedger<'a> {
    schema: &'a str,
    root_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: &'a Option<crate::store::PactParentProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: &'a Option<crate::store::PactBaseProvenance>,
    branches: std::collections::BTreeMap<String, CompactBranch<'a>>,
    content: CompactContent,
    event_index: CompactEventIndex<'a>,
    current_branch_id: &'a str,
}

#[derive(Serialize)]
struct CompactBranch<'a> {
    branch_id: &'a str,
    root_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_branch_id: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fork_ref: &'a Option<crate::store::PactRef>,
    base_ref: &'a crate::store::PactRef,
    head_revision: u64,
    lifecycle_tick: u64,
    transaction_traces: &'static [crate::transaction_trace::TransactionTrace],
    current: CompactView<'a>,
    aux: &'a std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize)]
struct CompactView<'a> {
    tree: CompactTree<'a>,
    branch_id: &'a str,
    head_revision: u64,
    lifecycle_seen: CompactLifecycleSeen<'a>,
    message_order: &'a [String],
    branch_refs: &'a [crate::store::PactRef],
    content_refs: &'a std::collections::BTreeMap<String, crate::store::ContentRecordSummary>,
}

struct CompactLifecycleSeen<'a> {
    values: &'a std::collections::BTreeMap<String, u64>,
    visible: &'a crate::store::ComponentViewIndex,
}

impl Serialize for CompactLifecycleSeen<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        for (component_id, tick) in self.values {
            if self.visible.by_id.contains_key(component_id) {
                map.serialize_entry(component_id, tick)?;
            }
        }
        map.end()
    }
}

#[derive(Serialize)]
struct CompactTree<'a> {
    root_id: &'a str,
    stats: &'a crate::store::PactStoreStats,
    root: serde_json::Value,
    index: crate::store::ComponentViewIndex,
    first_seen: &'a std::collections::BTreeMap<String, u64>,
    tool_pairs: std::collections::BTreeMap<String, crate::store::ToolPairRecord>,
}

#[derive(Serialize)]
struct CompactContent {
    records: serde_json::Map<String, serde_json::Value>,
    records_by_hash: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct CompactEventIndex<'a> {
    by_transaction: serde_json::Map<String, serde_json::Value>,
    transaction_traces: &'a std::collections::BTreeMap<String, crate::store::PactTransactionTrace>,
    by_role: serde_json::Map<String, serde_json::Value>,
    by_type: serde_json::Map<String, serde_json::Value>,
    by_aux_key: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct HeadCheckpoint {
    pub(crate) root_id: String,
    pub(crate) current_branch_id: String,
    pub(crate) branches: Vec<BranchCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct BranchCheckpoint {
    pub(crate) branch_id: String,
    pub(crate) revision: u64,
    pub(crate) base_ref: crate::store::PactRef,
    pub(crate) components: Vec<RecordLocator>,
}

impl HeadCheckpoint {
    pub(crate) fn from_ledger(ledger: &crate::store::ContextTreeLedger) -> PactResult<Self> {
        if !ledger.branches.contains_key(&ledger.current_branch_id) {
            return Err(PactError::invalid(
                "Pact head checkpoint current branch is missing",
            ));
        }
        let components = ledger
            .branches
            .iter()
            .map(|(branch_id, branch)| {
                Ok((
                    branch_id.clone(),
                    branch
                        .current
                        .tree
                        .index
                        .by_id
                        .keys()
                        .enumerate()
                        .map(|(ordinal, _)| {
                            Ok(RecordLocator {
                                segment_id: branch.head_revision,
                                ordinal: u32::try_from(ordinal).map_err(|_| {
                                    PactError::invalid(
                                        "Pact head checkpoint component count exceeds u32",
                                    )
                                })?,
                            })
                        })
                        .collect::<PactResult<Vec<_>>>()?,
                ))
            })
            .collect::<PactResult<std::collections::BTreeMap<_, _>>>()?;
        Self::from_ledger_and_components(ledger, components)
    }

    pub(crate) fn from_ledger_and_components(
        ledger: &crate::store::ContextTreeLedger,
        mut components: std::collections::BTreeMap<String, Vec<RecordLocator>>,
    ) -> PactResult<Self> {
        let branches = ledger
            .branches
            .iter()
            .map(|(branch_id, branch)| {
                Ok(BranchCheckpoint {
                    branch_id: branch_id.clone(),
                    revision: branch.head_revision,
                    base_ref: branch.base_ref.clone(),
                    components: components.remove(branch_id).unwrap_or_default(),
                })
            })
            .collect::<PactResult<Vec<_>>>()?;
        Ok(Self {
            root_id: ledger.root_id.clone(),
            current_branch_id: ledger.current_branch_id.clone(),
            branches,
        })
    }
}

pub(crate) fn encode(checkpoint: &HeadCheckpoint) -> PactResult<Vec<u8>> {
    let raw = serde_json::to_vec(checkpoint)
        .map_err(|error| PactError::codec(format!("encode Pact head checkpoint: {error}")))?;
    if raw.len() > MAX_CHECKPOINT_BYTES {
        return Err(PactError::invalid(
            "Pact head checkpoint exceeds fixed limit",
        ));
    }
    let compressed = zstd::bulk::compress(&raw, 7)
        .map_err(|error| PactError::codec(format!("compress Pact head checkpoint: {error}")))?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + compressed.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(
        &u64::try_from(raw.len())
            .map_err(|_| PactError::invalid("Pact head checkpoint length exceeds u64"))?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(blake3::hash(&raw).as_bytes());
    bytes.extend_from_slice(&compressed);
    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> PactResult<HeadCheckpoint> {
    if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
        return Err(PactError::corrupt("invalid Pact head checkpoint header"));
    }
    let raw_len =
        usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().map_err(
            |_| PactError::corrupt("invalid Pact head checkpoint length"),
        )?))
        .map_err(|_| PactError::corrupt("Pact head checkpoint exceeds platform limits"))?;
    if raw_len > MAX_CHECKPOINT_BYTES {
        return Err(PactError::corrupt(
            "decoded Pact head checkpoint exceeds fixed limit",
        ));
    }
    let raw = zstd::bulk::decompress(&bytes[HEADER_BYTES..], raw_len)
        .map_err(|error| PactError::corrupt(format!("invalid Pact head checkpoint: {error}")))?;
    if raw.len() != raw_len || blake3::hash(&raw).as_bytes() != &bytes[16..HEADER_BYTES] {
        return Err(PactError::corrupt("Pact head checkpoint checksum mismatch"));
    }
    serde_json::from_slice(&raw)
        .map_err(|error| PactError::corrupt(format!("invalid Pact head checkpoint: {error}")))
}

pub(crate) fn strip_derived_head(ledger: &serde_json::Value) -> PactResult<serde_json::Value> {
    strip_derived_head_owned(ledger.clone())
}

pub(crate) fn strip_derived_head_owned(
    mut compact: serde_json::Value,
) -> PactResult<serde_json::Value> {
    reference_immutable_head_components(&mut compact)?;
    let branches = compact
        .get_mut("branches")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| PactError::invalid("Pact compact ledger has no branches"))?;
    for branch in branches.values_mut() {
        branch
            .as_object_mut()
            .ok_or_else(|| PactError::invalid("Pact compact branch is not an object"))?
            .insert("transaction_traces".to_string(), serde_json::json!([]));
        let tree = branch
            .pointer_mut("/current/tree")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| PactError::invalid("Pact compact ledger branch has no current tree"))?;
        let root = tree
            .get_mut("root")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| PactError::invalid("Pact compact ledger tree has no root"))?;
        root.insert("children".to_string(), serde_json::Value::Array(Vec::new()));
        tree.insert(
            "index".to_string(),
            serde_json::to_value(crate::store::ComponentViewIndex::default()).map_err(|error| {
                PactError::codec(format!("encode empty Pact component index: {error}"))
            })?,
        );
        tree.insert("tool_pairs".to_string(), serde_json::json!({}));
    }
    if let Some(content) = compact
        .get_mut("content")
        .and_then(serde_json::Value::as_object_mut)
    {
        content.insert("records".to_string(), serde_json::json!({}));
        content.insert("records_by_hash".to_string(), serde_json::json!({}));
    }
    if let Some(event_index) = compact
        .get_mut("event_index")
        .and_then(serde_json::Value::as_object_mut)
    {
        for key in ["by_transaction", "by_role", "by_type", "by_aux_key"] {
            event_index.insert(key.to_string(), serde_json::json!({}));
        }
    }
    Ok(serde_json::json!({
        "schema": COMPACT_LEDGER_SCHEMA,
        "ledger": compact,
    }))
}

pub(crate) fn compact_ledger_value(
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<serde_json::Value> {
    let transaction_traces = compact_transaction_traces(ledger);
    let branches = ledger
        .branches
        .iter()
        .map(|(branch_id, branch)| {
            let root_child_ids = branch
                .current
                .tree
                .root
                .children
                .iter()
                .map(|child| child.id.clone())
                .collect::<Vec<_>>();
            let mut root = branch
                .current
                .tree
                .root
                .clone_without_children()
                .project_component_value();
            crate::pact_file::canonicalize_component_depth(&mut root);
            root["child_ids"] = serde_json::Value::Array(
                root_child_ids
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            );
            root["children"] = serde_json::Value::Array(Vec::new());
            let compact = CompactBranch {
                branch_id: &branch.branch_id,
                root_id: &branch.root_id,
                parent_branch_id: &branch.parent_branch_id,
                fork_ref: &branch.fork_ref,
                base_ref: &branch.base_ref,
                head_revision: branch.head_revision,
                lifecycle_tick: branch.lifecycle_tick,
                transaction_traces: &[],
                current: CompactView {
                    tree: CompactTree {
                        root_id: &branch.current.tree.root_id,
                        stats: &branch.current.tree.stats,
                        root,
                        index: crate::store::ComponentViewIndex::default(),
                        first_seen: &branch.current.tree.first_seen,
                        tool_pairs: std::collections::BTreeMap::new(),
                    },
                    branch_id: &branch.current.branch_id,
                    head_revision: branch.current.head_revision,
                    lifecycle_seen: CompactLifecycleSeen {
                        values: &branch.current.lifecycle_seen,
                        visible: &branch.current.tree.index,
                    },
                    message_order: &branch.current.message_order,
                    branch_refs: &branch.current.branch_refs,
                    content_refs: &branch.current.content_refs,
                },
                aux: &branch.aux,
            };
            (branch_id.clone(), compact)
        })
        .collect();
    serde_json::to_value(CompactLedgerEnvelope {
        schema: COMPACT_LEDGER_SCHEMA,
        ledger: CompactLedger {
            schema: &ledger.schema,
            root_id: &ledger.root_id,
            parent: &ledger.parent,
            base: &ledger.base,
            branches,
            content: CompactContent {
                records: serde_json::Map::new(),
                records_by_hash: serde_json::Map::new(),
            },
            event_index: CompactEventIndex {
                by_transaction: serde_json::Map::new(),
                transaction_traces: &transaction_traces,
                by_role: serde_json::Map::new(),
                by_type: serde_json::Map::new(),
                by_aux_key: serde_json::Map::new(),
            },
            current_branch_id: &ledger.current_branch_id,
        },
    })
    .map_err(|error| PactError::codec(format!("encode compact Pact ledger: {error}")))
}

fn compact_transaction_traces(
    ledger: &crate::store::ContextTreeLedger,
) -> std::collections::BTreeMap<String, crate::store::PactTransactionTrace> {
    let current_ids = ledger
        .branches
        .values()
        .flat_map(|branch| branch.current.tree.index.by_id.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let mut insert_counts = std::collections::BTreeMap::<String, usize>::new();
    let mut mutated = std::collections::BTreeSet::new();
    for trace in ledger.event_index.transaction_traces.values() {
        let Some(replay) = trace.replay_trace.as_ref() else {
            continue;
        };
        for op in &replay.ops {
            if matches!(
                op.kind,
                crate::transaction_trace::TraceOpKind::InsertComponent { .. }
            ) {
                *insert_counts.entry(op.component_id.clone()).or_default() += 1;
            } else {
                mutated.insert(op.component_id.clone());
            }
        }
    }
    let immutable = insert_counts
        .into_iter()
        .filter(|(component_id, count)| {
            *count == 1 && !mutated.contains(component_id) && current_ids.contains(component_id)
        })
        .map(|(component_id, _)| component_id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut traces = ledger.event_index.transaction_traces.clone();
    for trace in traces.values_mut() {
        let Some(replay) = trace.replay_trace.as_mut() else {
            continue;
        };
        for op in &mut replay.ops {
            if !immutable.contains(&op.component_id) {
                continue;
            }
            if let crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } =
                &mut op.kind
            {
                *component = serde_json::json!({"$pact_head_component": op.component_id});
            }
        }
    }
    traces
}

fn reference_immutable_head_components(ledger: &mut serde_json::Value) -> PactResult<()> {
    let current_ids = ledger
        .get("branches")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|branches| branches.values())
        .filter_map(|branch| branch.pointer("/current/tree/index/by_id"))
        .filter_map(serde_json::Value::as_object)
        .flat_map(|index| index.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let traces = ledger
        .pointer_mut("/event_index/transaction_traces")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| PactError::invalid("Pact compact ledger has no transaction traces"))?;
    let mut insert_counts = std::collections::BTreeMap::<String, usize>::new();
    let mut mutated = std::collections::BTreeSet::new();
    for trace in traces.values() {
        let Some(ops) = trace
            .pointer("/replay_trace/ops")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for op in ops {
            let Some(component_id) = op.get("component_id").and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            if op.get("op").and_then(serde_json::Value::as_str) == Some("insert_component") {
                *insert_counts.entry(component_id.to_string()).or_default() += 1;
            } else {
                mutated.insert(component_id.to_string());
            }
        }
    }
    let immutable = insert_counts
        .into_iter()
        .filter(|(component_id, count)| {
            *count == 1 && !mutated.contains(component_id) && current_ids.contains(component_id)
        })
        .map(|(component_id, _)| component_id)
        .collect::<std::collections::BTreeSet<_>>();
    for trace in traces.values_mut() {
        let Some(ops) = trace
            .pointer_mut("/replay_trace/ops")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for op in ops {
            let Some(component_id) = op
                .get("component_id")
                .and_then(serde_json::Value::as_str)
                .filter(|component_id| immutable.contains(*component_id))
                .map(str::to_string)
            else {
                continue;
            };
            if op.get("op").and_then(serde_json::Value::as_str) == Some("insert_component") {
                op.as_object_mut()
                    .ok_or_else(|| PactError::invalid("Pact trace operation is not an object"))?
                    .insert(
                        "component".to_string(),
                        serde_json::json!({"$pact_head_component": component_id}),
                    );
            }
        }
    }
    Ok(())
}

pub(crate) fn is_compact_ledger(value: &serde_json::Value) -> bool {
    value.get("schema").and_then(serde_json::Value::as_str) == Some(COMPACT_LEDGER_SCHEMA)
}

pub(crate) fn hydrate_ledger(
    file: &[u8],
    index_root: u64,
    compact: &serde_json::Value,
) -> PactResult<serde_json::Value> {
    serde_json::to_value(hydrate_ledger_typed(file, index_root, compact)?)
        .map_err(|error| PactError::codec(format!("encode hydrated Pact ledger: {error}")))
}

pub(crate) fn hydrate_ledger_typed(
    file: &[u8],
    index_root: u64,
    compact: &serde_json::Value,
) -> PactResult<crate::store::ContextTreeLedger> {
    let pages = CachedPages::new(file);
    hydrate_ledger_typed_from_pages(index_root, compact, &pages)
}

pub(crate) fn hydrate_ledger_typed_from_pages(
    index_root: u64,
    compact: &serde_json::Value,
    pages: &dyn crate::pact_file::btree::PageReader,
) -> PactResult<crate::store::ContextTreeLedger> {
    hydrate_ledger_typed_from_pages_owned(index_root, compact.clone(), pages)
}

fn referenced_head_component_ids(
    traces: &std::collections::BTreeMap<String, crate::store::PactTransactionTrace>,
) -> std::collections::BTreeSet<String> {
    let mut referenced = std::collections::BTreeSet::new();
    for trace in traces.values() {
        let Some(replay) = trace.replay_trace.as_ref() else {
            continue;
        };
        for op in &replay.ops {
            if let crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } =
                &op.kind
                && let Some(id) = component
                    .get("$pact_head_component")
                    .and_then(serde_json::Value::as_str)
            {
                referenced.insert(id.to_string());
            }
        }
    }
    referenced
}

pub(crate) fn hydrate_ledger_typed_from_pages_owned(
    index_root: u64,
    compact: serde_json::Value,
    pages: &dyn crate::pact_file::btree::PageReader,
) -> PactResult<crate::store::ContextTreeLedger> {
    if !is_compact_ledger(&compact) {
        return serde_json::from_value(compact)
            .map_err(|error| PactError::corrupt(format!("invalid Pact ledger: {error}")));
    }
    let mut compact = compact;
    let checkpoint = crate::pact_file::index::read_index_value_from_pages(
        pages,
        index_root,
        &crate::pact_file::index::IndexKey::HeadCheckpoint,
    )?
    .ok_or_else(|| PactError::corrupt("compact Pact ledger has no head checkpoint"))?;
    let checkpoint = decode(&checkpoint)?;
    let ledger = compact
        .as_object_mut()
        .and_then(|object| object.remove("ledger"))
        .ok_or_else(|| PactError::corrupt("compact Pact ledger payload is missing"))?;
    let mut ledger: crate::store::ContextTreeLedger = serde_json::from_value(ledger)
        .map_err(|error| PactError::corrupt(format!("invalid compact Pact ledger: {error}")))?;
    if checkpoint.root_id != ledger.root_id
        || checkpoint.current_branch_id != ledger.current_branch_id
    {
        return Err(PactError::corrupt(
            "compact Pact head checkpoint identity mismatch",
        ));
    }

    let referenced = referenced_head_component_ids(&ledger.event_index.transaction_traces);
    let mut head_components = std::collections::BTreeMap::new();
    for branch_checkpoint in checkpoint.branches {
        let branch = ledger
            .branches
            .get_mut(&branch_checkpoint.branch_id)
            .ok_or_else(|| PactError::corrupt("compact Pact checkpoint branch is missing"))?;
        if branch.head_revision != branch_checkpoint.revision
            || branch.base_ref != branch_checkpoint.base_ref
        {
            return Err(PactError::corrupt(
                "compact Pact checkpoint branch metadata mismatch",
            ));
        }
        let children_by_parent = crate::pact_file::index::parent_children_by_parent_from_pages(
            pages,
            index_root,
            &branch_checkpoint.branch_id,
        )?;
        let mut components = std::collections::BTreeMap::new();
        for locator in branch_checkpoint.components {
            let payload = crate::pact_file::index::read_component_payload_from_pages(
                pages, index_root, locator,
            )?;
            let indexed = crate::pact_file::index::IndexedComponent::decode(&payload)?;
            let id = indexed
                .value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| PactError::corrupt("compact Pact component has no id"))?
                .to_string();
            if referenced.contains(&id) {
                let mut trace_value = indexed.value.clone();
                let trace_object = trace_value
                    .as_object_mut()
                    .ok_or_else(|| PactError::corrupt("compact Pact component is not an object"))?;
                trace_object.insert(
                    "child_ids".to_string(),
                    serde_json::Value::Array(
                        children_by_parent
                            .get(&id)
                            .into_iter()
                            .flatten()
                            .cloned()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
                trace_object.insert("children".to_string(), serde_json::Value::Array(Vec::new()));
                head_components.insert(id.clone(), trace_value);
            }
            components.insert(id, indexed.value);
        }

        let root_id = branch.current.tree.root_id.clone();
        let root = branch.current.tree.root.project_component_value();
        let mut visited = std::collections::BTreeSet::new();
        let root = hydrate_component_tree(
            &root_id,
            root,
            &mut components,
            &children_by_parent,
            &mut visited,
        )?;
        drop(components);

        let first_seen = std::mem::take(&mut branch.current.tree.first_seen);
        let lifecycle_seen = std::mem::take(&mut branch.current.lifecycle_seen);
        let message_order = std::mem::take(&mut branch.current.message_order);
        let branch_refs = std::mem::take(&mut branch.current.branch_refs);
        let content_refs = std::mem::take(&mut branch.current.content_refs);
        let root = crate::Component::from_component_value(&root)
            .map_err(|err| PactError::invalid(format!("invalid PACT component tree: {err}")))?;
        let projection =
            crate::store::build_projection_from_owned_component(root, first_seen.clone(), 1024)?;
        let mut view = crate::store::PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_checkpoint.branch_id,
            branch.head_revision,
            lifecycle_seen,
            branch.lifecycle_tick,
        );
        view.tree.first_seen = first_seen;
        view.message_order = message_order;
        view.branch_refs = branch_refs;
        view.content_refs = content_refs;
        branch.current = view;
    }
    resolve_head_component_refs(&mut ledger.event_index.transaction_traces, &head_components)?;
    let replay_traces = ledger
        .event_index
        .transaction_traces
        .values()
        .filter_map(|trace| {
            trace.replay_trace.clone().map(|replay| {
                (
                    trace.branch_id.clone(),
                    trace.transaction_id.clone(),
                    replay,
                )
            })
        })
        .collect::<Vec<_>>();
    for (branch_id, transaction_id, replay_trace) in replay_traces {
        let branch = ledger
            .branches
            .get_mut(&branch_id)
            .ok_or_else(|| PactError::corrupt("compact Pact trace branch is missing"))?;
        branch.transaction_traces.push(replay_trace);
        ledger
            .event_index
            .by_transaction
            .entry(transaction_id)
            .or_default()
            .push(branch_id);
    }
    for branch in ledger.branches.values_mut() {
        branch
            .transaction_traces
            .sort_by_key(|trace| (trace.from_rev, trace.to_rev));
    }
    Ok(ledger)
}

pub(crate) fn resolve_head_component_refs(
    traces: &mut std::collections::BTreeMap<String, crate::store::PactTransactionTrace>,
    components: &std::collections::BTreeMap<String, serde_json::Value>,
) -> PactResult<()> {
    for trace in traces.values_mut() {
        let Some(replay) = trace.replay_trace.as_mut() else {
            continue;
        };
        for op in &mut replay.ops {
            let crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } =
                &mut op.kind
            else {
                continue;
            };
            let Some(component_id) = component
                .get("$pact_head_component")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            *component = components.get(&component_id).cloned().ok_or_else(|| {
                PactError::corrupt("compact Pact trace component reference is missing")
            })?;
        }
    }
    Ok(())
}

fn hydrate_component_tree(
    component_id: &str,
    mut value: serde_json::Value,
    components: &mut std::collections::BTreeMap<String, serde_json::Value>,
    children_by_parent: &std::collections::BTreeMap<String, Vec<String>>,
    visited: &mut std::collections::BTreeSet<String>,
) -> PactResult<serde_json::Value> {
    if !visited.insert(component_id.to_string()) {
        return Err(PactError::corrupt(
            "compact Pact component graph contains a cycle",
        ));
    }
    let child_ids = children_by_parent
        .get(component_id)
        .cloned()
        .unwrap_or_default();
    let mut children = Vec::with_capacity(child_ids.len());
    for child_id in &child_ids {
        if visited.contains(child_id) {
            return Err(PactError::corrupt(
                "compact Pact component graph contains a cycle",
            ));
        }

        let child = components
            .remove(child_id)
            .ok_or_else(|| PactError::corrupt("compact Pact child component is missing"))?;
        children.push(hydrate_component_tree(
            child_id,
            child,
            components,
            children_by_parent,
            visited,
        )?);
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt("compact Pact component is not an object"))?;
    object.insert(
        "child_ids".to_string(),
        serde_json::Value::Array(
            child_ids
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    object.insert("children".to_string(), serde_json::Value::Array(children));
    Ok(value)
}

struct CachedPages<'a> {
    file: &'a [u8],
    batches: std::cell::RefCell<
        std::collections::BTreeMap<u64, Vec<[u8; crate::pact_file::btree::PAGE_BYTES]>>,
    >,
    component_batches: std::cell::RefCell<
        std::collections::BTreeMap<u64, std::collections::BTreeMap<u32, Vec<u8>>>,
    >,
}

impl<'a> CachedPages<'a> {
    fn new(file: &'a [u8]) -> Self {
        Self {
            file,
            batches: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            component_batches: std::cell::RefCell::new(std::collections::BTreeMap::new()),
        }
    }
}

impl crate::pact_file::btree::PageReader for CachedPages<'_> {
    fn read_page(&self, address: u64) -> PactResult<[u8; crate::pact_file::btree::PAGE_BYTES]> {
        let batch_address = crate::pact_file::codec::index_page_batch_address(address);
        let ordinal = crate::pact_file::codec::index_page_ordinal(address);
        if let Some(page) = self
            .batches
            .borrow()
            .get(&batch_address)
            .and_then(|pages| pages.get(ordinal))
        {
            return Ok(*page);
        }
        let pages =
            crate::pact_file::codec::decode_index_page_batch_record(self.file, batch_address)?;
        let page = pages
            .get(ordinal)
            .copied()
            .ok_or_else(|| PactError::corrupt("Pact index page ordinal is out of range"))?;
        self.batches.borrow_mut().insert(batch_address, pages);
        Ok(page)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> PactResult<Vec<u8>> {
        let mut batches = self.component_batches.borrow_mut();
        if let std::collections::btree_map::Entry::Vacant(entry) = batches.entry(address) {
            let decoded = crate::pact_file::codec::decode_component_payload_batch_record_all(
                self.file, address,
            )?;
            entry.insert(decoded);
        }
        batches
            .get(&address)
            .and_then(|batch| batch.get(&ordinal))
            .cloned()
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "Pact component payload batch has no ordinal {ordinal}"
                ))
            })
    }
}
