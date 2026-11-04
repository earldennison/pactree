use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::commit::boundary_label;
use crate::components::{Component, ComponentKind, MessageTurnPayload, TextPayload};
use crate::error::{PactError, PactResult};
use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
use crate::store::{
    PactCommit, PactRef, PactRefSpec, PactTransactionTrace, PactTransactionTraceBuildRequest,
};
use crate::trace::{BoundaryKind, ResolvedOperationTrace};
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

#[cfg(test)]
mod tests;

pub(crate) trait AuthoringIdentity {
    fn next_local_id(&mut self) -> String;

    fn next_node_id(&mut self, prefix: &str) -> String;

    fn next_timestamp_ns(&mut self) -> u64;
}

pub(crate) struct DeterministicIdentity {
    state: super::DeterministicAuthoringState,
    node_counter: u64,
}

impl DeterministicIdentity {
    pub(crate) fn new(options: crate::DeterministicAuthoringOptions) -> PactResult<Self> {
        Ok(Self {
            state: super::DeterministicAuthoringState::from_options(options)?,
            node_counter: 0,
        })
    }
}

impl AuthoringIdentity for DeterministicIdentity {
    fn next_local_id(&mut self) -> String {
        self.state.next_local_id()
    }

    fn next_node_id(&mut self, prefix: &str) -> String {
        self.node_counter = self.node_counter.saturating_add(1);
        format!("{prefix}_{}", self.state.next_local_id())
    }

    fn next_timestamp_ns(&mut self) -> u64 {
        self.state.next_timestamp_ns()
    }
}

#[derive(Default)]
pub(crate) struct NativeIdentity {
    node_counter: u64,
}

impl AuthoringIdentity for NativeIdentity {
    fn next_local_id(&mut self) -> String {
        crate::components::mint_component_local_id()
    }

    fn next_node_id(&mut self, prefix: &str) -> String {
        self.node_counter = self.node_counter.saturating_add(1);
        format!(
            "{prefix}_{:x}_{:x}",
            super::commit::monotonic_ns(),
            self.node_counter
        )
    }

    fn next_timestamp_ns(&mut self) -> u64 {
        super::commit::monotonic_ns()
    }
}

impl AuthoringIdentity for super::PactResolver {
    fn next_local_id(&mut self) -> String {
        self.next_authoring_component_local_id()
    }

    fn next_node_id(&mut self, prefix: &str) -> String {
        super::PactResolver::next_node_id(self, prefix)
    }

    fn next_timestamp_ns(&mut self) -> u64 {
        self.next_authoring_timestamp_ns()
    }
}

pub(crate) fn author_components(
    context_id: &str,
    parts: Vec<Component>,
    identity: &mut impl AuthoringIdentity,
    is_used: &mut impl FnMut(&str) -> PactResult<bool>,
    authored: &mut std::collections::BTreeSet<String>,
) -> PactResult<Vec<Component>> {
    parts
        .into_iter()
        .map(|part| author_component(context_id, part, None, identity, is_used, authored))
        .collect()
}

fn author_component(
    context_id: &str,
    mut component: Component,
    parent_id: Option<&str>,
    identity: &mut impl AuthoringIdentity,
    is_used: &mut impl FnMut(&str) -> PactResult<bool>,
    authored: &mut std::collections::BTreeSet<String>,
) -> PactResult<Component> {
    let local = if component.id.is_empty() {
        identity.next_local_id()
    } else {
        local_part(&component.id).to_string()
    };
    let mut id = format!("{context_id}:{local}");
    while authored.contains(&id) || is_used(&id)? {
        id = format!("{context_id}:{}", identity.next_local_id());
    }
    authored.insert(id.clone());
    component.id = id;
    component.parent_id = parent_id.map(str::to_string);
    let parent_id = component.id.clone();
    let children = std::mem::take(&mut component.children);
    component.children = children
        .into_iter()
        .map(|child| {
            author_component(
                context_id,
                child,
                Some(&parent_id),
                identity,
                is_used,
                authored,
            )
        })
        .collect::<PactResult<Vec<_>>>()?;
    Ok(component)
}

pub(crate) fn local_part(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

pub(crate) fn text_component(
    id: String,
    content: String,
    parent_id: Option<String>,
    offset: i32,
    pact_depth: Option<i32>,
) -> Component {
    Component {
        id,
        subtype: Some("text_content".to_string()),
        parent_id,
        role: None,
        offset,
        pact_depth,
        priority: 0,
        transaction_index: 0,
        created_at_ns: 0,
        created_at_iso: None,
        creation_index: 0,
        key: None,
        tags: Vec::new(),
        aux: BTreeMap::new(),
        kind: ComponentKind::Text(TextPayload {
            content: Some(content),
            content_ref: None,
            source_ref: None,
            content_capture: None,
            content_status: None,
        }),
        provenance: None,
        lifecycle: Default::default(),
        external_refs: Vec::new(),
        children: Vec::new(),
    }
}

pub(crate) fn message_turn_component(
    id: String,
    root_id: &str,
    role: &str,
    children: Vec<Component>,
    aux: BTreeMap<String, Value>,
) -> PactResult<Component> {
    if children.is_empty() {
        return Err(PactError::invalid(
            "message must contain at least one component",
        ));
    }
    Ok(Component {
        id,
        subtype: Some("message_turn".to_string()),
        parent_id: Some(root_id.to_string()),
        role: Some(role.to_string()),
        offset: 0,
        pact_depth: Some(if role == "user" { 0 } else { 1 }),
        priority: 0,
        transaction_index: 0,
        created_at_ns: 0,
        created_at_iso: None,
        creation_index: 0,
        key: None,
        tags: Vec::new(),
        aux,
        kind: ComponentKind::MessageTurn(MessageTurnPayload {}),
        provenance: None,
        lifecycle: Default::default(),
        external_refs: Vec::new(),
        children,
    })
}

pub(crate) fn lower_message(
    root_id: &str,
    branch_id: &str,
    role: &str,
    parts: Vec<Component>,
    aux: BTreeMap<String, Value>,
    identity: &mut impl AuthoringIdentity,
) -> PactResult<Option<PactOperation>> {
    let source = match role {
        "system" => SourceActor::System,
        "user" => SourceActor::User,
        "assistant" => SourceActor::Assistant,
        other => {
            return Err(PactError::invalid(format!(
                "unsupported message role: {other}"
            )));
        }
    };
    let envelope = PactOperationEnvelope {
        branch_id: branch_id.to_string(),
        source,
        source_id: None,
        host_timestamp_ns: Some(identity.next_timestamp_ns()),
        idempotency_key: None,
        aux: aux.clone(),
    };

    if role == "system" {
        let mut text_parts = Vec::new();
        for part in parts {
            if part.component_type() == "text_content"
                && let Some(text) = part.content()
            {
                if !text.is_empty() {
                    text_parts.push(text.to_string());
                }
            } else {
                return Err(PactError::invalid("system messages only accept text parts"));
            }
        }
        if text_parts.is_empty() {
            return Ok(None);
        }
        let component_id = identity.next_node_id("sys");
        let component = text_component(
            component_id,
            text_parts.join("\n"),
            Some("system".to_string()),
            0,
            Some(-1),
        );
        return Ok(Some(PactOperation::Insert {
            envelope,
            target_selector: None,
            parent_id: Some("system".to_string()),
            depth: None,
            offset: None,
            component,
        }));
    }

    let message_id = identity.next_node_id("msg");
    let mut children: Vec<Component> = Vec::new();
    for mut part in parts {
        if part.id.is_empty() {
            part.id = identity.next_node_id("node");
        }
        part.parent_id = Some(message_id.clone());
        part.offset = children.len() as i32;
        part.pact_depth.get_or_insert(1);
        children.push(part);
    }
    let component = message_turn_component(message_id, root_id, role, children, aux)?;
    Ok(Some(PactOperation::Insert {
        envelope,
        target_selector: None,
        parent_id: Some(root_id.to_string()),
        depth: None,
        offset: None,
        component,
    }))
}

pub(crate) fn stamp_component_created_at(component: &mut Component, created_at_ns: u64) {
    if component.created_at_ns == 0 {
        component.created_at_ns = created_at_ns;
    }
    for child in &mut component.children {
        stamp_component_created_at(child, created_at_ns);
    }
}

pub(crate) fn stamp_insert_identity(op: &mut PactOperation, identity: &mut impl AuthoringIdentity) {
    if let PactOperation::Insert {
        envelope,
        component,
        ..
    } = op
    {
        if component.id.is_empty() {
            component.id = identity.next_node_id("node");
        }
        let timestamp = match envelope.host_timestamp_ns {
            Some(timestamp) => timestamp,
            None => {
                let timestamp = identity.next_timestamp_ns();
                envelope.host_timestamp_ns = Some(timestamp);
                timestamp
            }
        };
        stamp_component_created_at(component, timestamp);
    }
}

pub(crate) fn trace_identity(op: &PactOperation, created_at_ns: u64, counter: u64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(op.kind().as_bytes());
    hasher.update(&created_at_ns.to_le_bytes());
    hasher.update(&counter.to_le_bytes());
    if let Ok(encoded) = serde_json::to_vec(op) {
        hasher.update(&encoded);
    }
    let hash = hasher.finalize();
    format!("trace_{}", &hash.to_hex()[..16])
}

pub(crate) fn insert_subtree_trace_ops(
    component: &Component,
    parent_id: &str,
    index: usize,
    parent_child_ids: Option<&[String]>,
    sink: &mut impl FnMut(TraceOp) -> PactResult<()>,
) -> PactResult<()> {
    let mut payload = component.project_component_value();
    if let Some(object) = payload.as_object_mut()
        && object.contains_key("children")
    {
        object.insert("children".to_string(), Value::Array(Vec::new()));
    }
    sink(TraceOp::new(
        component.id.clone(),
        TraceOpKind::InsertComponent {
            parent_id: parent_id.to_string(),
            index,
            component: payload,
        },
    ))?;
    if let Some(parent_child_ids) = parent_child_ids {
        sink(child_ids_trace_op(parent_id, parent_child_ids))?;
    }
    let child_ids = component
        .children
        .iter()
        .map(|child| child.id.clone())
        .collect::<Vec<_>>();
    for (child_index, child) in component.children.iter().enumerate() {
        insert_subtree_trace_ops(child, &component.id, child_index, Some(&child_ids), sink)?;
    }
    Ok(())
}

pub(crate) fn child_ids_trace_op(component_id: &str, child_ids: &[String]) -> TraceOp {
    TraceOp::new(
        component_id.to_string(),
        TraceOpKind::SetField {
            key: "child_ids".to_string(),
            value: Value::Array(
                child_ids
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        },
    )
}

pub(crate) fn depth_trace_op(component_id: &str, depth: i64) -> TraceOp {
    TraceOp::new(
        component_id.to_string(),
        TraceOpKind::SetField {
            key: "pact_depth".to_string(),
            value: Value::Number(depth.into()),
        },
    )
}

pub(crate) fn produced_refs_for_insert(
    component: &Component,
    root_id: &str,
    branch_id: &str,
) -> Vec<PactRef> {
    let mut refs = vec![PactRef {
        root_id: root_id.to_string(),
        branch_id: branch_id.to_string(),
        revision: None,
        transaction_id: None,
        component_id: Some(component.id.clone()),
        content_ref: None,
    }];
    super::api::collect_node_content_refs(component, root_id, branch_id, &mut refs);
    refs
}

pub(crate) fn pact_ref_spec_from_ref(pact_ref: &PactRef) -> PactRefSpec {
    PactRefSpec {
        revision: pact_ref.revision,
        component_id: pact_ref.component_id.clone(),
        content_ref: pact_ref.content_ref.clone(),
    }
}

pub(crate) struct SealedTraceRequest<'a> {
    pub(crate) root_id: &'a str,
    pub(crate) branch_id: &'a str,
    pub(crate) next_rev: u64,
    pub(crate) boundary: BoundaryKind,
    pub(crate) before_ref: &'a PactRef,
    pub(crate) after_ref: &'a PactRef,
    pub(crate) applied: Vec<String>,
    pub(crate) events: &'a [ResolvedOperationTrace],
    pub(crate) extensions: Vec<crate::PactExtensionRecord>,
    pub(crate) replay_trace: Option<TransactionTrace>,
    pub(crate) aux: BTreeMap<String, Value>,
}

pub(crate) fn build_sealed_public_trace(
    request: SealedTraceRequest<'_>,
) -> PactResult<PactTransactionTrace> {
    let SealedTraceRequest {
        root_id,
        branch_id,
        next_rev,
        boundary,
        before_ref,
        after_ref,
        applied,
        events,
        extensions,
        replay_trace,
        mut aux,
    } = request;
    let transaction_id = format!("tx_{branch_id}_{next_rev}");
    let authored_ref = events
        .iter()
        .flat_map(|trace| trace.produced_refs.iter())
        .find(|pact_ref| pact_ref.content_ref.is_some())
        .map(pact_ref_spec_from_ref)
        .unwrap_or_else(|| PactRefSpec {
            revision: before_ref.revision,
            component_id: before_ref.component_id.clone(),
            content_ref: before_ref.content_ref.clone(),
        });
    let after_ref = PactRef {
        revision: Some(next_rev),
        ..after_ref.clone()
    };
    if !extensions.is_empty() {
        crate::extension::validate_extension_batch(&extensions)?;
        aux.insert(
            crate::PACT_EXTENSIONS_AUX_KEY.to_string(),
            serde_json::to_value(extensions).map_err(|error| {
                PactError::codec(format!("failed to encode staged Pact extensions: {error}"))
            })?,
        );
    }
    aux.insert("boundary".to_string(), json!(boundary_label(boundary)));
    aux.insert("applied".to_string(), json!(applied));
    aux.insert(
        "events".to_string(),
        Value::Array(
            events
                .iter()
                .map(|trace| {
                    json!({
                        "trace_id": trace.trace_id.clone(),
                        "op_kind": trace.op_kind.clone(),
                        "created_at_ns": trace.created_at_ns,
                        "produced_refs": trace.produced_refs.clone(),
                        "aux": trace.aux.clone(),
                    })
                })
                .collect(),
        ),
    );
    PactTransactionTrace::build(PactTransactionTraceBuildRequest {
        schema: crate::store::TRANSACTION_TRACE_BUILD_REQUEST_SCHEMA.to_string(),
        root_id: root_id.to_string(),
        branch_id: branch_id.to_string(),
        transaction_id: transaction_id.clone(),
        authored_ref,
        pre_request_ref: pact_ref_spec_from_ref(before_ref),
        post_request_alteration_ref: pact_ref_spec_from_ref(&after_ref),
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: Some(pact_ref_spec_from_ref(&after_ref)),
        alteration_ids: applied,
        commit: Some(PactCommit {
            commit_id: format!("commit_{branch_id}_{next_rev}"),
            phase: boundary_label(boundary).to_string(),
            kind: "pact_state".to_string(),
            pact_ref: Some(after_ref),
            content_ref: None,
            created_at: None,
        }),
        replay_trace,
        aux,
    })
}
