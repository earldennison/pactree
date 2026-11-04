use super::commit::{
    IngestOutcome, boundary_label, find_node_value, monotonic_ns, operation_report_refs,
};
use super::lifecycle::{lifecycle_cadence_clone_tree, lifecycle_expired_top_components};
use super::message_authoring::{
    SealedTraceRequest, build_sealed_public_trace, lower_message, stamp_component_created_at,
    trace_identity,
};
use super::mutations::op_to_mutations;
use super::payloads::externalize_mutation_payloads;
use super::tools::{
    is_empty_message_turn_noop, prune_hollow_message_turns, sync_tool_pair_lifecycle,
};
use super::{PactResolver, StagedTraceRecord};
use crate::atomic_output::{PublishMode, publish_bytes};
use crate::boundary::{
    BoundaryOutcome, BoundaryPolicy, ExternalMutationKind, ExternalMutationRequirement,
    PendingExternalMutations,
};
use crate::components::Component;
use crate::error::{PactError, PactResult};
use crate::mutation::{
    MUTATION_APPLY_RESULT_SCHEMA, MutationApplyResult, MutationKind, MutationRequest,
    MutationTarget, OperationReport, apply_mutation_tree,
};
use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
use crate::payload_hydration::hydrate_payload_refs;
use crate::selector::SelectedComponent;
use crate::store::{
    ContextTreeLedger, PactBranch, PactRef, PactTransactionTrace, PactView, build_projection,
    build_projection_from_component, externalize_large_content_in_value,
    project_branch_component_tree,
};
use crate::tools::{TOOL_RECORD_SCHEMA, ToolCallRecord, ToolCallStatus};
use crate::trace::{
    BoundaryKind, CommitRef, RESOLVE_OUTCOME_SCHEMA, RESOLVED_BOUNDARY_SCHEMA, RejectedOperation,
    ResolveOutcome, ResolvedBoundary, ResolvedOperationTrace, StagedTraceTarget,
};
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};
use crate::tree::ComponentTree;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileAttachmentIdentity {
    root_id: String,
    branch_id: String,
    head_revision: u64,
    semantic_digest: [u8; 32],
}

impl PactResolver {
    pub fn list_tool_calls(&self) -> Vec<ToolCallRecord> {
        self.current_tree_value()
            .map(|tree| tool_call_records_from_tree(&tree, &self.ledger.current_branch_id))
            .unwrap_or_default()
    }

    pub fn unpaired_tool_call_ids(&self) -> Vec<String> {
        self.current_component_tree()
            .map(|tree| crate::provider_thread::unpaired_tool_call_ids_tree(&tree))
            .unwrap_or_default()
    }

    fn current_component_tree(&self) -> PactResult<ComponentTree> {
        let branch_id = &self.ledger.current_branch_id;
        self.branch_component_tree(branch_id)
    }

    fn current_tree_value(&self) -> PactResult<Value> {
        self.current_component_tree()
            .map(|tree| tree.project_value())
    }

    fn branch_tree_value(&self, branch_id: &str) -> PactResult<Value> {
        self.branch_component_tree(branch_id)
            .map(|tree| tree.project_value())
    }

    pub(crate) fn branch_component_tree(&self, branch_id: &str) -> PactResult<ComponentTree> {
        if let Some((tree, _)) = self.staged_state.get(branch_id) {
            return Ok(tree.clone());
        }
        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        project_branch_component_tree(&self.ledger, branch, Some(branch.head_revision))
    }

    pub(crate) fn branch_component_id_set(&self, branch_id: &str) -> PactResult<BTreeSet<String>> {
        if let Some((tree, _)) = self.staged_state.get(branch_id) {
            return Ok(tree.component_ids().map(str::to_string).collect());
        }
        let tree = self.branch_component_tree(branch_id)?;
        Ok(tree.component_ids().map(str::to_string).collect())
    }

    pub fn select(
        &self,
        branch_id: &str,
        selector: &str,
        include_archived: bool,
    ) -> PactResult<Vec<SelectedComponent>> {
        let tree = self.branch_component_tree(branch_id)?;
        crate::selector::select_components_tree(&tree, selector, include_archived)
    }

    pub fn archive_component_tree(&self, branch_id: &str) -> PactResult<ComponentTree> {
        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        let (tree, _first_seen) =
            crate::store::project_branch_archive_component_tree(&self.ledger, branch, None)?;
        Ok(tree)
    }

    pub fn open_view_read_only_live(
        &self,
        source_branch_id: &str,
    ) -> Result<crate::view::ViewRef, crate::view::ViewError> {
        crate::view::open_read_only_live(&self.ledger, source_branch_id)
    }

    pub fn open_view_read_only_pinned(
        &self,
        source_branch_id: &str,
        revision: Option<u64>,
    ) -> Result<crate::view::ViewRef, crate::view::ViewError> {
        crate::view::open_read_only_pinned(&self.ledger, source_branch_id, revision)
    }

    pub fn open_view_mutable_consumer(
        &mut self,
        source_branch_id: &str,
        consumer_branch_id: &str,
    ) -> Result<crate::view::ViewRef, crate::view::ViewError> {
        crate::view::open_mutable_consumer(
            std::sync::Arc::make_mut(&mut self.ledger),
            source_branch_id,
            consumer_branch_id,
        )
    }

    pub fn resolve_view(
        &self,
        view_ref: &crate::view::ViewRef,
    ) -> Result<crate::store::PactRef, crate::view::ViewError> {
        crate::view::resolve_view(&self.ledger, view_ref)
    }

    pub fn preview_view(&self, branch_id: &str) -> PactResult<PactView> {
        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        let Some((staged_tree, _)) = self.staged_state.get(branch_id) else {
            return Ok(branch.current.clone());
        };

        let next_rev = branch.head_revision + 1;
        let mut first_seen = branch.current.tree.first_seen.clone();
        for component_id in staged_tree.component_ids() {
            first_seen
                .entry(component_id.to_string())
                .or_insert(next_rev);
        }
        let staged_tree = staged_tree.project_value();
        let projection = build_projection(&staged_tree, &first_seen, self.blob_inline_limit)?;
        Ok(PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_id.to_string(),
            next_rev,
            branch.current.lifecycle_seen.clone(),
            branch.lifecycle_tick + 1,
        ))
    }

    pub fn preview_view_json(&self, branch_id: &str) -> PactResult<String> {
        let view = self.preview_view(branch_id)?;
        serde_json::to_string(&view)
            .map_err(|err| PactError::codec(format!("failed to encode preview view: {err}")))
    }

    pub fn build_provider_thread(
        &self,
        branch_id: &str,
        revision: Option<u64>,
        options: &crate::provider_thread::ProviderThreadOptions,
    ) -> Result<Value, crate::provider_thread::ProviderThreadError> {
        let branch = self.ledger.branches.get(branch_id).ok_or_else(|| {
            crate::provider_thread::ProviderThreadError {
                schema: crate::provider_thread::PROVIDER_THREAD_SCHEMA.to_string(),
                reason: format!("branch not found: {branch_id}"),
                offending_tool_call_ids: vec![],
            }
        })?;

        let tree = if let Some(target) = revision {
            project_branch_component_tree(&self.ledger, branch, Some(target)).map_err(|err| {
                crate::provider_thread::ProviderThreadError {
                    schema: crate::provider_thread::PROVIDER_THREAD_SCHEMA.to_string(),
                    reason: format!("failed to project branch: {err}"),
                    offending_tool_call_ids: vec![],
                }
            })?
        } else if let Some((staged_tree, _)) = self.staged_state.get(branch_id) {
            staged_tree.clone()
        } else {
            project_branch_component_tree(&self.ledger, branch, Some(branch.head_revision))
                .map_err(|err| crate::provider_thread::ProviderThreadError {
                    schema: crate::provider_thread::PROVIDER_THREAD_SCHEMA.to_string(),
                    reason: format!("failed to project branch: {err}"),
                    offending_tool_call_ids: vec![],
                })?
        };
        crate::provider_thread::build_provider_thread_tree_with_content_store(
            &tree,
            &self.ledger.content,
            options,
        )
    }

    pub fn seal_pre_flight(
        &mut self,
        branch_id: &str,
        options: &crate::provider_thread::ProviderThreadOptions,
    ) -> PactResult<Value> {
        let (commit_ref, resolved) =
            self.seal_staged_with_cycle_policy_report(branch_id, BoundaryKind::PreRequest, true)?;
        let revision = commit_ref.as_ref().map(|commit_ref| commit_ref.revision);
        let thread = self.build_provider_thread(branch_id, revision, options)?;
        let produced_component_ids = resolved.produced_component_ids.clone();
        let produced_message_ids = resolved.produced_message_ids.clone();
        let produced_message_child_ids = resolved.produced_message_child_ids.clone();
        let replaced_component_ids = resolved.replaced_component_ids.clone();
        let deleted_component_ids = resolved.deleted_component_ids.clone();
        let affected_component_ids = resolved.affected_component_ids.clone();
        Ok(json!({
            "schema": "pact.seal_pre_flight.v1",
            "branch_id": branch_id,
            "thread": thread,
            "commit_ref": commit_ref,
            "produced_component_ids": produced_component_ids,
            "produced_message_ids": produced_message_ids,
            "produced_message_child_ids": produced_message_child_ids,
            "replaced_component_ids": replaced_component_ids,
            "deleted_component_ids": deleted_component_ids,
            "affected_component_ids": affected_component_ids,
            "resolved": resolved,
        }))
    }

    pub fn bind_file(&mut self, path: PathBuf) {
        self.file_path = Some(path);
        self.file_manifest = None;
        self.persisted_content_ids.clear();
    }

    pub(crate) fn bind_created_file(&mut self, path: PathBuf) {
        self.file_path = Some(path);
        self.file_manifest = None;
        self.persisted_content_ids = self.ledger.content.records.keys().cloned().collect();
    }

    pub(crate) fn attach_file(&mut self, path: PathBuf, branch_id: &str) -> PactResult<()> {
        let attached = crate::pact_file::open_ledger(&path)?;
        let expected = file_attachment_identity(&self.ledger, branch_id)?;
        let actual = file_attachment_identity(&attached, branch_id)?;
        if expected != actual {
            return Err(PactError::storage(format!(
                "file identity mismatch: expected root={} branch={} revision={} digest={}, got root={} branch={} revision={} digest={}",
                expected.root_id,
                expected.branch_id,
                expected.head_revision,
                hex_digest(&expected.semantic_digest),
                actual.root_id,
                actual.branch_id,
                actual.head_revision,
                hex_digest(&actual.semantic_digest),
            )));
        }
        self.file_path = Some(path);
        self.file_manifest = Some(super::candidate_manifest(
            self.file_path
                .as_deref()
                .expect("file path was just assigned"),
            &attached,
        ));
        self.persisted_content_ids = attached.content.records.keys().cloned().collect();
        Ok(())
    }

    pub fn ledger(&self) -> &ContextTreeLedger {
        &self.ledger
    }

    pub fn head(&self) -> PactResult<&PactView> {
        let branch_id = &self.ledger.current_branch_id;
        self.ledger
            .branches
            .get(branch_id)
            .map(|branch| &branch.current)
            .ok_or_else(|| PactError::not_found(format!("current branch `{branch_id}` not found")))
    }

    pub fn pending(&self, branch_id: &str) -> &[PactOperation] {
        self.pending
            .get(branch_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn add_message(
        &mut self,
        branch_id: &str,
        role: &str,
        parts: Vec<Component>,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<IngestOutcome> {
        let ops = self.message_insert_ops(branch_id, role, parts, aux)?;
        self.ingest_operations(ops)
    }

    pub fn add_message_json(
        &mut self,
        branch_id: &str,
        role: &str,
        parts: Vec<Value>,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<IngestOutcome> {
        let parts = self.message_parts_from_values(parts)?;
        self.add_message(branch_id, role, parts, aux)
    }

    pub fn receive(
        &mut self,
        branch_id: &str,
        parts: Vec<Component>,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<Value> {
        let expected_tool_call_ids = tool_call_ids_from_components(&parts);
        let ingested = self.add_message(branch_id, "assistant", parts, aux)?;
        self.seal_received_response(
            branch_id,
            expected_tool_call_ids,
            ingested.accepted,
            ingested.deduped,
            ingested.rejected,
        )
    }

    pub fn receive_staged(
        &mut self,
        branch_id: &str,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<Value> {
        let _ = aux;
        self.seal_received_response(branch_id, Vec::new(), 0, 0, Vec::new())
    }

    pub fn receive_json(
        &mut self,
        branch_id: &str,
        parts: Option<Vec<Value>>,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<Value> {
        match parts {
            Some(parts) => {
                let parts = self.message_parts_from_values(parts)?;
                self.receive(branch_id, parts, aux)
            }
            None => self.receive_staged(branch_id, aux),
        }
    }

    fn seal_received_response(
        &mut self,
        branch_id: &str,
        expected_tool_call_ids: Vec<String>,
        accepted: usize,
        deduped: usize,
        rejected: Vec<RejectedOperation>,
    ) -> PactResult<Value> {
        let resolved = self.resolve(Vec::new(), branch_id, BoundaryKind::PostResponse)?;
        if !resolved.rejected.is_empty() {
            return Err(PactError::resolver(format!(
                "provider response rejected by Pact: {}",
                resolved
                    .rejected
                    .iter()
                    .map(|item| item.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        self.verify_staged_tool_calls(branch_id, &expected_tool_call_ids)?;
        let mut report = self.seal_resolved_post_response(branch_id, resolved)?;
        if let Some(object) = report.as_object_mut() {
            object.insert(
                "schema".to_string(),
                Value::String("pact.receive.v1".to_string()),
            );
            object.insert("accepted".to_string(), json!(accepted));
            object.insert("deduped".to_string(), json!(deduped));
            object.insert("rejected".to_string(), json!(rejected));
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_json(
        &mut self,
        branch_id: &str,
        target_selector: Option<String>,
        parent_id: Option<String>,
        depth: Option<i32>,
        offset: Option<i32>,
        mut component: Value,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        self.ensure_operation_node_id(&mut component, None);
        let component = component_from_authoring_value(component, "insert")?;
        self.insert_component(
            branch_id,
            target_selector,
            parent_id,
            depth,
            offset,
            component,
            boundary,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_component(
        &mut self,
        branch_id: &str,
        target_selector: Option<String>,
        parent_id: Option<String>,
        depth: Option<i32>,
        offset: Option<i32>,
        component: Component,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let mut op = PactOperation::Insert {
            envelope: host_operation_envelope(branch_id),
            target_selector,
            parent_id,
            depth,
            offset,
            component,
        };
        self.stamp_typed_operation(&mut op);
        self.resolve_and_seal_operation(branch_id, op, boundary)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        content: Option<String>,
        content_ref: Option<String>,
        fields: BTreeMap<String, Value>,
        aux: BTreeMap<String, Value>,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let op = PactOperation::Update {
            envelope: host_operation_envelope(branch_id),
            target_component_id,
            target_selector,
            content,
            content_ref,
            fields,
            aux,
        };
        self.resolve_and_seal_operation(branch_id, op, boundary)
    }

    pub fn replace_json(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        mut component: Value,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        self.ensure_operation_node_id(&mut component, target_component_id.as_deref());
        let component = component_from_authoring_value(component, "replace")?;
        self.replace_component(
            branch_id,
            target_component_id,
            target_selector,
            component,
            boundary,
        )
    }

    pub fn replace_component(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        component: Component,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let mut op = PactOperation::Replace {
            envelope: host_operation_envelope(branch_id),
            target_component_id,
            target_selector,
            component,
        };
        self.stamp_typed_operation(&mut op);
        self.resolve_and_seal_operation(branch_id, op, boundary)
    }

    pub fn append_json(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        offset: Option<i32>,
        mut component: Value,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        self.ensure_operation_node_id(&mut component, None);
        let component = component_from_authoring_value(component, "append")?;
        self.append_component(
            branch_id,
            target_component_id,
            target_selector,
            offset,
            component,
            boundary,
        )
    }

    pub fn append_component(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        offset: Option<i32>,
        component: Component,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let mut op = PactOperation::Append {
            envelope: host_operation_envelope(branch_id),
            target_component_id,
            target_selector,
            offset,
            component,
        };
        self.stamp_typed_operation(&mut op);
        self.resolve_and_seal_operation(branch_id, op, boundary)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_json(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        parent_id: Option<String>,
        depth: Option<i32>,
        offset: Option<i32>,
        mut component: Value,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        self.ensure_operation_node_id(&mut component, target_component_id.as_deref());
        let component = component_from_authoring_value(component, "upsert")?;
        self.upsert_component(
            branch_id,
            target_component_id,
            target_selector,
            parent_id,
            depth,
            offset,
            component,
            boundary,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_component(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        parent_id: Option<String>,
        depth: Option<i32>,
        offset: Option<i32>,
        component: Component,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let mut op = PactOperation::Upsert {
            envelope: host_operation_envelope(branch_id),
            target_component_id,
            target_selector,
            parent_id,
            depth,
            offset,
            component,
        };
        self.stamp_typed_operation(&mut op);
        self.resolve_and_seal_operation(branch_id, op, boundary)
    }

    pub fn delete(
        &mut self,
        branch_id: &str,
        target_component_id: Option<String>,
        target_selector: Option<String>,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let op = PactOperation::Delete {
            envelope: host_operation_envelope(branch_id),
            target_component_id,
            target_selector,
        };
        self.resolve_and_seal_operation(branch_id, op, boundary)
    }

    fn resolve_and_seal_operation(
        &mut self,
        branch_id: &str,
        op: PactOperation,
        boundary: BoundaryKind,
    ) -> PactResult<Value> {
        let op_kind = op.kind();
        let resolved = self.resolve(vec![op], branch_id, boundary)?;
        if !resolved.rejected.is_empty() {
            return Err(PactError::resolver(format!(
                "{op_kind} rejected by Pact: {}",
                resolved
                    .rejected
                    .iter()
                    .map(|item| item.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        let produced_component_ids = resolved.produced_component_ids.clone();
        let produced_message_ids = resolved.produced_message_ids.clone();
        let produced_message_child_ids = resolved.produced_message_child_ids.clone();
        let replaced_component_ids = resolved.replaced_component_ids.clone();
        let deleted_component_ids = resolved.deleted_component_ids.clone();
        let affected_component_ids = resolved.affected_component_ids.clone();
        let commit_ref = if let Some(candidate) = resolved.commit_candidate.clone() {
            Some(self.seal(&candidate)?)
        } else {
            self.seal_staged(branch_id, boundary)?
        };
        Ok(json!({
            "schema": format!("pact.{op_kind}.v1"),
            "branch_id": branch_id,
            "commit_ref": commit_ref,
            "produced_component_ids": produced_component_ids,
            "produced_message_ids": produced_message_ids,
            "produced_message_child_ids": produced_message_child_ids,
            "replaced_component_ids": replaced_component_ids,
            "deleted_component_ids": deleted_component_ids,
            "affected_component_ids": affected_component_ids,
            "resolved": resolved,
        }))
    }

    pub fn ledger_json(&self) -> PactResult<String> {
        serde_json::to_string(self.ledger.as_ref())
            .map_err(|err| PactError::codec(format!("failed to encode PACT root store: {err}")))
    }

    fn seal_resolved_post_response(
        &mut self,
        branch_id: &str,
        resolved: ResolveOutcome,
    ) -> PactResult<Value> {
        let produced_component_ids = resolved.produced_component_ids.clone();
        let produced_message_ids = resolved.produced_message_ids.clone();
        let produced_message_child_ids = resolved.produced_message_child_ids.clone();
        let replaced_component_ids = resolved.replaced_component_ids.clone();
        let deleted_component_ids = resolved.deleted_component_ids.clone();
        let affected_component_ids = resolved.affected_component_ids.clone();
        let commit_candidate = resolved.commit_candidate.clone();
        let commit_ref = if let Some(candidate) = commit_candidate {
            Some(self.seal(&candidate)?)
        } else {
            self.seal_staged_with_cycle_policy(branch_id, BoundaryKind::PostResponse, true)?
        };
        Ok(json!({
            "schema": "pact.receive.v1",
            "branch_id": branch_id,
            "produced_component_ids": produced_component_ids,
            "produced_message_ids": produced_message_ids,
            "produced_message_child_ids": produced_message_child_ids,
            "replaced_component_ids": replaced_component_ids,
            "deleted_component_ids": deleted_component_ids,
            "affected_component_ids": affected_component_ids,
            "resolved": resolved,
            "commit_ref": commit_ref,
        }))
    }

    fn verify_staged_tool_calls(
        &self,
        branch_id: &str,
        expected_tool_call_ids: &[String],
    ) -> PactResult<()> {
        if expected_tool_call_ids.is_empty() {
            return Ok(());
        }
        let tree = self.branch_tree_value(branch_id)?;
        let mut actual = Vec::new();
        collect_tool_call_node_ids(&tree, &mut actual);
        actual.sort();
        actual.dedup();
        let missing: Vec<&String> = expected_tool_call_ids
            .iter()
            .filter(|tool_call_id| !actual.contains(tool_call_id))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(PactError::resolver(format!(
            "provider tool call(s) {} were not persisted to Pact active depth",
            missing
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    fn message_insert_ops(
        &mut self,
        branch_id: &str,
        role: &str,
        parts: Vec<Component>,
        aux: BTreeMap<String, Value>,
    ) -> PactResult<Vec<PactOperation>> {
        let root_id = self.ledger.root_id.clone();
        Ok(lower_message(&root_id, branch_id, role, parts, aux, self)?
            .into_iter()
            .collect())
    }

    fn message_parts_from_values(&mut self, parts: Vec<Value>) -> PactResult<Vec<Component>> {
        let mut components = Vec::new();
        for part in parts {
            if let Some(text) = message_part_text(&part) {
                let explicit_id = part
                    .get("id")
                    .and_then(|value| value.as_str())
                    .filter(|id| !id.is_empty())
                    .map(str::to_string);
                if text.is_empty() && explicit_id.is_none() {
                    continue;
                }
                let id = explicit_id.unwrap_or_else(|| self.next_node_id("text"));
                components.push(self.text_component(id, text, None, 0, None));
                continue;
            }
            let mut node = canonical_message_child(part)?;
            let object = node
                .as_object_mut()
                .ok_or_else(|| PactError::invalid("message child must be an object"))?;
            object
                .entry("id".to_string())
                .or_insert_with(|| json!(self.next_node_id("node")));
            components.push(component_from_authoring_value(node, "message adapter")?);
        }
        Ok(components)
    }

    fn text_component(
        &self,
        id: String,
        content: String,
        parent_id: Option<String>,
        offset: i32,
        pact_depth: Option<i32>,
    ) -> Component {
        super::message_authoring::text_component(id, content, parent_id, offset, pact_depth)
    }

    pub(super) fn next_node_id(&mut self, prefix: &str) -> String {
        self.node_counter = self.node_counter.saturating_add(1);
        if self.deterministic_authoring.is_some() {
            return format!("{prefix}_{}", self.next_authoring_component_local_id());
        }
        format!("{prefix}_{:x}_{:x}", monotonic_ns(), self.node_counter)
    }

    fn next_trace_identity(&mut self, op: &PactOperation) -> (String, u64) {
        if let Some(identity) = self.replay_trace_ids.pop_front() {
            return identity;
        }
        self.trace_counter = self.trace_counter.saturating_add(1);
        let created_at_ns = op
            .envelope()
            .host_timestamp_ns
            .unwrap_or_else(|| self.next_authoring_timestamp_ns());
        (
            trace_identity(op, created_at_ns, self.trace_counter),
            created_at_ns,
        )
    }

    pub(super) fn ingest_operations(
        &mut self,
        ops: Vec<PactOperation>,
    ) -> PactResult<IngestOutcome> {
        let mut accepted = 0usize;
        let mut deduped = 0usize;
        let mut rejected: Vec<RejectedOperation> = Vec::new();

        for op in ops {
            let op = self.with_native_node_identity(&op);
            if let Err(err) = op.validate() {
                rejected.push(RejectedOperation {
                    kind: op.kind().to_string(),
                    reason: err.to_string(),
                    payload: serde_json::to_value(&op).unwrap_or(Value::Null),
                });
                continue;
            }
            let branch = op.envelope().branch_id.clone();

            if let Some(key) = op.envelope().idempotency_key.clone() {
                let seen = self.seen_keys.entry(branch.clone()).or_default();
                if !seen.insert(key) {
                    deduped += 1;
                    continue;
                }
            }
            self.pending.entry(branch).or_default().push(op);
            accepted += 1;
        }

        Ok(IngestOutcome {
            accepted,
            deduped,
            rejected,
        })
    }

    pub fn resolve(
        &mut self,
        ops: Vec<PactOperation>,
        branch_id: &str,
        boundary: BoundaryKind,
    ) -> PactResult<ResolveOutcome> {
        let ingested = self.ingest_operations(ops)?;
        let pending = self.pending.remove(branch_id).unwrap_or_default();

        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?
            .clone();

        let had_prior_staged_state = self.staged_state.contains_key(branch_id);
        let (mut tree, mut recorded_mutations): (ComponentTree, Vec<MutationRequest>) =
            if let Some((staged_tree, staged_mutations)) = self.staged_state.remove(branch_id) {
                (staged_tree, staged_mutations)
            } else {
                (
                    project_branch_component_tree(
                        &self.ledger,
                        &branch,
                        Some(branch.head_revision),
                    )?,
                    Vec::new(),
                )
            };
        let before_ref = self.head_ref(&branch, branch.head_revision);

        let mut applied: Vec<String> = Vec::new();
        let mut rejected: Vec<RejectedOperation> = Vec::new();
        let mut traces: Vec<ResolvedOperationTrace> = Vec::new();

        let mut produced_component_ids: Vec<String> = Vec::new();
        let mut replaced_component_ids: Vec<String> = Vec::new();
        let mut deleted_component_ids: Vec<String> = Vec::new();
        let mut affected_component_ids: Vec<String> = Vec::new();

        for op in pending.iter() {
            if is_empty_message_turn_noop(op) {
                continue;
            }
            let prepared_op = self.with_native_node_identity(op);
            let trace_before_tree = tree.project_value();
            let direct_before_tree = tree.clone();

            let mutation_result = op_to_mutations(&prepared_op, &tree);
            match mutation_result {
                Ok(requests) => {
                    let mut produced: Vec<PactRef> = Vec::new();
                    let mut all_ok = true;
                    let mut direct_trace_ops: Vec<TraceOp> = Vec::new();
                    let mut direct_trace_complete = true;
                    for mut req in requests {
                        let tree_value = tree.project_value();
                        let removed_tool_call_ids = if req.kind == MutationKind::Delete {
                            tool_call_ids_for_mutation_target(&tree_value, &req.target)
                        } else {
                            Vec::new()
                        };
                        externalize_mutation_payloads(
                            &mut std::sync::Arc::make_mut(&mut self.ledger).content,
                            &mut req,
                        )?;
                        match apply_mutation_tree(&mut tree, &mut req) {
                            Ok(report) => {
                                let next_tree = tree.project_value();
                                let result = MutationApplyResult {
                                    schema: MUTATION_APPLY_RESULT_SCHEMA.to_string(),
                                    request: req.clone(),
                                    report,
                                    tree: next_tree.clone(),
                                };
                                match mutation_result_trace_ops(
                                    &result.request,
                                    &result.report,
                                    &direct_before_tree,
                                    &tree,
                                    branch.derived_depth(),
                                ) {
                                    Ok(Some(ops)) => direct_trace_ops.extend(ops),
                                    Ok(None) => direct_trace_complete = false,
                                    Err(_) => direct_trace_complete = false,
                                }

                                for inserted_id in &result.report.inserted_ids {
                                    if let Some(node) = find_node_value(&next_tree, inserted_id) {
                                        collect_subtree_ids(node, &mut produced_component_ids);
                                        collect_subtree_ids(node, &mut affected_component_ids);
                                    } else {
                                        produced_component_ids.push(inserted_id.clone());
                                        affected_component_ids.push(inserted_id.clone());
                                    }
                                }
                                replaced_component_ids
                                    .extend(result.report.replaced_ids.iter().cloned());
                                deleted_component_ids
                                    .extend(result.report.removed_ids.iter().cloned());
                                affected_component_ids
                                    .extend(result.report.replaced_ids.iter().cloned());
                                affected_component_ids
                                    .extend(result.report.removed_ids.iter().cloned());
                                affected_component_ids.extend(
                                    result
                                        .report
                                        .shifted_offsets
                                        .iter()
                                        .map(|shift| shift.component_id.clone()),
                                );
                                affected_component_ids.extend(
                                    result
                                        .report
                                        .shifted_depths
                                        .iter()
                                        .map(|shift| shift.component_id.clone()),
                                );
                                affected_component_ids.extend(
                                    result
                                        .report
                                        .parent_changes
                                        .iter()
                                        .map(|change| change.component_id.clone()),
                                );
                                affected_component_ids.extend(
                                    result
                                        .report
                                        .content_ref_changes
                                        .iter()
                                        .map(|change| change.component_id.clone()),
                                );
                                observe_tool_nodes_after_mutation(
                                    &next_tree,
                                    &result,
                                    &removed_tool_call_ids,
                                    self.pending_external
                                        .entry(branch_id.to_string())
                                        .or_default(),
                                    &mut self.requirement_counter,
                                    branch_id,
                                    boundary,
                                );
                                produced.extend(operation_report_refs(
                                    &result.report,
                                    &self.ledger.root_id,
                                    branch_id,
                                ));
                                produced.extend(operation_request_content_refs(
                                    &result.request,
                                    &self.ledger.root_id,
                                    branch_id,
                                ));
                                recorded_mutations.push(result.request);
                                let lifecycle_synced =
                                    sync_tool_pair_lifecycle(&mut tree, &mut recorded_mutations)?;
                                if !lifecycle_synced.is_empty() {
                                    append_lifecycle_sync_trace_ops(
                                        &tree,
                                        &lifecycle_synced,
                                        &mut direct_trace_ops,
                                    )?;
                                    produced_component_ids.extend(lifecycle_synced.iter().cloned());
                                    affected_component_ids.extend(lifecycle_synced.iter().cloned());
                                    produced.extend(lifecycle_synced.into_iter().map(
                                        |component_id| PactRef {
                                            root_id: self.ledger.root_id.clone(),
                                            branch_id: branch_id.to_string(),
                                            revision: None,
                                            transaction_id: None,
                                            component_id: Some(component_id),
                                            content_ref: None,
                                        },
                                    ));
                                }
                            }
                            Err(err) => {
                                rejected.push(RejectedOperation {
                                    kind: prepared_op.kind().to_string(),
                                    reason: err.to_string(),
                                    payload: serde_json::to_value(&prepared_op)
                                        .unwrap_or(Value::Null),
                                });
                                all_ok = false;
                                break;
                            }
                        }
                    }
                    if all_ok {
                        let idem = prepared_op.envelope().idempotency_key.clone();
                        let (trace_id, created_at_ns) = self.next_trace_identity(&prepared_op);
                        let after_tree = tree.project_value();
                        if !direct_trace_complete {
                            return Err(PactError::resolver(format!(
                                "direct trace lowering incomplete for accepted {} mutation",
                                prepared_op.kind()
                            )));
                        }
                        if !trace_ops_replay_to(&trace_before_tree, &after_tree, &direct_trace_ops)
                        {
                            return Err(PactError::resolver(format!(
                                "direct trace lowering for accepted {} mutation did not replay to the post-mutation tree",
                                prepared_op.kind()
                            )));
                        }
                        let trace_ops = direct_trace_ops;
                        let trace = ResolvedOperationTrace {
                            trace_id,
                            op_kind: prepared_op.kind().to_string(),
                            idempotency_key: idem,
                            created_at_ns,
                            ops: trace_ops,
                            produced_refs: produced,
                            aux: operation_trace_aux(&prepared_op),
                        };
                        applied.push(prepared_op.kind().to_string());
                        self.staged_trace_records
                            .entry(branch_id.to_string())
                            .or_default()
                            .push(StagedTraceRecord {
                                trace: trace.clone(),
                                boundary,
                                op: prepared_op.clone(),
                            });
                        traces.push(trace);
                    }
                }
                Err(err) => {
                    rejected.push(RejectedOperation {
                        kind: prepared_op.kind().to_string(),
                        reason: err.to_string(),
                        payload: serde_json::to_value(&prepared_op).unwrap_or(Value::Null),
                    });
                }
            }
        }

        if !applied.is_empty() {
            let pruned = prune_hollow_message_turns(&mut tree, &mut recorded_mutations)?;
            deleted_component_ids.extend(pruned.iter().cloned());
            affected_component_ids.extend(pruned);
        }

        produced_component_ids.retain(|id| !deleted_component_ids.contains(id));

        let tree_value = tree.project_value();
        let (produced_message_ids, produced_message_child_ids) =
            classify_produced_message_ids(&tree_value, &produced_component_ids);
        dedup_node_ids(&mut produced_component_ids);
        dedup_node_ids(&mut replaced_component_ids);
        dedup_node_ids(&mut deleted_component_ids);
        dedup_node_ids(&mut affected_component_ids);

        let next_rev = branch.head_revision + 1;
        let after_ref = PactRef {
            root_id: self.ledger.root_id.clone(),
            branch_id: branch_id.to_string(),
            revision: Some(next_rev),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };

        let dirty_projections = if !applied.is_empty() {
            vec![
                "view".to_string(),
                "chat".to_string(),
                "inspection".to_string(),
                "provider".to_string(),
            ]
        } else {
            vec![]
        };

        let policy = BoundaryPolicy::for_kind(boundary);
        let branch_pending = self
            .pending_external
            .entry(branch_id.to_string())
            .or_default();

        let policy_outcome = branch_pending.evaluate(&policy);
        let outcome_state =
            if applied.is_empty() && matches!(policy_outcome, BoundaryOutcome::Committed) {
                BoundaryOutcome::ProjectedReadonly
            } else {
                policy_outcome
            };

        let can_commit = matches!(
            outcome_state,
            BoundaryOutcome::Committed | BoundaryOutcome::Deferred
        ) && !applied.is_empty();

        let commit_candidate = if can_commit {
            Some(ResolvedBoundary {
                schema: RESOLVED_BOUNDARY_SCHEMA.to_string(),
                branch_id: branch_id.to_string(),
                boundary,
                before_ref: before_ref.clone(),
                after_ref: after_ref.clone(),
                applied: applied.clone(),
                traces: traces.clone(),
                dirty_projections: dirty_projections.clone(),
                aux: BTreeMap::new(),
            })
        } else {
            None
        };

        if commit_candidate.is_some() || had_prior_staged_state {
            self.staged_state
                .insert(branch_id.to_string(), (tree, recorded_mutations));
        }
        if !traces.is_empty() {
            self.staged_traces
                .entry(branch_id.to_string())
                .or_default()
                .extend(traces.clone());
        }

        let pending_external_mutations: Vec<ExternalMutationRequirement> =
            branch_pending.iter().cloned().collect();

        let mut all_rejected = ingested.rejected;
        all_rejected.extend(rejected);

        Ok(ResolveOutcome {
            schema: RESOLVE_OUTCOME_SCHEMA.to_string(),
            branch_id: branch_id.to_string(),
            boundary,
            accepted: ingested.accepted,
            deduped: ingested.deduped,
            applied,
            rejected: all_rejected,
            produced_component_ids,
            produced_message_ids,
            produced_message_child_ids,
            replaced_component_ids,
            deleted_component_ids,
            affected_component_ids,
            pending_external_mutations,
            dirty_projections,
            traces,
            outcome: outcome_state,
            commit_candidate,
        })
    }

    pub fn pending_external(&self, branch_id: &str) -> Vec<ExternalMutationRequirement> {
        self.pending_external
            .get(branch_id)
            .map(|p| p.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn record_external_failure(
        &mut self,
        branch_id: &str,
        idempotency_key: &str,
        reason: String,
    ) {
        self.pending_external
            .entry(branch_id.to_string())
            .or_default()
            .record_failure(idempotency_key, reason);
    }

    pub fn preview(&self, branch_id: &str) -> PactResult<Value> {
        if let Some((tree, _recorded_mutations)) = self.staged_state.get(branch_id) {
            let mut hydrated = tree.project_value();
            hydrate_payload_refs(&mut hydrated, &self.ledger.content)?;
            return Ok(hydrated);
        }
        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        let mut hydrated =
            project_branch_component_tree(&self.ledger, branch, Some(branch.head_revision))?
                .project_value();
        hydrate_payload_refs(&mut hydrated, &self.ledger.content)?;
        Ok(hydrated)
    }

    pub fn discard_staged(&mut self, branch_id: &str) -> bool {
        let removed_pending = self.pending.remove(branch_id).is_some();
        let removed_state = self.staged_state.remove(branch_id).is_some();
        let removed_trace = self.staged_traces.remove(branch_id).is_some();
        let removed_trace_records = self.staged_trace_records.remove(branch_id).is_some();
        let removed_extensions = self.staged_extensions.remove(branch_id).is_some();
        self.pending_external.remove(branch_id);
        self.replay_trace_ids.clear();
        removed_pending
            || removed_state
            || removed_trace
            || removed_trace_records
            || removed_extensions
    }

    pub fn has_staged(&self, branch_id: &str) -> bool {
        self.pending
            .get(branch_id)
            .is_some_and(|pending| !pending.is_empty())
            || self.staged_state.contains_key(branch_id)
            || self
                .staged_trace_records
                .get(branch_id)
                .is_some_and(|records| !records.is_empty())
            || self
                .staged_extensions
                .get(branch_id)
                .is_some_and(|records| !records.is_empty())
    }

    pub fn stage_extensions(
        &mut self,
        branch_id: &str,
        records: Vec<crate::PactExtensionRecord>,
    ) -> PactResult<()> {
        if !self.ledger.branches.contains_key(branch_id) {
            return Err(PactError::not_found(format!(
                "branch not found: {branch_id}"
            )));
        }
        crate::extension::validate_extension_batch(&records)?;
        let mut existing_ids = self
            .ledger
            .event_index
            .transaction_traces
            .values()
            .filter(|trace| trace.branch_id == branch_id)
            .map(crate::extension::records_from_trace)
            .collect::<PactResult<Vec<_>>>()?
            .into_iter()
            .flatten()
            .map(|record| record.record_id)
            .collect::<BTreeSet<_>>();
        existing_ids.extend(
            self.staged_extensions
                .get(branch_id)
                .into_iter()
                .flatten()
                .map(|record| record.record_id.clone()),
        );
        if let Some(duplicate) = records
            .iter()
            .find(|record| existing_ids.contains(&record.record_id))
        {
            return Err(PactError::invalid(format!(
                "duplicate Pact extension record id: {}",
                duplicate.record_id
            )));
        }
        self.staged_extensions
            .entry(branch_id.to_string())
            .or_default()
            .extend(records);
        Ok(())
    }

    pub(crate) fn has_resolved_staged_state(&self, branch_id: &str) -> bool {
        self.staged_state.contains_key(branch_id)
    }

    pub fn staged_traces(&self, branch_id: &str) -> Vec<ResolvedOperationTrace> {
        self.staged_trace_records
            .get(branch_id)
            .map(|records| records.iter().map(|record| record.trace.clone()).collect())
            .unwrap_or_default()
    }

    pub fn discard_staged_trace(&mut self, branch_id: &str, trace_id: &str) -> PactResult<bool> {
        Ok(self
            .discard_staged_traces(branch_id, StagedTraceTarget::TraceId(trace_id.to_string()))?
            > 0)
    }

    pub fn discard_staged_traces(
        &mut self,
        branch_id: &str,
        target: StagedTraceTarget,
    ) -> PactResult<usize> {
        if let StagedTraceTarget::Selector(selector) = &target {
            return Err(PactError::invalid(format!(
                "selector-based staged trace discard is not supported yet: {selector}"
            )));
        }
        let Some(records) = self.staged_trace_records.remove(branch_id) else {
            return Ok(0);
        };
        let original_len = records.len();
        let remaining: Vec<StagedTraceRecord> = records
            .into_iter()
            .filter(|record| !staged_trace_record_matches(record, &target))
            .collect();
        let removed_count = original_len - remaining.len();
        if removed_count == 0 {
            self.staged_trace_records
                .insert(branch_id.to_string(), remaining);
            return Ok(0);
        }

        self.pending.remove(branch_id);
        self.staged_state.remove(branch_id);
        self.staged_traces.remove(branch_id);
        self.pending_external.remove(branch_id);
        self.replay_trace_ids.clear();

        for record in remaining {
            self.replay_trace_ids
                .push_back((record.trace.trace_id.clone(), record.trace.created_at_ns));
            self.pending
                .entry(branch_id.to_string())
                .or_default()
                .push(record.op);
            let outcome = self.resolve(Vec::new(), branch_id, record.boundary)?;
            if !outcome.rejected.is_empty() {
                self.replay_trace_ids.clear();
                return Err(PactError::resolver(format!(
                    "failed to replay staged operation after discard: {}",
                    outcome
                        .rejected
                        .iter()
                        .map(|item| item.reason.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                )));
            }
        }
        self.replay_trace_ids.clear();
        Ok(removed_count)
    }

    pub fn seal_staged(
        &mut self,
        branch_id: &str,
        boundary: BoundaryKind,
    ) -> PactResult<Option<CommitRef>> {
        self.seal_staged_with_cycle_policy(branch_id, boundary, boundary_closes_cycle(boundary))
    }

    fn seal_staged_with_cycle_policy(
        &mut self,
        branch_id: &str,
        boundary: BoundaryKind,
        closes_cycle: bool,
    ) -> PactResult<Option<CommitRef>> {
        self.seal_staged_with_cycle_policy_report(branch_id, boundary, closes_cycle)
            .map(|(commit_ref, _)| commit_ref)
    }

    fn seal_staged_with_cycle_policy_report(
        &mut self,
        branch_id: &str,
        boundary: BoundaryKind,
        closes_cycle: bool,
    ) -> PactResult<(Option<CommitRef>, ResolveOutcome)> {
        let outcome = self.resolve(Vec::new(), branch_id, boundary)?;
        let lifecycle_changed = if closes_cycle {
            self.apply_lifecycle_boundary(branch_id)?
        } else {
            false
        };
        let has_extensions = self
            .staged_extensions
            .get(branch_id)
            .is_some_and(|records| !records.is_empty());
        if has_extensions && !self.staged_state.contains_key(branch_id) {
            let branch =
                self.ledger.branches.get(branch_id).ok_or_else(|| {
                    PactError::not_found(format!("branch not found: {branch_id}"))
                })?;
            let tree =
                project_branch_component_tree(&self.ledger, branch, Some(branch.head_revision))?;
            self.staged_state
                .insert(branch_id.to_string(), (tree, Vec::new()));
        }

        if !self.staged_state.contains_key(branch_id) && !closes_cycle && !has_extensions {
            return Ok((None, outcome));
        }

        if let Some(candidate) = outcome.commit_candidate.clone() {
            let commit_ref = self.commit_resolved_boundary(&candidate, closes_cycle)?;
            return Ok((Some(commit_ref), outcome));
        }

        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?
            .clone();
        let before_ref = self.head_ref(&branch, branch.head_revision);
        let after_ref = PactRef {
            root_id: self.ledger.root_id.clone(),
            branch_id: branch_id.to_string(),
            revision: Some(branch.head_revision + 1),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        };
        let synthetic_traces = self.staged_traces(branch_id);
        let synthetic = ResolvedBoundary {
            schema: RESOLVED_BOUNDARY_SCHEMA.to_string(),
            branch_id: branch_id.to_string(),
            boundary,
            before_ref,
            after_ref,
            applied: if lifecycle_changed {
                vec!["lifecycle".to_string()]
            } else {
                vec!["seal_staged".to_string()]
            },
            traces: synthetic_traces,
            dirty_projections: vec![
                "view".to_string(),
                "chat".to_string(),
                "inspection".to_string(),
                "provider".to_string(),
            ],
            aux: BTreeMap::new(),
        };
        let commit_ref = self.commit_resolved_boundary(&synthetic, closes_cycle)?;
        Ok((Some(commit_ref), outcome))
    }

    pub fn seal(&mut self, boundary: &ResolvedBoundary) -> PactResult<CommitRef> {
        let closes_cycle = boundary_closes_cycle(boundary.boundary);
        if closes_cycle {
            self.apply_lifecycle_boundary(&boundary.branch_id)?;
        }
        self.commit_resolved_boundary(boundary, closes_cycle)
    }

    fn commit_resolved_boundary(
        &mut self,
        boundary: &ResolvedBoundary,
        closes_cycle: bool,
    ) -> PactResult<CommitRef> {
        let branch_id = &boundary.branch_id;
        let (tree, recorded_mutations) = self.staged_state.remove(branch_id).ok_or_else(|| {
            PactError::resolver(format!(
                "no resolved staged state for branch {branch_id}; call resolve() first"
            ))
        })?;
        let tree_component = tree.root().clone();
        let mut tree = tree.project_value();

        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?
            .clone();
        let next_rev = branch.head_revision + 1;

        let mut first_seen = branch.current.tree.first_seen.clone();
        externalize_large_content_in_value(
            &mut tree,
            &mut std::sync::Arc::make_mut(&mut self.ledger).content,
            self.blob_inline_limit,
        );
        let probe =
            build_projection_from_component(&tree_component, &first_seen, self.blob_inline_limit)?;
        for component_id in probe.component_ids() {
            first_seen.entry(component_id.clone()).or_insert(next_rev);
        }
        let projection =
            build_projection_from_component(&tree_component, &first_seen, self.blob_inline_limit)?;
        let view = PactView::from_projection_with_lifecycle_seen(
            projection,
            branch_id.to_string(),
            next_rev,
            branch.current.lifecycle_seen.clone(),
            branch.lifecycle_tick + 1,
        );

        let before_tree =
            project_branch_component_tree(&self.ledger, &branch, Some(branch.head_revision))?
                .project_value();
        let birth_base_content_ref = if branch.head_revision == 0
            && next_rev == 1
            && branch.base_ref.content_ref.is_none()
            && branch.fork_ref.is_none()
        {
            let base_json = serde_json::to_string(&before_tree).map_err(|err| {
                PactError::codec(format!("failed to encode branch birth base: {err}"))
            })?;
            Some(
                std::sync::Arc::make_mut(&mut self.ledger)
                    .content
                    .put_inline("pact.branch.base", &base_json, self.blob_inline_limit)
                    .id,
            )
        } else {
            None
        };

        let staged_ops = self
            .staged_traces
            .get(branch_id)
            .map(|traces| {
                traces
                    .iter()
                    .flat_map(|trace| trace.ops.iter().cloned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let direct_trace = (!staged_ops.is_empty())
            .then_some(TransactionTrace {
                from_rev: branch.head_revision,
                to_rev: next_rev,
                ops: staged_ops,
            })
            .or_else(|| {
                sealed_transaction_trace_from_boundary(boundary, branch.head_revision, next_rev)
            });
        let canonical_trace = direct_trace
            .filter(|trace| {
                trace_ops_preserve_preexisting_removals(&before_tree, &tree, &trace.ops)
            })
            .or_else(|| {
                let trace =
                    TransactionTrace::diff(branch.head_revision, next_rev, &before_tree, &tree);
                (!trace.is_empty()).then_some(trace)
            });

        if let Some(ref path) = self.file_path {
            match std::fs::metadata(path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let file_bytes = self.encode_new_file_baseline()?;
                    publish_bytes(path, &file_bytes, PublishMode::CreateNew)?;
                    self.persisted_content_ids =
                        self.ledger.content.records.keys().cloned().collect();
                }
                Err(error) => return Err(PactError::Io(error)),
            }
        }

        let branch_mut = std::sync::Arc::make_mut(&mut self.ledger)
            .branches
            .get_mut(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
        if let Some(trace) = canonical_trace.clone() {
            branch_mut.transaction_traces.push(trace);
        }
        if let Some(content_ref) = birth_base_content_ref {
            branch_mut.base_ref.revision = Some(0);
            branch_mut.base_ref.content_ref = Some(content_ref);
        }
        branch_mut.head_revision = next_rev;
        if closes_cycle {
            branch_mut.lifecycle_tick = branch.lifecycle_tick + 1;
        }
        branch_mut.current = view;

        let public_trace = self.seal_trace_record(
            boundary,
            next_rev,
            recorded_mutations.is_empty(),
            canonical_trace.clone(),
        )?;
        if let Some(path) = self.file_path.clone() {
            let committed_branch =
                self.ledger.branches.get(branch_id).ok_or_else(|| {
                    PactError::not_found(format!("branch not found: {branch_id}"))
                })?;
            let first_seen_additions = committed_branch
                .current
                .tree
                .first_seen
                .iter()
                .filter(|(component_id, _)| {
                    !branch.current.tree.first_seen.contains_key(*component_id)
                })
                .map(|(component_id, revision)| (component_id.clone(), *revision))
                .collect();
            let lifecycle_seen_additions = committed_branch
                .current
                .lifecycle_seen
                .iter()
                .filter(|(component_id, _)| {
                    !branch.current.lifecycle_seen.contains_key(*component_id)
                })
                .map(|(component_id, tick)| (component_id.clone(), *tick))
                .collect();
            let lifecycle_seen_removals = branch
                .current
                .lifecycle_seen
                .keys()
                .filter(|component_id| {
                    !committed_branch
                        .current
                        .tree
                        .index
                        .by_id
                        .contains_key(*component_id)
                })
                .cloned()
                .collect();
            let new_content = self
                .ledger
                .content
                .records
                .iter()
                .filter(|(content_id, _)| !self.persisted_content_ids.contains(*content_id))
                .map(|(_, content)| content.clone())
                .collect::<Vec<_>>();
            let new_content_ids = new_content
                .iter()
                .map(|content| content.id.clone())
                .collect::<Vec<_>>();
            let delta = crate::pact_file::transaction::LedgerDelta {
                lifecycle_tick: committed_branch.lifecycle_tick,
                lifecycle_birth_tick: branch.lifecycle_tick + 1,
                base_ref: committed_branch.base_ref.clone(),
                first_seen_additions,
                lifecycle_seen_additions,
                lifecycle_seen_removals,
                record_canonical_trace: canonical_trace.is_some(),
                record_public_trace: public_trace.is_some(),
                derived_depth: branch.derived_depth(),
            };
            let trace = canonical_trace.unwrap_or(TransactionTrace {
                from_rev: branch.head_revision,
                to_rev: next_rev,
                ops: Vec::new(),
            });
            if let Some(log) = self.file_commit_log.as_mut() {
                log.push(super::RecordedFileCommit {
                    trace: trace.clone(),
                    public_trace: public_trace.clone(),
                    delta: delta.clone(),
                    new_content: new_content.clone(),
                    head_metadata: crate::pact_file::index::HeadMetadata::from_ledger(&self.ledger),
                });
            }
            crate::pact_file::commit_ledger(
                &path,
                crate::pact_file::LedgerCommit {
                    ledger: &self.ledger,
                    branch_id,
                    boundary: boundary.boundary,
                    trace,
                    public_trace,
                    delta,
                    new_content,
                    maintain: self.automatic_compaction,
                },
            )?;
            self.persisted_content_ids.extend(new_content_ids);
        }
        self.staged_trace_records.remove(branch_id);
        self.replay_trace_ids.clear();

        Ok(CommitRef {
            branch_id: branch_id.clone(),
            revision: next_rev,
            boundary: boundary.boundary,
            before_ref: boundary.before_ref.clone(),
            after_ref: PactRef {
                revision: Some(next_rev),
                ..boundary.after_ref.clone()
            },
        })
    }

    fn seal_trace_record(
        &mut self,
        boundary: &ResolvedBoundary,
        next_rev: u64,
        mutationless_commit: bool,
        canonical_replay_trace: Option<TransactionTrace>,
    ) -> PactResult<Option<PactTransactionTrace>> {
        let branch_id = &boundary.branch_id;
        let mut traces = self.staged_traces.remove(branch_id).unwrap_or_default();
        let extensions = self.staged_extensions.remove(branch_id).unwrap_or_default();
        if traces.is_empty() {
            traces = boundary.traces.clone();
        }
        if traces.is_empty() && mutationless_commit && extensions.is_empty() {
            return Ok(None);
        }

        let replay_from_rev = boundary
            .before_ref
            .revision
            .unwrap_or_else(|| next_rev.saturating_sub(1));

        let replay_ops = traces
            .iter()
            .flat_map(|trace| trace.ops.iter().cloned())
            .collect::<Vec<_>>();
        let replay_trace = canonical_replay_trace
            .or_else(|| {
                (!replay_ops.is_empty()).then_some(TransactionTrace {
                    from_rev: replay_from_rev,
                    to_rev: next_rev,
                    ops: replay_ops,
                })
            })
            .or_else(|| {
                sealed_transaction_trace_from_boundary(boundary, replay_from_rev, next_rev)
            });
        let trace = build_sealed_public_trace(SealedTraceRequest {
            root_id: &self.ledger.root_id,
            branch_id,
            next_rev,
            boundary: boundary.boundary,
            before_ref: &boundary.before_ref,
            after_ref: &boundary.after_ref,
            applied: boundary.applied.clone(),
            events: &traces,
            extensions,
            replay_trace,
            aux: boundary.aux.clone(),
        })?;
        let transaction_id = trace.transaction_id.clone();
        let branch_ids = std::sync::Arc::make_mut(&mut self.ledger)
            .event_index
            .by_transaction
            .entry(transaction_id.clone())
            .or_default();
        if !branch_ids.contains(branch_id) {
            branch_ids.push(branch_id.clone());
        }
        std::sync::Arc::make_mut(&mut self.ledger)
            .event_index
            .transaction_traces
            .insert(transaction_id, trace.clone());
        Ok(Some(trace))
    }

    fn head_ref(&self, branch: &PactBranch, revision: u64) -> PactRef {
        PactRef {
            root_id: self.ledger.root_id.clone(),
            branch_id: branch.branch_id.clone(),
            revision: Some(revision),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        }
    }

    fn with_native_node_identity(&mut self, op: &PactOperation) -> PactOperation {
        let mut prepared = op.clone();
        match &mut prepared {
            PactOperation::Insert {
                envelope,
                component,
                ..
            }
            | PactOperation::Append {
                envelope,
                component,
                ..
            }
            | PactOperation::Upsert {
                envelope,
                component,
                ..
            } => {
                self.ensure_operation_component_id(component, None);
                let ts = self.ensure_operation_timestamp(envelope);
                stamp_component_created_at(component, ts);
            }
            PactOperation::Replace {
                envelope,
                target_component_id,
                component,
                ..
            } => {
                self.ensure_operation_component_id(component, target_component_id.as_deref());
                let ts = self.ensure_operation_timestamp(envelope);
                stamp_component_created_at(component, ts);
            }
            _ => {}
        }
        prepared
    }

    fn ensure_operation_timestamp(&mut self, envelope: &mut PactOperationEnvelope) -> u64 {
        if let Some(timestamp) = envelope.host_timestamp_ns {
            return timestamp;
        }
        let timestamp = self.next_authoring_timestamp_ns();
        envelope.host_timestamp_ns = Some(timestamp);
        timestamp
    }

    fn stamp_typed_operation(&mut self, op: &mut PactOperation) {
        match op {
            PactOperation::Insert { envelope, .. }
            | PactOperation::Replace { envelope, .. }
            | PactOperation::Append { envelope, .. }
            | PactOperation::Upsert { envelope, .. } => {
                self.ensure_operation_timestamp(envelope);
            }
            _ => {}
        }
    }

    fn ensure_operation_component_id(
        &mut self,
        component: &mut Component,
        preferred_id: Option<&str>,
    ) {
        if !component.id.is_empty() {
            return;
        }
        component.id = preferred_id
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.next_node_id("node"));
    }

    fn ensure_operation_node_id(&mut self, node: &mut Value, preferred_id: Option<&str>) {
        let Some(object) = node.as_object_mut() else {
            return;
        };
        let has_id = object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty());
        if has_id {
            return;
        }
        let id = preferred_id
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.next_node_id("node"));
        object.insert("id".to_string(), Value::String(id));
    }

    pub(crate) fn operation_component_from_value(
        &mut self,
        mut value: Value,
        preferred_id: Option<&str>,
        operation: &str,
    ) -> PactResult<Component> {
        self.ensure_operation_node_id(&mut value, preferred_id);
        component_from_authoring_value(value, operation)
    }

    fn apply_lifecycle_boundary(&mut self, branch_id: &str) -> PactResult<bool> {
        let branch = self
            .ledger
            .branches
            .get(branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?
            .clone();
        let next_rev = branch.head_revision + 1;
        let (mut tree, mut recorded_mutations): (ComponentTree, Vec<MutationRequest>) =
            if let Some((staged_tree, staged_mutations)) = self.staged_state.remove(branch_id) {
                (staged_tree, staged_mutations)
            } else {
                (
                    project_branch_component_tree(
                        &self.ledger,
                        &branch,
                        Some(branch.head_revision),
                    )?,
                    Vec::new(),
                )
            };

        let component_ids = tree.component_ids().map(str::to_string).collect::<Vec<_>>();

        let mut first_seen = branch.current.tree.first_seen.clone();
        for component_id in &component_ids {
            first_seen.entry(component_id.clone()).or_insert(next_rev);
        }

        let mut lifecycle_seen = branch.current.lifecycle_seen.clone();
        for component_id in &component_ids {
            lifecycle_seen
                .entry(component_id.clone())
                .or_insert(branch.lifecycle_tick + 1);
        }

        let expired =
            lifecycle_expired_top_components(&tree, &lifecycle_seen, branch.lifecycle_tick + 1);
        let mut changed = false;
        for component_id in expired {
            let cadence_clone =
                lifecycle_cadence_clone_tree(&tree, &component_id, next_rev, || {
                    self.next_authoring_timestamp_ns()
                })?;
            let delete_req = MutationRequest {
                schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                request_id: Some(format!("lifecycle_expire_{component_id}_{next_rev}")),
                transaction_id: None,
                kind: MutationKind::Delete,
                target: MutationTarget {
                    component_id: Some(component_id.clone()),
                    ..MutationTarget::default()
                },
                node: None,
                mode: None,
                content: None,
                content_ref: None,
                fields: BTreeMap::new(),
                attrs: BTreeMap::new(),
            };
            let mut delete_req = delete_req;
            apply_mutation_tree(&mut tree, &mut delete_req)?;
            recorded_mutations.push(delete_req);
            changed = true;

            if let Some((parent_id, offset, node)) = cadence_clone {
                let insert_req = MutationRequest {
                    schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                    request_id: Some(format!("lifecycle_resurface_{}_{}", node.id, next_rev)),
                    transaction_id: None,
                    kind: MutationKind::Insert,
                    target: MutationTarget {
                        parent_id: Some(parent_id),
                        offset: Some(offset),
                        ..MutationTarget::default()
                    },
                    node: Some(node),
                    mode: None,
                    content: None,
                    content_ref: None,
                    fields: BTreeMap::new(),
                    attrs: BTreeMap::new(),
                };
                let mut insert_req = insert_req;
                apply_mutation_tree(&mut tree, &mut insert_req)?;
                recorded_mutations.push(insert_req);
            }
        }

        if changed {
            prune_hollow_message_turns(&mut tree, &mut recorded_mutations)?;
        }

        self.staged_state
            .insert(branch_id.to_string(), (tree, recorded_mutations));
        Ok(changed)
    }
}

fn file_attachment_identity(
    ledger: &ContextTreeLedger,
    branch_id: &str,
) -> PactResult<FileAttachmentIdentity> {
    let branch = ledger
        .branches
        .get(branch_id)
        .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))?;
    let tree = project_branch_component_tree(ledger, branch, Some(branch.head_revision))?;
    let semantic = json!({
        "root_id": ledger.root_id,
        "branch_id": branch_id,
        "head_revision": branch.head_revision,
        "parent_branch_id": branch.parent_branch_id,
        "fork_ref": branch.fork_ref,
        "lifecycle_tick": branch.lifecycle_tick,
        "message_order": branch.current.message_order,
        "transaction_traces": branch.transaction_traces,
        "tree": storage_normalized_projection(tree.project_value()),
    });
    let bytes = serde_json::to_vec(&canonical_json(semantic))
        .map_err(|error| PactError::codec(format!("failed to encode file identity: {error}")))?;
    Ok(FileAttachmentIdentity {
        root_id: ledger.root_id.clone(),
        branch_id: branch_id.to_string(),
        head_revision: branch.head_revision,
        semantic_digest: *blake3::hash(&bytes).as_bytes(),
    })
}

fn storage_normalized_projection(mut value: Value) -> Value {
    fn normalize(value: &mut Value) {
        let Value::Object(node) = value else {
            return;
        };
        if let Some(Value::Array(children)) = node.get_mut("children") {
            children.iter_mut().for_each(normalize);
        }
        if node.contains_key("parent_id") {
            node.insert("parent_id".to_string(), Value::Null);
        }

        let Some(Value::Object(aux)) = node.get("aux") else {
            return;
        };
        let mut redundant = Vec::new();
        for (aux_key, node_key) in [
            ("id", "id"),
            ("componentType", "componentType"),
            ("node_type", "componentType"),
            ("type", "type"),
            ("role", "role"),
            ("parent_id", "parent_id"),
            ("offset", "offset"),
            ("pact_depth", "pact_depth"),
            ("depth", "pact_depth"),
            ("created_at_ns", "created_at_ns"),
            ("creation_index", "creation_index"),
            ("ttl", "ttl"),
            ("cad", "cad"),
            ("cadence", "cad"),
            ("key", "key"),
            ("tags", "tags"),
            ("tool_call_id", "tool_call_id"),
            ("tool_name", "tool_name"),
            ("parameters", "parameters"),
            ("parameters_ref", "parameters_ref"),
            ("success", "success"),
            ("result", "result"),
            ("result_ref", "result_ref"),
            ("error_message", "error_message"),
            ("error_ref", "error_ref"),
        ] {
            if aux.get(aux_key).is_some_and(|value| {
                node.get(node_key)
                    .is_some_and(|canonical| canonical == value)
            }) {
                redundant.push(aux_key);
            }
        }
        if let Some(Value::Array(tags)) = node.get("tags")
            && tags.len() == 1
            && aux.get("tag") == tags.first()
        {
            redundant.push("tag");
        }
        redundant.push("created_at_iso");

        let Some(Value::Object(aux)) = node.get_mut("aux") else {
            return;
        };
        for key in redundant {
            aux.remove(key);
        }
        if aux.is_empty() {
            node.remove("aux");
        }
    }

    normalize(&mut value);
    value
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn boundary_closes_cycle(boundary: BoundaryKind) -> bool {
    matches!(
        boundary,
        BoundaryKind::Response | BoundaryKind::PostResponse
    )
}

fn host_operation_envelope(branch_id: &str) -> PactOperationEnvelope {
    PactOperationEnvelope {
        branch_id: branch_id.to_string(),
        source: SourceActor::Host,
        source_id: None,
        host_timestamp_ns: None,
        idempotency_key: None,
        aux: BTreeMap::new(),
    }
}

fn component_from_authoring_value(value: Value, operation: &str) -> PactResult<Component> {
    Component::from_component_value(&value)
        .map_err(|err| PactError::invalid(format!("{operation}: invalid component payload: {err}")))
}

fn message_part_text(part: &Value) -> Option<String> {
    if let Some(text) = part.as_str() {
        return Some(text.to_string());
    }
    let object = part.as_object()?;
    let node_type = object
        .get("componentType")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str);
    if matches!(node_type, Some("text_content") | Some("text")) {
        return object
            .get("content")
            .or_else(|| object.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if matches!(node_type, Some("reasoning_content") | Some("reasoning")) {
        return None;
    }
    if node_type.is_none()
        && !object.contains_key("tool_call_id")
        && !object.contains_key("parameters")
        && !object.contains_key("result")
        && !object.contains_key("success")
    {
        return object
            .get("content")
            .or_else(|| object.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    None
}

fn canonical_message_child(mut part: Value) -> PactResult<Value> {
    if !part.is_object() {
        return Err(PactError::invalid(
            "message child must be text or an object node",
        ));
    }
    if let Some(object) = part.as_object_mut()
        && !object.contains_key("componentType")
        && !object.contains_key("type")
    {
        let node_type = infer_message_child_kind(object);
        object.insert("componentType".to_string(), json!(node_type.clone()));
        object.insert("type".to_string(), json!(node_type));
    }
    Ok(part)
}

fn infer_message_child_kind(object: &Map<String, Value>) -> String {
    if object.contains_key("tool_call_id")
        && (object.contains_key("success")
            || object.contains_key("result")
            || object.contains_key("result_ref")
            || object.contains_key("error")
            || object.contains_key("error_ref"))
    {
        return "tool_result".to_string();
    }
    if object.contains_key("tool_call_id")
        && (object.contains_key("tool_name")
            || object.contains_key("name")
            || object.contains_key("parameters")
            || object.contains_key("parameters_ref"))
    {
        return "tool_call".to_string();
    }
    if object.contains_key("visibility")
        || object.contains_key("encrypted_content")
        || object.contains_key("data")
    {
        return "reasoning_content".to_string();
    }
    if object.contains_key("content") || object.contains_key("content_ref") {
        return "text_content".to_string();
    }
    "pact_node".to_string()
}

fn tool_call_ids_from_components(parts: &[Component]) -> Vec<String> {
    let mut ids = Vec::new();
    for part in parts {
        collect_tool_call_ids_from_component(part, &mut ids);
    }
    ids.sort();
    ids.dedup();
    ids
}

fn collect_tool_call_ids_from_component(component: &Component, out: &mut Vec<String>) {
    if component.component_type() == "tool_call"
        && let Some(tool_call_id) = component.tool_call_id()
    {
        out.push(tool_call_id.to_string());
    }
    for child in &component.children {
        collect_tool_call_ids_from_component(child, out);
    }
}

fn collect_tool_call_node_ids(node: &Value, out: &mut Vec<String>) {
    if node_value_node_type(node) == Some("tool_call")
        && let Some(tool_call_id) = tool_call_id_from_node(node)
    {
        out.push(tool_call_id);
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_tool_call_node_ids(child, out);
        }
    }
}

fn operation_trace_aux(op: &PactOperation) -> BTreeMap<String, Value> {
    op.envelope().aux.clone()
}

fn staged_trace_record_matches(record: &StagedTraceRecord, target: &StagedTraceTarget) -> bool {
    match target {
        StagedTraceTarget::TraceId(trace_id) => record.trace.trace_id == *trace_id,
        StagedTraceTarget::IdempotencyKey(idempotency_key) => record
            .trace
            .idempotency_key
            .as_deref()
            .is_some_and(|value| value == idempotency_key),
        StagedTraceTarget::OperationKind(kind) => record.trace.op_kind.eq_ignore_ascii_case(kind),
        StagedTraceTarget::ComponentId(component_id) => {
            record
                .trace
                .ops
                .iter()
                .any(|op| op.component_id == *component_id)
                || record
                    .trace
                    .produced_refs
                    .iter()
                    .any(|pact_ref| pact_ref.component_id.as_deref() == Some(component_id))
                || operation_mentions_component_id(&record.op, component_id)
        }
        StagedTraceTarget::Selector(_) => false,
    }
}

fn operation_mentions_component_id(op: &PactOperation, component_id: &str) -> bool {
    match op {
        PactOperation::Insert {
            parent_id,
            component,
            ..
        } => parent_id.as_deref() == Some(component_id) || component.id == component_id,
        PactOperation::Update {
            target_component_id,
            ..
        }
        | PactOperation::Delete {
            target_component_id,
            ..
        }
        | PactOperation::Move {
            target_component_id,
            ..
        } => target_component_id.as_deref() == Some(component_id),
        PactOperation::Replace {
            target_component_id,
            component,
            ..
        } => target_component_id.as_deref() == Some(component_id) || component.id == component_id,
        PactOperation::Append {
            target_component_id,
            component,
            ..
        } => target_component_id.as_deref() == Some(component_id) || component.id == component_id,
        PactOperation::Upsert {
            target_component_id,
            parent_id,
            component,
            ..
        } => {
            target_component_id.as_deref() == Some(component_id)
                || parent_id.as_deref() == Some(component_id)
                || component.id == component_id
        }
    }
}

fn dedup_node_ids(ids: &mut Vec<String>) {
    ids.sort();
    ids.dedup();
}

fn node_value_node_type(node: &Value) -> Option<&str> {
    node.get("componentType")
        .or_else(|| node.get("type"))
        .and_then(Value::as_str)
}

fn classify_produced_message_ids(tree: &Value, produced: &[String]) -> (Vec<String>, Vec<String>) {
    let produced_set: BTreeSet<&str> = produced.iter().map(String::as_str).collect();
    let mut message_ids: Vec<String> = Vec::new();
    let mut child_ids: Vec<String> = Vec::new();
    classify_produced_walk(tree, false, &produced_set, &mut message_ids, &mut child_ids);

    for id in produced {
        if message_ids.contains(id) || child_ids.contains(id) {
            continue;
        }
        if let Some(node) = find_node_value(tree, id)
            && node_value_node_type(node) == Some("text_content")
            && node.get("parent_id").and_then(Value::as_str) == Some("system")
        {
            message_ids.push(id.clone());
        }
    }
    dedup_node_ids(&mut message_ids);
    dedup_node_ids(&mut child_ids);
    (message_ids, child_ids)
}

fn classify_produced_walk(
    node: &Value,
    inside_produced_message: bool,
    produced: &BTreeSet<&str>,
    message_ids: &mut Vec<String>,
    child_ids: &mut Vec<String>,
) {
    let component_id = node.get("id").and_then(Value::as_str);
    let is_message_turn = node_value_node_type(node) == Some("message_turn");
    let is_produced = component_id.is_some_and(|id| produced.contains(id));

    let mut descend_inside_message = inside_produced_message;
    if let Some(id) = component_id {
        if is_produced && is_message_turn {
            message_ids.push(id.to_string());
            descend_inside_message = true;
        } else if is_produced && inside_produced_message {
            child_ids.push(id.to_string());
        }
    }
    if is_message_turn && is_produced {
        descend_inside_message = true;
    }

    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            classify_produced_walk(
                child,
                descend_inside_message,
                produced,
                message_ids,
                child_ids,
            );
        }
    }
}

fn observe_tool_nodes_after_mutation(
    tree: &Value,
    result: &MutationApplyResult,
    removed_tool_call_ids: &[String],
    pending: &mut PendingExternalMutations,
    requirement_counter: &mut u64,
    branch_id: &str,
    boundary: BoundaryKind,
) {
    for tool_call_id in removed_tool_call_ids {
        pending.fulfill(&format!("tool_exec:{tool_call_id}"));
    }

    let mut observed_inserted_node = false;
    for component_id in &result.report.inserted_ids {
        if let Some(node) = find_node_value(tree, component_id) {
            observe_tool_node_tree(node, pending, requirement_counter, branch_id, boundary);
            observed_inserted_node = true;
        }
    }
    if !observed_inserted_node
        && let Some(node) = &result.request.node
        && let Ok(value) = serde_json::to_value(node)
    {
        observe_tool_node_tree(&value, pending, requirement_counter, branch_id, boundary);
    }
}

fn observe_tool_node_tree(
    node: &Value,
    pending: &mut PendingExternalMutations,
    requirement_counter: &mut u64,
    branch_id: &str,
    boundary: BoundaryKind,
) {
    match tool_node_kind(node) {
        Some("tool_call") => {
            if let Some(tool_call_id) = tool_call_id_from_node(node) {
                *requirement_counter += 1;
                pending.record(ExternalMutationRequirement {
                    schema: crate::boundary::EXTERNAL_MUTATION_REQUIREMENT_SCHEMA.to_string(),
                    requirement_id: format!(
                        "req_{}_{}",
                        boundary_label(boundary),
                        *requirement_counter
                    ),
                    idempotency_key: format!("tool_exec:{tool_call_id}"),
                    kind: ExternalMutationKind::ExecuteTool,
                    boundary,
                    branch_id: branch_id.to_string(),
                    subject_id: Some(tool_call_id),
                    payload: BTreeMap::new(),
                    aux: BTreeMap::new(),
                });
            }
        }
        Some("tool_result") => {
            if let Some(tool_call_id) = tool_call_id_from_node(node) {
                pending.fulfill(&format!("tool_exec:{tool_call_id}"));
            }
        }
        _ => {}
    }

    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            observe_tool_node_tree(child, pending, requirement_counter, branch_id, boundary);
        }
    }
}

fn tool_call_ids_for_mutation_target(tree: &Value, target: &MutationTarget) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(component_id) = target
        .component_id
        .as_deref()
        .filter(|value| !value.is_empty())
        && let Some(node) = find_node_value(tree, component_id)
    {
        collect_tool_call_ids(node, &mut ids);
    }
    if let Some(selector) = target.selector.as_deref()
        && let Some(component_id) = selector.strip_prefix('#')
        && !component_id.is_empty()
        && let Some(node) = find_node_value(tree, component_id)
    {
        collect_tool_call_ids(node, &mut ids);
    }
    ids.sort();
    ids.dedup();
    ids
}

fn collect_tool_call_ids(node: &Value, out: &mut Vec<String>) {
    if let Some(tool_call_id) = tool_call_id_from_node(node) {
        out.push(tool_call_id);
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_tool_call_ids(child, out);
        }
    }
}

fn collect_subtree_ids(node: &Value, out: &mut Vec<String>) {
    if let Some(id) = node.get("id").and_then(Value::as_str) {
        out.push(id.to_string());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_subtree_ids(child, out);
        }
    }
}

fn tool_call_id_from_node(node: &Value) -> Option<String> {
    node.get("tool_call_id")
        .and_then(Value::as_str)
        .or_else(|| {
            node.get("aux")
                .and_then(|aux| aux.get("tool_call_id"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
}

fn tool_node_kind(node: &Value) -> Option<&'static str> {
    match node
        .get("componentType")
        .or_else(|| node.get("type"))
        .and_then(Value::as_str)
    {
        Some("tool_call") => Some("tool_call"),
        Some("tool_result") => Some("tool_result"),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
struct DerivedToolResult {
    component_id: Option<String>,
    status: Option<ToolCallStatus>,
    summary: Option<String>,
    result_ref: Option<String>,
}

fn tool_call_records_from_tree(tree: &Value, branch_id: &str) -> Vec<ToolCallRecord> {
    let mut calls = Vec::new();
    let mut results = BTreeMap::new();
    let root_id = tree.get("id").and_then(Value::as_str).unwrap_or("root");
    collect_tool_records(tree, branch_id, root_id, &mut calls, &mut results);

    for call in &mut calls {
        if let Some(result) = results.get(&call.tool_call_id) {
            call.status = result.status.unwrap_or(ToolCallStatus::Completed);
            call.result_node_id = result.component_id.clone();
            call.result_summary = result.summary.clone();
            call.result_ref = result.result_ref.clone();
        }
    }

    calls.sort_by(|a, b| a.tool_call_id.cmp(&b.tool_call_id));
    calls
}

fn collect_tool_records(
    node: &Value,
    branch_id: &str,
    current_message_id: &str,
    calls: &mut Vec<ToolCallRecord>,
    results: &mut BTreeMap<String, DerivedToolResult>,
) {
    let component_id = node
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(current_message_id);
    let next_message_id =
        if node.get("componentType").and_then(Value::as_str) == Some("message_turn") {
            component_id
        } else {
            current_message_id
        };

    match tool_node_kind(node) {
        Some("tool_call") => {
            if let Some(tool_call_id) = tool_call_id_from_node(node) {
                let aux = node.get("aux").unwrap_or(&Value::Null);
                calls.push(ToolCallRecord {
                    schema: TOOL_RECORD_SCHEMA.to_string(),
                    tool_call_id,
                    tool_name: node
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .or_else(|| aux.get("tool_name").and_then(Value::as_str))
                        .unwrap_or("unknown")
                        .to_string(),
                    status: ToolCallStatus::Running,
                    branch_id: branch_id.to_string(),
                    call_node_id: Some(component_id.to_string()),
                    result_node_id: None,
                    parent_message_id: next_message_id.to_string(),
                    result_summary: None,
                    result_ref: None,
                });
            }
        }
        Some("tool_result") => {
            if let Some(tool_call_id) = tool_call_id_from_node(node) {
                let aux = node.get("aux").unwrap_or(&Value::Null);
                let success = node
                    .get("success")
                    .and_then(Value::as_bool)
                    .or_else(|| aux.get("success").and_then(Value::as_bool))
                    .unwrap_or(true);
                let status = if success {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };
                let summary = node
                    .get("content")
                    .and_then(Value::as_str)
                    .or_else(|| aux.get("content").and_then(Value::as_str))
                    .or_else(|| node.get("error_message").and_then(Value::as_str))
                    .or_else(|| aux.get("error_message").and_then(Value::as_str))
                    .map(str::to_string);
                let result_ref = node
                    .get("result_ref")
                    .and_then(Value::as_str)
                    .or_else(|| aux.get("result_ref").and_then(Value::as_str))
                    .map(str::to_string);
                results.insert(
                    tool_call_id,
                    DerivedToolResult {
                        component_id: Some(component_id.to_string()),
                        status: Some(status),
                        summary,
                        result_ref,
                    },
                );
            }
        }
        _ => {}
    }

    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_tool_records(child, branch_id, next_message_id, calls, results);
        }
    }
}

fn operation_request_content_refs(
    request: &MutationRequest,
    root_id: &str,
    branch_id: &str,
) -> Vec<PactRef> {
    let mut refs = Vec::new();
    if let Some(content_ref) = &request.content_ref {
        refs.push(PactRef {
            root_id: root_id.to_string(),
            branch_id: branch_id.to_string(),
            revision: None,
            transaction_id: None,
            component_id: request.target.component_id.clone(),
            content_ref: Some(content_ref.id.clone()),
        });
    }
    if let Some(node) = &request.node {
        collect_node_content_refs(node, root_id, branch_id, &mut refs);
    }
    refs
}

fn mutation_result_trace_ops(
    request: &MutationRequest,
    report: &OperationReport,
    before_tree: &ComponentTree,
    after_tree: &ComponentTree,
    derived_depth: bool,
) -> PactResult<Option<Vec<TraceOp>>> {
    match request.kind {
        MutationKind::Insert => {
            let mut ops = Vec::new();
            for component_id in &report.inserted_ids {
                append_insert_trace_ops(
                    after_tree,
                    component_id,
                    &mut ops,
                    report.shifted_offsets.is_empty()
                        && is_root_tail_append(before_tree, after_tree, component_id),
                )?;
            }
            append_report_position_trace_ops(report, &mut ops, derived_depth);
            Ok(Some(ops))
        }
        MutationKind::Update if request.mode.as_deref() == Some("append") => {
            let mut ops = Vec::new();
            for component_id in &report.inserted_ids {
                append_insert_trace_ops(
                    after_tree,
                    component_id,
                    &mut ops,
                    report.shifted_offsets.is_empty()
                        && is_root_tail_append(before_tree, after_tree, component_id),
                )?;
            }
            append_report_position_trace_ops(report, &mut ops, derived_depth);
            Ok(Some(ops))
        }
        MutationKind::Update if request.node.is_some() => {
            let Some(component_id) = report
                .replaced_ids
                .first()
                .or(request.target.component_id.as_ref())
            else {
                return Ok(None);
            };
            let mut ops = vec![TraceOp::new(
                component_id.clone(),
                TraceOpKind::DeleteComponent,
            )];
            append_insert_trace_ops(after_tree, component_id, &mut ops, false)?;
            Ok(Some(ops))
        }
        MutationKind::Update => {
            let Some(component_id) = report
                .replaced_ids
                .first()
                .or(request.target.component_id.as_ref())
            else {
                return Ok(None);
            };
            let mut ops = Vec::new();
            for (key, value) in &request.fields {
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::SetField {
                        key: key.clone(),
                        value: value.clone(),
                    },
                ));
            }
            for (key, value) in &request.attrs {
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::SetAttr {
                        key: key.clone(),
                        value: value.clone(),
                    },
                ));
            }
            if request.content.is_some() || request.content_ref.is_some() {
                let after_value = after_tree
                    .component(component_id)
                    .ok_or_else(|| {
                        PactError::not_found(format!(
                            "updated component `{component_id}` not found for trace op lowering"
                        ))
                    })?
                    .project_component_value();
                ops.push(TraceOp::new(
                    component_id.clone(),
                    TraceOpKind::SetContent {
                        content: after_value
                            .get("content")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        content_ref: after_value.get("content_ref").cloned(),
                        content_preview: after_value
                            .get("content_preview")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
                ));
            }
            Ok(Some(ops))
        }
        MutationKind::Delete => {
            if report.removed_ids.is_empty() {
                return Ok(None);
            };
            let mut ops = Vec::new();
            for component_id in top_level_removed_component_ids(before_tree, &report.removed_ids) {
                ops.push(TraceOp::new(component_id, TraceOpKind::DeleteComponent));
            }
            for shift in &report.shifted_offsets {
                ops.push(TraceOp::new(
                    shift.component_id.clone(),
                    TraceOpKind::SetField {
                        key: "offset".to_string(),
                        value: Value::Number(shift.to_offset.into()),
                    },
                ));
            }
            if !derived_depth {
                for shift in &report.shifted_depths {
                    ops.push(TraceOp::new(
                        shift.component_id.clone(),
                        TraceOpKind::SetField {
                            key: "pact_depth".to_string(),
                            value: Value::Number(shift.to_depth.into()),
                        },
                    ));
                }
            }
            for parent_id in
                surviving_removed_parent_ids(before_tree, after_tree, &report.removed_ids)
            {
                push_child_ids_trace_op(after_tree, &parent_id, &mut ops)?;
            }
            Ok(Some(ops))
        }
        MutationKind::Move => {
            let Some(component_id) = report
                .parent_changes
                .first()
                .map(|change| &change.component_id)
                .or(request.target.component_id.as_ref())
            else {
                return Ok(None);
            };
            let path = after_tree.component_path(component_id).ok_or_else(|| {
                PactError::not_found(format!(
                    "moved component `{component_id}` not found for trace op lowering"
                ))
            })?;
            let parent_id = path.parent_id.clone().ok_or_else(|| {
                PactError::invalid(format!(
                    "moved component `{component_id}` has no parent for trace op lowering"
                ))
            })?;
            let index = *path.path.last().ok_or_else(|| {
                PactError::invalid(format!(
                    "moved component `{component_id}` has no child index for trace op lowering"
                ))
            })?;
            let mut ops = vec![TraceOp::new(
                component_id.clone(),
                TraceOpKind::MoveComponent {
                    parent_id: parent_id.clone(),
                    index,
                },
            )];
            ops.push(TraceOp::new(
                component_id.clone(),
                TraceOpKind::SetField {
                    key: "parent_id".to_string(),
                    value: Value::String(parent_id.clone()),
                },
            ));
            for shift in &report.shifted_offsets {
                ops.push(TraceOp::new(
                    shift.component_id.clone(),
                    TraceOpKind::SetField {
                        key: "offset".to_string(),
                        value: Value::Number(shift.to_offset.into()),
                    },
                ));
            }
            if !derived_depth {
                for shift in &report.shifted_depths {
                    ops.push(TraceOp::new(
                        shift.component_id.clone(),
                        TraceOpKind::SetField {
                            key: "pact_depth".to_string(),
                            value: Value::Number(shift.to_depth.into()),
                        },
                    ));
                }
            }
            if let Some(old_parent_id) = before_tree.parent_id(component_id)
                && old_parent_id != parent_id
            {
                push_child_ids_trace_op(after_tree, old_parent_id, &mut ops)?;
            }
            push_child_ids_trace_op(after_tree, &parent_id, &mut ops)?;
            Ok(Some(ops))
        }
    }
}

fn sealed_transaction_trace_from_boundary(
    boundary: &ResolvedBoundary,
    from_rev: u64,
    to_rev: u64,
) -> Option<TransactionTrace> {
    let ops = boundary
        .traces
        .iter()
        .flat_map(|trace| trace.ops.iter().cloned())
        .collect::<Vec<_>>();
    (!ops.is_empty()).then_some(TransactionTrace {
        from_rev,
        to_rev,
        ops,
    })
}

fn is_root_tail_append(before: &ComponentTree, after: &ComponentTree, component_id: &str) -> bool {
    let (Some(before_root), Some(after_root)) = (
        before.component(before.root_id()),
        after.component(after.root_id()),
    ) else {
        return false;
    };
    let before_ids = before_root
        .children
        .iter()
        .map(|child| child.id.as_str())
        .collect::<Vec<_>>();
    let after_ids = after_root
        .children
        .iter()
        .map(|child| child.id.as_str())
        .collect::<Vec<_>>();
    after_ids.last() == Some(&component_id)
        && after_ids.len() == before_ids.len() + 1
        && after_ids[..before_ids.len()] == before_ids[..]
}

fn append_insert_trace_ops(
    tree: &ComponentTree,
    component_id: &str,
    ops: &mut Vec<TraceOp>,
    elide_root_tail: bool,
) -> PactResult<()> {
    let path = tree.component_path(component_id).ok_or_else(|| {
        PactError::not_found(format!(
            "inserted component `{component_id}` not found for trace op lowering"
        ))
    })?;
    let parent_id = path.parent_id.clone().ok_or_else(|| {
        PactError::invalid(format!(
            "inserted component `{component_id}` has no parent for trace op lowering"
        ))
    })?;
    let index = *path.path.last().ok_or_else(|| {
        PactError::invalid(format!(
            "inserted component `{component_id}` has no child index for trace op lowering"
        ))
    })?;
    let component = tree.component(component_id).ok_or_else(|| {
        PactError::not_found(format!(
            "inserted component `{component_id}` not found for trace op lowering"
        ))
    })?;
    let mut payload = component.project_component_value();
    strip_insert_payload_children(&mut payload);
    ops.push(TraceOp::new(
        component_id.to_string(),
        TraceOpKind::InsertComponent {
            parent_id: parent_id.clone(),
            index,
            component: payload,
        },
    ));

    let root_tail_append = elide_root_tail
        && parent_id == tree.root_id()
        && tree
            .component(&parent_id)
            .is_some_and(|parent| index + 1 == parent.children.len());
    if !root_tail_append {
        push_child_ids_trace_op(tree, &parent_id, ops)?;
    }
    for child in &component.children {
        append_insert_trace_ops(tree, &child.id, ops, false)?;
    }
    Ok(())
}

fn push_child_ids_trace_op(
    tree: &ComponentTree,
    component_id: &str,
    ops: &mut Vec<TraceOp>,
) -> PactResult<()> {
    let child_ids = tree
        .component(component_id)
        .ok_or_else(|| {
            PactError::not_found(format!(
                "component `{component_id}` not found for child_ids trace op lowering"
            ))
        })?
        .children
        .iter()
        .map(|child| Value::String(child.id.clone()))
        .collect();
    ops.push(TraceOp::new(
        component_id.to_string(),
        TraceOpKind::SetField {
            key: "child_ids".to_string(),
            value: Value::Array(child_ids),
        },
    ));
    Ok(())
}

fn append_report_position_trace_ops(
    report: &OperationReport,
    ops: &mut Vec<TraceOp>,
    derived_depth: bool,
) {
    for shift in &report.shifted_offsets {
        ops.push(TraceOp::new(
            shift.component_id.clone(),
            TraceOpKind::SetField {
                key: "offset".to_string(),
                value: Value::Number(shift.to_offset.into()),
            },
        ));
    }

    if !derived_depth {
        for shift in &report.shifted_depths {
            ops.push(TraceOp::new(
                shift.component_id.clone(),
                TraceOpKind::SetField {
                    key: "pact_depth".to_string(),
                    value: Value::Number(shift.to_depth.into()),
                },
            ));
        }
    }
    for change in &report.parent_changes {
        let value = change
            .to_parent_id
            .as_ref()
            .map(|parent_id| Value::String(parent_id.clone()))
            .unwrap_or(Value::Null);
        ops.push(TraceOp::new(
            change.component_id.clone(),
            TraceOpKind::SetField {
                key: "parent_id".to_string(),
                value,
            },
        ));
    }
}

fn append_lifecycle_sync_trace_ops(
    tree: &ComponentTree,
    component_ids: &[String],
    ops: &mut Vec<TraceOp>,
) -> PactResult<()> {
    for component_id in component_ids {
        let component = tree.component(component_id).ok_or_else(|| {
            PactError::not_found(format!(
                "lifecycle-synced component `{component_id}` not found for trace op lowering"
            ))
        })?;
        let value = component.project_component_value();
        let ttl = value.get("ttl").cloned().unwrap_or(Value::Null);
        ops.push(TraceOp::new(
            component_id.clone(),
            TraceOpKind::SetField {
                key: "ttl".to_string(),
                value: ttl,
            },
        ));
        match value.get("cad").cloned() {
            Some(value) => ops.push(TraceOp::new(
                component_id.clone(),
                TraceOpKind::SetField {
                    key: "cad".to_string(),
                    value,
                },
            )),
            None => ops.push(TraceOp::new(
                component_id.clone(),
                TraceOpKind::UnsetField {
                    key: "cad".to_string(),
                },
            )),
        }
        if let Some(lifecycle) = value.get("lifecycle").cloned() {
            ops.push(TraceOp::new(
                component_id.clone(),
                TraceOpKind::SetLifecycle { value: lifecycle },
            ));
        }
    }
    Ok(())
}

fn top_level_removed_component_ids(tree: &ComponentTree, removed_ids: &[String]) -> Vec<String> {
    removed_ids
        .iter()
        .filter(|component_id| {
            let mut parent = tree.parent_id(component_id);
            while let Some(parent_id) = parent {
                if removed_ids.iter().any(|removed_id| removed_id == parent_id) {
                    return false;
                }
                parent = tree.parent_id(parent_id);
            }
            true
        })
        .cloned()
        .collect()
}

fn surviving_removed_parent_ids(
    before_tree: &ComponentTree,
    after_tree: &ComponentTree,
    removed_ids: &[String],
) -> Vec<String> {
    let mut parent_ids = removed_ids
        .iter()
        .filter_map(|component_id| before_tree.parent_id(component_id))
        .filter(|parent_id| after_tree.component(parent_id).is_some())
        .map(str::to_string)
        .collect::<Vec<_>>();
    parent_ids.sort();
    parent_ids.dedup();
    parent_ids
}

fn strip_insert_payload_children(value: &mut Value) {
    if let Some(object) = value.as_object_mut()
        && object.contains_key("children")
    {
        object.insert("children".to_string(), Value::Array(Vec::new()));
    }
}

fn trace_ops_replay_to(before: &Value, after: &Value, ops: &[TraceOp]) -> bool {
    TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: ops.to_vec(),
    }
    .replay(before)
    .is_ok_and(|replayed| {
        crate::transaction_trace::normalize_for_transaction_trace(&replayed)
            == crate::transaction_trace::normalize_for_transaction_trace(after)
    })
}

fn trace_ops_preserve_preexisting_removals(before: &Value, after: &Value, ops: &[TraceOp]) -> bool {
    let Ok(replayed) = (TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: ops.to_vec(),
    })
    .replay(before) else {
        return false;
    };
    let (Ok(before), Ok(after), Ok(replayed)) = (
        ComponentTree::from_value(before),
        ComponentTree::from_value(after),
        ComponentTree::from_value(&replayed),
    ) else {
        return false;
    };
    before
        .component_ids()
        .all(|id| after.component(id).is_some() || replayed.component(id).is_none())
}

pub(super) fn collect_node_content_refs(
    node: &Component,
    root_id: &str,
    branch_id: &str,
    refs: &mut Vec<PactRef>,
) {
    if let Some(content_ref) = node.content_ref() {
        refs.push(PactRef {
            root_id: root_id.to_string(),
            branch_id: branch_id.to_string(),
            revision: None,
            transaction_id: None,
            component_id: Some(node.id.clone()),
            content_ref: Some(content_ref.id.clone()),
        });
    }
    for field in [node.parameters_ref(), node.result_ref(), node.error_ref()] {
        if let Some(content_ref) = content_ref_id_from_value(field) {
            refs.push(PactRef {
                root_id: root_id.to_string(),
                branch_id: branch_id.to_string(),
                revision: None,
                transaction_id: None,
                component_id: Some(node.id.clone()),
                content_ref: Some(content_ref),
            });
        }
    }
    for child in &node.children {
        collect_node_content_refs(child, root_id, branch_id, refs);
    }
}

fn content_ref_id_from_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str()
        && !text.is_empty()
    {
        return Some(text.to_string());
    }
    value
        .as_object()
        .and_then(|object| {
            object
                .get("id")
                .or_else(|| object.get("content_ref"))
                .or_else(|| object.get("ref"))
        })
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests;
