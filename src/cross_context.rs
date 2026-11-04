use crate::components::{Component, PactAttrs};
use crate::error::{PactError, PactResult};
use crate::file_format::{file_create_from_ledger_json, file_open_ledger};
use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
use crate::resolver::PactResolver;
use crate::store::ContextTreeLedger;
use crate::trace::BoundaryKind;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PactContextRef {
    pub root_id: String,
    pub branch_id: Option<String>,
    pub head_ref: Option<crate::store::PactRef>,

    pub path: Option<PathBuf>,
}

impl PactContextRef {
    pub fn at_path(root_id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            root_id: root_id.into(),
            branch_id: None,
            head_ref: None,
            path: Some(path.into()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CrossContextMessage {
    pub from_context: PactContextRef,
    pub to_context: PactContextRef,

    pub parts: Vec<Component>,

    pub source_trace_id: Option<String>,

    pub source_tool_call_id: Option<String>,

    pub created_at: Option<String>,
    pub aux: PactAttrs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryReport {
    pub delivered_component_id: String,

    pub delivery_trace_id: String,

    pub from_root_id: String,

    pub to_root_id: String,

    pub source_trace_id: Option<String>,

    pub source_tool_call_id: Option<String>,

    pub to_branch_id: String,

    pub to_revision: u64,
}

const DEFAULT_DELIVERY_BLOB_INLINE_LIMIT: usize = 0;

pub fn send_context_message(message: CrossContextMessage) -> PactResult<DeliveryReport> {
    if message.parts.is_empty() {
        return Err(PactError::invalid(
            "cross-context message requires at least one part",
        ));
    }
    let target_path = message.to_context.path.as_deref().ok_or_else(|| {
        PactError::invalid("cross-context message target requires a .pact path to deliver into")
    })?;

    let target_ledger = file_open_ledger(target_path)?;
    verify_target_identity(&message.to_context, &target_ledger)?;
    let target_root_id = target_ledger.root_id.clone();

    let target_branch_id = message
        .to_context
        .branch_id
        .clone()
        .unwrap_or_else(|| target_ledger.current_branch_id.clone());
    if !target_ledger.branches.contains_key(&target_branch_id) {
        return Err(PactError::not_found(format!(
            "cross-context message target branch not found: {target_branch_id}"
        )));
    }

    let delivery_trace_id = mint_delivery_trace_id(&message, &target_root_id);
    let component_id = format!("xcmsg_{delivery_trace_id}");

    let message_turn = build_delivery_message_turn(&message, &component_id, &delivery_trace_id)?;

    let component = Component::from_component_value(&message_turn)
        .map_err(|err| PactError::invalid(format!("invalid cross-context message turn: {err}")))?;
    let op = PactOperation::Append {
        envelope: PactOperationEnvelope {
            branch_id: target_branch_id.clone(),
            source: SourceActor::Host,
            source_id: Some("cross_context".to_string()),
            host_timestamp_ns: None,
            idempotency_key: Some(delivery_trace_id.clone()),
            aux: BTreeMap::new(),
        },
        target_component_id: Some(target_ledger.root_id.clone()),
        target_selector: None,
        offset: None,
        component,
    };
    let mut resolver = PactResolver::from_ledger(target_ledger);
    let outcome = resolver.resolve(vec![op], &target_branch_id, BoundaryKind::Authored)?;
    let candidate = outcome.commit_candidate.ok_or_else(|| {
        PactError::resolver("cross-context delivery produced no commit candidate")
    })?;
    resolver.seal(&candidate)?;
    let mutated = resolver.into_ledger();
    let to_revision = mutated
        .branches
        .get(&target_branch_id)
        .map(|branch| branch.head_revision)
        .unwrap_or(0);

    let mutated_json = serde_json::to_string(&mutated).map_err(|err| {
        PactError::codec(format!(
            "failed to encode target ledger after cross-context delivery: {err}"
        ))
    })?;
    file_create_from_ledger_json(
        target_path,
        &mutated_json,
        DEFAULT_DELIVERY_BLOB_INLINE_LIMIT,
    )?;

    Ok(DeliveryReport {
        delivered_component_id: component_id,
        delivery_trace_id,
        from_root_id: message.from_context.root_id.clone(),
        to_root_id: target_root_id,
        source_trace_id: message.source_trace_id.clone(),
        source_tool_call_id: message.source_tool_call_id.clone(),
        to_branch_id: target_branch_id,
        to_revision,
    })
}

fn verify_target_identity(
    to_context: &PactContextRef,
    target_ledger: &ContextTreeLedger,
) -> PactResult<()> {
    if target_ledger.root_id != to_context.root_id {
        return Err(PactError::parent_mismatch(format!(
            "cross-context target root_id {found:?} does not match requested {expected:?}",
            found = target_ledger.root_id,
            expected = to_context.root_id,
        )));
    }
    if let Some(head_ref) = &to_context.head_ref
        && head_ref.root_id != target_ledger.root_id
    {
        return Err(PactError::parent_mismatch(format!(
            "cross-context target head_ref root_id {found:?} does not match target {expected:?}",
            found = head_ref.root_id,
            expected = target_ledger.root_id,
        )));
    }
    Ok(())
}

fn build_delivery_message_turn(
    message: &CrossContextMessage,
    component_id: &str,
    delivery_trace_id: &str,
) -> PactResult<Value> {
    let parts = message
        .parts
        .iter()
        .map(|part| part.project_component_value())
        .collect::<Vec<_>>();

    let mut provenance = json!({
        "kind": "cross_context_message",
        "from_context": context_ref_value(&message.from_context),
        "to_context": context_ref_value(&message.to_context),
        "delivery_trace_id": delivery_trace_id,
    });
    let provenance_object = provenance
        .as_object_mut()
        .ok_or_else(|| PactError::internal("cross-context provenance must be a JSON object"))?;
    if let Some(source_trace_id) = &message.source_trace_id {
        provenance_object.insert(
            "source_trace_id".to_string(),
            Value::String(source_trace_id.clone()),
        );
    }
    if let Some(source_tool_call_id) = &message.source_tool_call_id {
        provenance_object.insert(
            "source_tool_call_id".to_string(),
            Value::String(source_tool_call_id.clone()),
        );
    }
    if let Some(created_at) = &message.created_at {
        provenance_object.insert("created_at".to_string(), Value::String(created_at.clone()));
    }

    let mut attrs = serde_json::Map::new();
    attrs.insert("cross_context_message".to_string(), provenance);
    for (key, value) in &message.aux {
        attrs.entry(key.clone()).or_insert_with(|| value.clone());
    }

    Ok(json!({
        "id": component_id,
        "componentType": "message_turn",
        "type": "message_turn",
        "role": "user",
        "pact_depth": 0,
        "offset": 0,
        "attrs": Value::Object(attrs),
        "children": parts,
    }))
}

fn context_ref_value(context: &PactContextRef) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "root_id".to_string(),
        Value::String(context.root_id.clone()),
    );
    if let Some(branch_id) = &context.branch_id {
        object.insert("branch_id".to_string(), Value::String(branch_id.clone()));
    }
    if let Some(head_ref) = &context.head_ref {
        object.insert(
            "head_ref".to_string(),
            serde_json::to_value(head_ref).unwrap_or(Value::Null),
        );
    }
    Value::Object(object)
}

fn mint_delivery_trace_id(message: &CrossContextMessage, target_root_id: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut feed = |bytes: &[u8]| {
        hasher.update(bytes);
        hasher.update(&[0]);
    };
    feed(message.from_context.root_id.as_bytes());
    feed(target_root_id.as_bytes());
    feed(
        message
            .to_context
            .branch_id
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    feed(message.source_trace_id.as_deref().unwrap_or("").as_bytes());
    feed(
        message
            .source_tool_call_id
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    feed(message.created_at.as_deref().unwrap_or("").as_bytes());
    for part in &message.parts {
        let value = part.project_component_value();
        feed(value.to_string().as_bytes());
    }
    let digest = hasher.finalize();
    digest.as_bytes()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn text_message_part(text: &str) -> PactResult<Component> {
    let value = json!({
        "id": format!("xcpart_{}", short_hash(text.as_bytes())),
        "componentType": "text_content",
        "type": "text_content",
        "content": text,
    });
    Component::from_component_value(&value)
        .map_err(|err| PactError::invalid(format!("invalid text message part: {err}")))
}

fn short_hash(bytes: &[u8]) -> String {
    let digest = blake3::hash(bytes);
    digest.as_bytes()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
