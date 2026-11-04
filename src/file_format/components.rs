use super::*;
use crate::error::{PactError, PactResult};

pub(super) fn decode_view_records(view_records_json: &str) -> PactResult<PactViewRecords> {
    let value: Value = serde_json::from_str(view_records_json)
        .map_err(|err| PactError::codec(format!("invalid view records JSON: {err}")))?;
    if value.is_array() {
        let views: Vec<PactViewRecord> = serde_json::from_value(value)
            .map_err(|err| PactError::codec(format!("invalid view record array: {err}")))?;
        return Ok(PactViewRecords { views });
    }
    if value.get("views").is_some() {
        return serde_json::from_value(value)
            .map_err(|err| PactError::codec(format!("invalid view records payload: {err}")));
    }
    let view: PactViewRecord = serde_json::from_value(value)
        .map_err(|err| PactError::codec(format!("invalid view record: {err}")))?;
    Ok(PactViewRecords { views: vec![view] })
}

pub(super) fn view_records_from_records(records: &[ScannedRecord]) -> PactResult<PactViewRecords> {
    let mut views = Vec::new();
    for record in records.iter().filter(|record| record.kind == "view") {
        let view: PactViewRecord = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid view record payload: {err}")))?;
        views.push(view);
    }
    Ok(PactViewRecords { views })
}

pub(super) fn find_view<'a>(
    views: &'a PactViewRecords,
    view_id: &str,
) -> PactResult<&'a PactViewRecord> {
    views
        .views
        .iter()
        .find(|view| view.view_id == view_id)
        .ok_or_else(|| PactError::not_found(format!("view not found in .pact file: {view_id}")))
}

pub(super) fn view_projection_branch(view: &PactViewRecord) -> &str {
    if matches!(view.mode.as_str(), "mutable" | "mutable_live") {
        view.view_branch_id
            .as_deref()
            .unwrap_or(&view.source_branch_id)
    } else {
        &view.source_branch_id
    }
}

pub(super) fn view_projection_revision(view: &PactViewRecord) -> Option<u64> {
    if matches!(view.mode.as_str(), "readonly") {
        view.source_revision
    } else {
        None
    }
}

pub(super) fn merge_view_records(existing: &mut PactViewRecords, incoming: PactViewRecords) {
    let mut by_view_id = existing
        .views
        .iter()
        .enumerate()
        .map(|(index, view)| (view.view_id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    for view in incoming.views {
        if let Some(index) = by_view_id.get(&view.view_id).copied() {
            existing.views[index] = view;
        } else {
            by_view_id.insert(view.view_id.clone(), existing.views.len());
            existing.views.push(view);
        }
    }
}

pub(super) fn validate_view_records(
    views: &PactViewRecords,
    root_store: &ContextTreeLedger,
) -> PactResult<()> {
    let mut view_ids = BTreeMap::new();
    for view in &views.views {
        if view.view_id.trim().is_empty() {
            return Err(PactError::invalid("view record missing view_id"));
        }
        if view.root_id.trim().is_empty() {
            return Err(PactError::invalid(format!(
                "view {} missing root_id",
                view.view_id
            )));
        }
        if view.source_branch_id.trim().is_empty() {
            return Err(PactError::invalid(format!(
                "view {} missing source_branch_id",
                view.view_id
            )));
        }
        if !root_store.branches.contains_key(&view.source_branch_id) {
            return Err(PactError::not_found(format!(
                "view {} references missing source branch {}",
                view.view_id, view.source_branch_id
            )));
        }
        if !matches!(
            view.mode.as_str(),
            "readonly" | "live" | "mutable" | "mutable_live"
        ) {
            return Err(PactError::invalid(format!(
                "view {} has unsupported mode {}",
                view.view_id, view.mode
            )));
        }
        if matches!(view.mode.as_str(), "mutable" | "mutable_live")
            && view
                .view_branch_id
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            return Err(PactError::invalid(format!(
                "mutable view {} requires view_branch_id",
                view.view_id
            )));
        }
        if let Some(view_branch_id) = &view.view_branch_id
            && !root_store.branches.contains_key(view_branch_id)
        {
            return Err(PactError::not_found(format!(
                "view {} references missing view branch {}",
                view.view_id, view_branch_id
            )));
        }
        if view_ids.insert(view.view_id.clone(), ()).is_some() {
            return Err(PactError::invalid(format!(
                "duplicate view_id {}",
                view.view_id
            )));
        }
    }
    Ok(())
}

pub(super) fn collect_view_changed_components(view: &PactViewRecord) -> Vec<String> {
    let mut ids = view.changed_component_ids.to_vec();
    ids.sort();
    ids.dedup();
    ids
}

pub(super) fn collect_view_conflicts(
    _view: &PactViewRecord,
    _source_head_revision: Option<u64>,
) -> Vec<Value> {
    Vec::new()
}

pub(super) fn unique_branch_id(root_store: &ContextTreeLedger, base: &str) -> String {
    let mut candidate = base.to_string();
    let mut index = 1_u64;
    while root_store.branches.contains_key(&candidate) {
        candidate = format!("{base}_{index}");
        index += 1;
    }
    candidate
}

pub(super) fn push_index_value(index: &mut BTreeMap<String, Value>, key: &str, value: &str) {
    match index
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(items) => {
            if !items.iter().any(|item| item.as_str() == Some(value)) {
                items.push(Value::String(value.to_string()));
            }
        }
        other => *other = Value::Array(vec![Value::String(value.to_string())]),
    }
}

pub(super) fn collect_aux_namespaces(
    value: &Value,
    component_id: &str,
    index: &mut BTreeMap<String, Value>,
) {
    if let Value::Object(object) = value {
        for key in object.keys() {
            push_index_value(index, key, component_id);
        }
    }
}
