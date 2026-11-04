use super::TransactionTrace;
use super::trace_op::TraceOpKind;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RevisionTrace {
    pub from_rev: u64,
    pub to_rev: u64,

    pub changed_component_ids: Vec<String>,

    pub op_counts: BTreeMap<String, usize>,
}

fn op_name(kind: &TraceOpKind) -> &'static str {
    match kind {
        TraceOpKind::InsertComponent { .. } => "insert_component",
        TraceOpKind::DeleteComponent => "delete_component",
        TraceOpKind::MoveComponent { .. } => "move_component",
        TraceOpKind::SetField { .. } => "set_field",
        TraceOpKind::UnsetField { .. } => "unset_field",
        TraceOpKind::SetAttr { .. } => "set_aux",
        TraceOpKind::UnsetAttr { .. } => "unset_aux",
        TraceOpKind::SetContent { .. } => "set_content",
        TraceOpKind::ReplaceContentRange { .. } => "replace_content_range",
        TraceOpKind::SetLifecycle { .. } => "set_lifecycle",
    }
}

pub fn derive_revision_trace(trace: &TransactionTrace) -> RevisionTrace {
    let mut changed_component_ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut op_counts: BTreeMap<String, usize> = BTreeMap::new();
    for op in &trace.ops {
        if seen.insert(op.component_id.clone()) {
            changed_component_ids.push(op.component_id.clone());
        }
        *op_counts.entry(op_name(&op.kind).to_string()).or_insert(0) += 1;
    }
    RevisionTrace {
        from_rev: trace.from_rev,
        to_rev: trace.to_rev,
        changed_component_ids,
        op_counts,
    }
}

pub fn derive_trace(traces: &[TransactionTrace]) -> Vec<RevisionTrace> {
    traces.iter().map(derive_revision_trace).collect()
}

#[cfg(test)]
mod tests;
