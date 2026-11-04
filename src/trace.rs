use crate::boundary::ExternalMutationRequirement;
use crate::store::PactRef;
use crate::transaction_trace::TraceOp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const RESOLVED_BOUNDARY_SCHEMA: &str = "pact.resolved_boundary.v1";
pub const RESOLVE_OUTCOME_SCHEMA: &str = "pact.resolve_outcome.v1";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryKind {
    Authored,

    #[serde(
        rename = "Preflight",
        alias = "pre_request",
        alias = "before_provider_call"
    )]
    PreRequest,

    PostRequestAlteration,

    Request,

    Response,

    PostResponseAlteration,

    #[serde(
        rename = "PostResponse",
        alias = "post_response",
        alias = "after_provider_response"
    )]
    PostResponse,

    Adhoc,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RejectedOperation {
    pub kind: String,
    pub reason: String,

    pub payload: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResolveOutcome {
    #[serde(default = "default_resolve_outcome_schema")]
    pub schema: String,
    pub branch_id: String,
    pub boundary: BoundaryKind,

    #[serde(default)]
    pub accepted: usize,
    #[serde(default)]
    pub deduped: usize,
    pub applied: Vec<String>,
    pub rejected: Vec<RejectedOperation>,

    #[serde(default)]
    pub produced_component_ids: Vec<String>,

    #[serde(default)]
    pub produced_message_ids: Vec<String>,

    #[serde(default)]
    pub produced_message_child_ids: Vec<String>,

    #[serde(default)]
    pub replaced_component_ids: Vec<String>,

    #[serde(default)]
    pub deleted_component_ids: Vec<String>,

    #[serde(default)]
    pub affected_component_ids: Vec<String>,

    #[serde(default)]
    pub pending_external_mutations: Vec<ExternalMutationRequirement>,
    pub dirty_projections: Vec<String>,

    pub traces: Vec<ResolvedOperationTrace>,

    pub outcome: crate::boundary::BoundaryOutcome,

    pub commit_candidate: Option<ResolvedBoundary>,
}

fn default_resolve_outcome_schema() -> String {
    RESOLVE_OUTCOME_SCHEMA.to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResolvedOperationTrace {
    pub trace_id: String,
    pub op_kind: String,
    pub idempotency_key: Option<String>,
    pub created_at_ns: u64,

    #[serde(default)]
    pub ops: Vec<TraceOp>,

    pub produced_refs: Vec<PactRef>,
    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "target", content = "value", rename_all = "snake_case")]
pub enum StagedTraceTarget {
    TraceId(String),
    IdempotencyKey(String),
    OperationKind(String),
    ComponentId(String),
    Selector(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResolvedBoundary {
    #[serde(default = "default_resolved_boundary_schema")]
    pub schema: String,
    pub branch_id: String,
    pub boundary: BoundaryKind,
    pub before_ref: PactRef,
    pub after_ref: PactRef,
    pub applied: Vec<String>,
    pub traces: Vec<ResolvedOperationTrace>,
    pub dirty_projections: Vec<String>,

    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

fn default_resolved_boundary_schema() -> String {
    RESOLVED_BOUNDARY_SCHEMA.to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CommitRef {
    pub branch_id: String,
    pub revision: u64,
    pub boundary: BoundaryKind,
    pub before_ref: PactRef,
    pub after_ref: PactRef,
}

#[cfg(test)]
mod tests;
