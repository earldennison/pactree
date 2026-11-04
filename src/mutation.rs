use crate::components::{Component, PactContentRef};
use crate::data_structures::{DepthShift, OffsetShift};
use crate::error::{PactError, PactResult};
use crate::tree::{ComponentTree, children_by_id_mut, component_by_id_mut};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const MUTATION_REQUEST_SCHEMA: &str = "pact.mutation_request.v1";
pub const OPERATION_REPORT_SCHEMA: &str = "pact.operation_report.v1";
pub(crate) const MUTATION_APPLY_RESULT_SCHEMA: &str = "pact.mutation_apply_result.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct MutationApplyResult {
    pub schema: String,
    pub request: MutationRequest,
    pub report: OperationReport,
    pub tree: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    Insert,
    Update,
    Delete,
    Move,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MutationTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
}

impl MutationTarget {
    fn has_parent_position(&self) -> bool {
        self.parent_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
            || self.depth.is_some()
            || self.offset.is_some()
            || self
                .selector
                .as_ref()
                .is_some_and(|value| !value.is_empty())
    }

    fn has_identity(&self) -> bool {
        self.component_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
            || self
                .selector
                .as_ref()
                .is_some_and(|value| !value.is_empty())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MutationRequest {
    #[serde(default = "mutation_request_schema")]
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    pub kind: MutationKind,
    #[serde(default)]
    pub target: MutationTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<Component>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<PactContentRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeDepthShift {
    pub component_id: String,
    pub from_depth: i32,
    pub to_depth: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeOffsetShift {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub from_offset: i32,
    pub to_offset: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ParentChange {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_parent_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContentRefChange {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_content_ref: Option<PactContentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_content_ref: Option<PactContentRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct OperationReport {
    #[serde(default = "operation_report_schema")]
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserted_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shifted_depths: Vec<NodeDepthShift>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shifted_offsets: Vec<NodeOffsetShift>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_changes: Vec<ParentChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_ref_changes: Vec<ContentRefChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl OperationReport {
    pub fn empty() -> Self {
        Self {
            schema: operation_report_schema(),
            request_id: None,
            transaction_id: None,
            inserted_ids: Vec::new(),
            removed_ids: Vec::new(),
            replaced_ids: Vec::new(),
            shifted_depths: Vec::new(),
            shifted_offsets: Vec::new(),
            parent_changes: Vec::new(),
            content_ref_changes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn with_depth_shifts<I>(mut self, shifts: I) -> Self
    where
        I: IntoIterator<Item = (String, DepthShift)>,
    {
        self.shifted_depths
            .extend(
                shifts
                    .into_iter()
                    .map(|(component_id, shift)| NodeDepthShift {
                        component_id,
                        from_depth: shift.from_depth,
                        to_depth: shift.to_depth,
                    }),
            );
        self
    }

    pub fn with_offset_shifts<I>(mut self, parent_id: Option<String>, shifts: I) -> Self
    where
        I: IntoIterator<Item = (String, OffsetShift)>,
    {
        self.shifted_offsets
            .extend(
                shifts
                    .into_iter()
                    .map(|(component_id, shift)| NodeOffsetShift {
                        component_id,
                        parent_id: parent_id.clone(),
                        from_offset: shift.from_offset,
                        to_offset: shift.to_offset,
                    }),
            );
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationValidationError {
    InvalidSchema {
        expected: &'static str,
        found: String,
    },
    MissingTarget {
        kind: MutationKind,
    },
    MissingNode {
        kind: MutationKind,
    },
    MissingUpdatePayload,
    EmptyReport,
}

impl fmt::Display for MutationValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSchema { expected, found } => {
                write!(
                    formatter,
                    "invalid PACT mutation schema `{found}`; expected `{expected}`"
                )
            }
            Self::MissingTarget { kind } => {
                write!(
                    formatter,
                    "PACT mutation request {kind:?} requires a target"
                )
            }
            Self::MissingNode { kind } => {
                write!(formatter, "PACT mutation request {kind:?} requires a node")
            }
            Self::MissingUpdatePayload => {
                write!(
                    formatter,
                    "PACT update request requires node, content, content_ref, fields, or attrs"
                )
            }
            Self::EmptyReport => write!(formatter, "PACT operation report must describe a change"),
        }
    }
}

impl std::error::Error for MutationValidationError {}

pub fn mutation_request_from_json(request_json: &str) -> PactResult<MutationRequest> {
    let request: MutationRequest = serde_json::from_str(request_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT mutation request JSON: {err}")))?;
    validate_mutation_request(&request)?;
    Ok(request)
}

pub fn operation_report_from_json(report_json: &str) -> PactResult<OperationReport> {
    let report: OperationReport = serde_json::from_str(report_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT operation report JSON: {err}")))?;
    validate_operation_report(&report)?;
    Ok(report)
}

pub fn validate_mutation_request_json(request_json: &str) -> PactResult<String> {
    let request = mutation_request_from_json(request_json)?;
    serde_json::to_string_pretty(&request)
        .map_err(|err| PactError::codec(format!("failed to encode mutation request: {err}")))
}

pub fn validate_operation_report_json(report_json: &str) -> PactResult<String> {
    let report = operation_report_from_json(report_json)?;
    serde_json::to_string_pretty(&report)
        .map_err(|err| PactError::codec(format!("failed to encode operation report: {err}")))
}

pub(crate) fn apply_mutation_tree(
    tree: &mut ComponentTree,
    request: &mut MutationRequest,
) -> PactResult<OperationReport> {
    validate_mutation_request(request)?;
    let report = match request.kind {
        MutationKind::Insert => apply_insert_tree(tree, request)?,
        MutationKind::Update => apply_update_tree(tree, request)?,
        MutationKind::Delete => {
            let result = prune_tree(tree, &request.target)?;
            let (shifted_offsets, shifted_depths) = compact_after_prune_tree(tree)?;
            OperationReport {
                schema: operation_report_schema(),
                request_id: request.request_id.clone(),
                transaction_id: request.transaction_id.clone(),
                removed_ids: result.deleted_ids,
                shifted_offsets,
                shifted_depths,
                ..OperationReport::empty()
            }
        }
        MutationKind::Move => apply_move_tree(tree, request)?,
    };
    validate_operation_report(&report)?;
    Ok(report)
}

pub fn validate_mutation_request(request: &MutationRequest) -> Result<(), MutationValidationError> {
    if request.schema != MUTATION_REQUEST_SCHEMA {
        return Err(MutationValidationError::InvalidSchema {
            expected: MUTATION_REQUEST_SCHEMA,
            found: request.schema.clone(),
        });
    }

    match request.kind {
        MutationKind::Insert => {
            if request.node.is_none() {
                return Err(MutationValidationError::MissingNode {
                    kind: request.kind.clone(),
                });
            }
            if !request.target.has_parent_position() {
                return Err(MutationValidationError::MissingTarget {
                    kind: request.kind.clone(),
                });
            }
        }
        MutationKind::Update => {
            if !request.target.has_identity() {
                return Err(MutationValidationError::MissingTarget {
                    kind: request.kind.clone(),
                });
            }
            if request.node.is_none()
                && request.content.is_none()
                && request.content_ref.is_none()
                && request.fields.is_empty()
                && request.attrs.is_empty()
            {
                return Err(MutationValidationError::MissingUpdatePayload);
            }
        }
        MutationKind::Delete => {
            if !request.target.has_identity() {
                return Err(MutationValidationError::MissingTarget {
                    kind: request.kind.clone(),
                });
            }
        }
        MutationKind::Move => {
            if !request.target.has_identity() || !request.target.has_parent_position() {
                return Err(MutationValidationError::MissingTarget {
                    kind: request.kind.clone(),
                });
            }
        }
    }

    Ok(())
}

pub fn validate_operation_report(report: &OperationReport) -> Result<(), MutationValidationError> {
    if report.schema != OPERATION_REPORT_SCHEMA {
        return Err(MutationValidationError::InvalidSchema {
            expected: OPERATION_REPORT_SCHEMA,
            found: report.schema.clone(),
        });
    }
    let has_change = !report.inserted_ids.is_empty()
        || !report.removed_ids.is_empty()
        || !report.replaced_ids.is_empty()
        || !report.shifted_depths.is_empty()
        || !report.shifted_offsets.is_empty()
        || !report.parent_changes.is_empty()
        || !report.content_ref_changes.is_empty();
    if !has_change {
        return Err(MutationValidationError::EmptyReport);
    }
    Ok(())
}

fn mutation_request_schema() -> String {
    MUTATION_REQUEST_SCHEMA.to_string()
}

fn operation_report_schema() -> String {
    OPERATION_REPORT_SCHEMA.to_string()
}

fn apply_insert_tree(
    tree: &mut ComponentTree,
    request: &mut MutationRequest,
) -> PactResult<OperationReport> {
    let mut node = request
        .node
        .clone()
        .ok_or_else(|| PactError::invalid("insert mutation requires node"))?;
    let parent_id = resolve_parent_id_tree(tree, &request.target)?;
    let root_id = tree.root_id().to_string();
    let parent = tree
        .component(&parent_id)
        .ok_or_else(|| PactError::not_found(format!("insert parent `{parent_id}` not found")))?;
    let top_level_turn_insert =
        parent_id == root_id && is_turn_container_kind(node.component_type());
    let resolved_depth = if top_level_turn_insert {
        Some(0)
    } else {
        node.pact_depth.or(parent.pact_depth).or_else(|| {
            if is_system_region_component(parent) {
                Some(-1)
            } else if is_active_message_region_component(parent) {
                Some(0)
            } else {
                None
            }
        })
    };
    let requested_offset = request.target.offset.unwrap_or_else(|| {
        tree.component(&parent_id)
            .map(|parent| next_append_offset_components(&parent.children))
            .unwrap_or(0)
    });
    node.parent_id = Some(parent_id.clone());
    node.pact_depth = resolved_depth;
    if let Some(depth) = resolved_depth {
        set_subtree_depth_component(&mut node, depth);
    }
    let inserted_id = node.id.clone();

    let (shifted_depths, actual_offset, shifted_offsets) = tree.edit(|root| {
        let mut shifted_depths = Vec::new();
        let children = children_by_id_mut(root, &parent_id).ok_or_else(|| {
            PactError::not_found(format!("insert parent `{parent_id}` not found"))
        })?;
        if top_level_turn_insert {
            shift_existing_turns_for_new_active_components(children, &mut shifted_depths);
        }
        let (actual_offset, shifted_offsets) =
            assign_insert_offset_components(children, requested_offset, &parent_id);
        node.offset = actual_offset;
        children.push(node);
        sort_component_children(children);
        Ok((shifted_depths, actual_offset, shifted_offsets))
    })?;

    if let Some(req_node) = &mut request.node {
        req_node.parent_id = Some(parent_id);
        req_node.offset = actual_offset;
        req_node.pact_depth = resolved_depth;
    }

    Ok(OperationReport {
        schema: operation_report_schema(),
        request_id: request.request_id.clone(),
        transaction_id: request.transaction_id.clone(),
        inserted_ids: vec![inserted_id],
        shifted_depths,
        shifted_offsets,
        ..OperationReport::empty()
    })
}

fn apply_update_tree(
    tree: &mut ComponentTree,
    request: &mut MutationRequest,
) -> PactResult<OperationReport> {
    if request.mode.as_deref() == Some("append") {
        return apply_update_append_tree(tree, request);
    }

    let target_id = resolve_target_id_tree(tree, &request.target)?;
    let request_ref = &*request;
    let content_ref_changes = tree.edit(|root| {
        let target = component_by_id_mut(root, &target_id).ok_or_else(|| {
            PactError::not_found(format!("update target `{target_id}` not found"))
        })?;
        let prior_content_ref = target.content_ref().cloned();

        if let Some(replacement) = &request_ref.node {
            let children = target.children.clone();
            let parent_id = target.parent_id.clone();
            let offset = target.offset;
            let depth = target.pact_depth;
            let mut replacement = replacement.clone();

            replacement.id = target_id.clone();
            replacement.children = children;
            replacement.parent_id = parent_id;
            replacement.offset = offset;
            replacement.pact_depth = depth;
            *target = replacement;
        } else {
            apply_component_field_update(target, request_ref)?;
        }

        let after_content_ref = target.content_ref().cloned();
        let content_ref_changes = if prior_content_ref != after_content_ref {
            vec![ContentRefChange {
                component_id: target_id.clone(),
                from_content_ref: prior_content_ref,
                to_content_ref: after_content_ref,
            }]
        } else {
            Vec::new()
        };
        Ok(content_ref_changes)
    })?;

    Ok(OperationReport {
        schema: operation_report_schema(),
        request_id: request.request_id.clone(),
        transaction_id: request.transaction_id.clone(),
        replaced_ids: vec![target_id],
        content_ref_changes,
        ..OperationReport::empty()
    })
}

fn apply_update_append_tree(
    tree: &mut ComponentTree,
    request: &mut MutationRequest,
) -> PactResult<OperationReport> {
    let mut node = request
        .node
        .clone()
        .ok_or_else(|| PactError::invalid("append update mutation requires node"))?;
    let parent_id = resolve_target_id_tree(tree, &request.target)?;
    let requested_offset_override = request.target.offset;
    node.parent_id = Some(parent_id.clone());
    let inserted_id = node.id.clone();
    let children = &tree
        .component(&parent_id)
        .ok_or_else(|| PactError::not_found(format!("append target `{parent_id}` not found")))?
        .children;
    let requested_offset =
        requested_offset_override.unwrap_or_else(|| next_append_offset_components(children));
    let actual_offset = append_offset_components(children, requested_offset);
    node.offset = actual_offset;
    let ordered_tail = children
        .windows(2)
        .all(|pair| pair[0].offset < pair[1].offset)
        && children
            .last()
            .is_none_or(|last| last.offset < actual_offset);
    if ordered_tail {
        tree.append_child(&parent_id, node)?;
    } else {
        tree.edit(|root| {
            let children = children_by_id_mut(root, &parent_id).ok_or_else(|| {
                PactError::not_found(format!("append target `{parent_id}` not found"))
            })?;
            children.push(node);
            sort_component_children(children);
            Ok(())
        })?;
    }

    if let Some(req_node) = &mut request.node {
        req_node.parent_id = Some(parent_id.clone());
        req_node.offset = actual_offset;
    }

    Ok(OperationReport {
        schema: operation_report_schema(),
        request_id: request.request_id.clone(),
        transaction_id: request.transaction_id.clone(),
        inserted_ids: vec![inserted_id.clone()],
        shifted_offsets: Vec::new(),
        parent_changes: vec![ParentChange {
            component_id: inserted_id,
            from_parent_id: None,
            to_parent_id: Some(parent_id),
        }],
        ..OperationReport::empty()
    })
}

fn apply_move_tree(
    tree: &mut ComponentTree,
    request: &mut MutationRequest,
) -> PactResult<OperationReport> {
    let target_id = resolve_target_id_tree(tree, &request.target)?;
    if target_id == tree.root_id() {
        return Err(PactError::invalid("cannot move the PACT root node"));
    }
    let to_parent_id = resolve_parent_id_tree(tree, &request.target)?;
    if to_parent_id == target_id {
        return Err(PactError::invalid("cannot move a component under itself"));
    }
    let from_parent_id = tree
        .parent_id(&target_id)
        .map(str::to_string)
        .ok_or_else(|| PactError::invalid(format!("move target `{target_id}` has no parent")))?;
    let target = tree
        .component(&target_id)
        .ok_or_else(|| PactError::not_found(format!("move target `{target_id}` not found")))?;
    if component_subtree_contains_id(target, &to_parent_id) {
        return Err(PactError::invalid(format!(
            "cannot move component `{target_id}` under its own descendant `{to_parent_id}`"
        )));
    }

    let parent = tree
        .component(&to_parent_id)
        .ok_or_else(|| PactError::not_found(format!("move parent `{to_parent_id}` not found")))?;
    let requested_offset = request
        .target
        .offset
        .unwrap_or_else(|| next_append_offset_components(&parent.children));
    let root_id = tree.root_id().to_string();
    let top_level_turn_move =
        to_parent_id == root_id && is_turn_container_kind(target.component_type());
    let target_pact_depth = target.pact_depth;
    let resolved_depth = request.target.depth.or_else(|| {
        if top_level_turn_move {
            Some(0)
        } else {
            target_pact_depth.or(parent.pact_depth).or_else(|| {
                if is_system_region_component(parent) {
                    Some(-1)
                } else if is_active_message_region_component(parent) {
                    Some(0)
                } else {
                    None
                }
            })
        }
    });

    let edit_target_id = target_id.clone();
    let edit_to_parent_id = to_parent_id.clone();
    let (shifted_depths, shifted_offsets) = tree.edit(|root| {
        let mut moved =
            detach_component_by_id(&mut root.children, &edit_target_id).ok_or_else(|| {
                PactError::not_found(format!("move target `{edit_target_id}` not found"))
            })?;

        let mut shifted_depths = Vec::new();
        if let Some(to_depth) = resolved_depth {
            if let Some(from_depth) = moved.pact_depth {
                if from_depth != to_depth {
                    shift_subtree_depth_component(
                        &mut moved,
                        from_depth,
                        to_depth,
                        &mut shifted_depths,
                    );
                }
            } else {
                set_subtree_depth_component(&mut moved, to_depth);
            }
        }
        let from_offset = moved.offset;
        moved.parent_id = Some(edit_to_parent_id.clone());
        let children = children_by_id_mut(root, &edit_to_parent_id).ok_or_else(|| {
            PactError::not_found(format!("move parent `{edit_to_parent_id}` not found"))
        })?;
        let (actual_offset, mut shifted_offsets) =
            assign_insert_offset_components(children, requested_offset, &edit_to_parent_id);
        moved.offset = actual_offset;
        shifted_offsets.push(NodeOffsetShift {
            component_id: edit_target_id.clone(),
            parent_id: Some(edit_to_parent_id.clone()),
            from_offset,
            to_offset: actual_offset,
        });
        children.push(moved);
        sort_component_children(children);
        Ok((shifted_depths, shifted_offsets))
    })?;

    Ok(OperationReport {
        schema: operation_report_schema(),
        request_id: request.request_id.clone(),
        transaction_id: request.transaction_id.clone(),
        shifted_depths,
        shifted_offsets,
        parent_changes: vec![ParentChange {
            component_id: target_id,
            from_parent_id: Some(from_parent_id),
            to_parent_id: Some(to_parent_id),
        }],
        ..OperationReport::empty()
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PruneResult {
    pub target_ids: Vec<String>,
    pub deleted_ids: Vec<String>,
}

pub(crate) fn prune_tree(
    tree: &mut ComponentTree,
    target: &MutationTarget,
) -> PactResult<PruneResult> {
    let mut selected_ids = selected_component_ids_tree(tree, target)?;
    if selected_ids.is_empty() {
        return Ok(PruneResult {
            target_ids: Vec::new(),
            deleted_ids: Vec::new(),
        });
    }
    if selected_ids.contains(tree.root_id()) {
        return Err(PactError::invalid("cannot delete the PACT root node"));
    }

    let selected_tool_call_ids = tool_call_ids_for_components(tree, &selected_ids);
    if !selected_tool_call_ids.is_empty() {
        add_components_for_tool_call_ids(tree, &selected_tool_call_ids, &mut selected_ids);
    }

    let target_ids: Vec<String> = selected_ids.iter().cloned().collect();
    let deleted_ids = tree.edit(|root| {
        let mut deleted_ids = Vec::new();
        delete_component_ids_recursive(&mut root.children, &selected_ids, &mut deleted_ids);
        deleted_ids.sort();
        deleted_ids.dedup();
        Ok(deleted_ids)
    })?;

    Ok(PruneResult {
        target_ids,
        deleted_ids,
    })
}

fn apply_component_field_update(
    target: &mut Component,
    request: &MutationRequest,
) -> PactResult<()> {
    let children = target.children.clone();
    let internal_refs = target.lifecycle.internal_refs.clone();
    let mut value = target.project_component_value();
    let object = value
        .as_object_mut()
        .ok_or_else(|| PactError::invalid("update target is not an object"))?;
    if let Some(content) = &request.content {
        object.insert("content".to_string(), Value::String(content.clone()));
    }
    if let Some(content_ref) = &request.content_ref {
        object.insert(
            "content_ref".to_string(),
            serde_json::to_value(content_ref)
                .map_err(|err| PactError::codec(format!("failed to encode content_ref: {err}")))?,
        );
    }
    for (key, value) in &request.fields {
        object.insert(key.clone(), value.clone());
        remove_promoted_attr_shadow(object, key);
    }
    if !request.attrs.is_empty() {
        let aux = object
            .entry("aux")
            .or_insert_with(|| Value::Object(Default::default()));
        let Some(aux_object) = aux.as_object_mut() else {
            return Err(PactError::invalid("update target aux is not an object"));
        };
        for (key, value) in &request.attrs {
            aux_object.insert(key.clone(), value.clone());
        }
    }
    let mut updated = Component::from_component_value(&value)
        .map_err(|err| PactError::invalid(format!("invalid updated component: {err}")))?;
    updated.children = children;
    updated.lifecycle.internal_refs = internal_refs;
    *target = updated;
    Ok(())
}

fn selected_component_ids_tree(
    tree: &ComponentTree,
    target: &MutationTarget,
) -> PactResult<BTreeSet<String>> {
    if let Some(component_id) = &target.component_id
        && !component_id.is_empty()
    {
        return Ok(BTreeSet::from([component_id.clone()]));
    }
    if let Some(selector) = &target.selector {
        return crate::selector::selected_component_ids_tree(tree, selector);
    }
    Err(PactError::invalid(
        "mutation target requires component_id or selector",
    ))
}

pub(crate) fn resolve_parent_id_tree(
    tree: &ComponentTree,
    target: &MutationTarget,
) -> PactResult<String> {
    if let Some(parent_id) = &target.parent_id
        && !parent_id.is_empty()
    {
        return Ok(parent_id.clone());
    }
    if let Some(selector) = &target.selector {
        return exactly_one_id(
            crate::selector::selected_component_ids_tree(tree, selector)?,
            "insert parent selector",
        );
    }
    if let Some(depth) = target.depth {
        let selector = format!("d{depth}");
        return exactly_one_id(
            crate::selector::selected_component_ids_tree(tree, &selector)?,
            "insert depth target",
        );
    }
    Err(PactError::invalid(
        "insert mutation target requires parent_id, selector, or depth",
    ))
}

pub(crate) fn resolve_target_id_tree(
    tree: &ComponentTree,
    target: &MutationTarget,
) -> PactResult<String> {
    if let Some(component_id) = &target.component_id
        && !component_id.is_empty()
    {
        return Ok(component_id.clone());
    }
    if let Some(selector) = &target.selector {
        return exactly_one_id(
            crate::selector::selected_component_ids_tree(tree, selector)?,
            "mutation target selector",
        );
    }
    Err(PactError::invalid(
        "mutation target requires component_id or selector",
    ))
}

fn tool_call_ids_for_components(
    tree: &ComponentTree,
    target_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    visit_components(tree.root(), &mut |component| {
        if !target_ids.contains(&component.id) {
            return;
        }
        if let Some(tool_call_id) = component.tool_call_id() {
            out.insert(tool_call_id.to_string());
        }
    });
    out
}

fn add_components_for_tool_call_ids(
    tree: &ComponentTree,
    tool_call_ids: &BTreeSet<String>,
    target_ids: &mut BTreeSet<String>,
) {
    visit_components(tree.root(), &mut |component| {
        let Some(tool_call_id) = component.tool_call_id() else {
            return;
        };
        if tool_call_ids.contains(tool_call_id) {
            target_ids.insert(component.id.clone());
        }
    });
}

fn visit_components<F>(component: &Component, visitor: &mut F)
where
    F: FnMut(&Component),
{
    visitor(component);
    for child in &component.children {
        visit_components(child, visitor);
    }
}

fn delete_component_ids_recursive(
    children: &mut Vec<Component>,
    target_ids: &BTreeSet<String>,
    deleted_ids: &mut Vec<String>,
) {
    let mut kept = Vec::with_capacity(children.len());
    for mut child in children.drain(..) {
        if target_ids.contains(&child.id) {
            collect_subtree_ids_component(&child, deleted_ids);
        } else {
            delete_component_ids_recursive(&mut child.children, target_ids, deleted_ids);
            kept.push(child);
        }
    }
    *children = kept;
}

fn collect_subtree_ids_component(component: &Component, out: &mut Vec<String>) {
    out.push(component.id.clone());
    for child in &component.children {
        collect_subtree_ids_component(child, out);
    }
}

fn assign_insert_offset_components(
    children: &mut [Component],
    requested_offset: i32,
    parent_id: &str,
) -> (i32, Vec<NodeOffsetShift>) {
    let mut shifted_offsets = Vec::new();
    let child_offsets: Vec<i32> = children.iter().map(|child| child.offset).collect();
    let actual_offset = if requested_offset < 0 {
        let min_negative = child_offsets
            .iter()
            .copied()
            .filter(|offset| *offset < 0)
            .min()
            .unwrap_or(0);
        requested_offset.max(min_negative - 1)
    } else {
        let max_positive = child_offsets
            .iter()
            .copied()
            .filter(|offset| *offset >= 0)
            .max()
            .unwrap_or(-1);
        requested_offset.min(max_positive + 1)
    };
    for child in children {
        let current_offset = child.offset;
        let should_shift = if actual_offset < 0 {
            current_offset <= actual_offset
        } else {
            current_offset >= actual_offset
        };
        if !should_shift {
            continue;
        }
        let next_offset = if actual_offset < 0 {
            current_offset - 1
        } else {
            current_offset + 1
        };
        child.offset = next_offset;
        shifted_offsets.push(NodeOffsetShift {
            component_id: child.id.clone(),
            parent_id: Some(parent_id.to_string()),
            from_offset: current_offset,
            to_offset: next_offset,
        });
    }
    (actual_offset, shifted_offsets)
}

fn next_append_offset_components(children: &[Component]) -> i32 {
    children
        .iter()
        .map(|child| child.offset)
        .filter(|offset| *offset >= 0)
        .max()
        .map(|offset| offset + 1)
        .unwrap_or(0)
}

fn append_offset_components(children: &[Component], requested_offset: i32) -> i32 {
    if requested_offset < 0 {
        children
            .iter()
            .map(|child| child.offset)
            .filter(|offset| *offset < 0)
            .min()
            .map(|offset| offset - 1)
            .unwrap_or(-1)
    } else {
        next_append_offset_components(children)
    }
}

fn sort_component_children(children: &mut [Component]) {
    children.sort_by(|left, right| {
        (
            left.offset,
            left.priority,
            left.created_at_ns,
            left.creation_index,
            left.id.as_str(),
        )
            .cmp(&(
                right.offset,
                right.priority,
                right.created_at_ns,
                right.creation_index,
                right.id.as_str(),
            ))
    });
}

fn sort_component_child_list(parent: &Component, children: &mut [Component]) {
    if parent.component_type() == "root" {
        children.sort_by(|left, right| {
            (
                root_child_rank_component(left),
                left.offset,
                left.priority,
                left.created_at_ns,
                left.creation_index,
                left.id.as_str(),
            )
                .cmp(&(
                    root_child_rank_component(right),
                    right.offset,
                    right.priority,
                    right.created_at_ns,
                    right.creation_index,
                    right.id.as_str(),
                ))
        });
    } else if is_conversation_history_region_component(parent) {
        children.sort_by(|left, right| {
            (
                right.pact_depth.unwrap_or(0),
                left.offset,
                left.priority,
                left.created_at_ns,
                left.creation_index,
                left.id.as_str(),
            )
                .cmp(&(
                    left.pact_depth.unwrap_or(0),
                    right.offset,
                    right.priority,
                    right.created_at_ns,
                    right.creation_index,
                    right.id.as_str(),
                ))
        });
    } else {
        sort_component_children(children);
    }
}

fn compact_after_prune_tree(
    tree: &mut ComponentTree,
) -> PactResult<(Vec<NodeOffsetShift>, Vec<NodeDepthShift>)> {
    tree.edit(|root| {
        let mut shifted_offsets = Vec::new();
        compact_child_offsets_component(root, &mut shifted_offsets, true);
        let shifted_depths = compact_historical_depths_component(root);
        Ok((shifted_offsets, shifted_depths))
    })
}

fn compact_child_offsets_component(
    component: &mut Component,
    shifted_offsets: &mut Vec<NodeOffsetShift>,
    is_root: bool,
) {
    let parent_id = component.id.clone();
    let is_sequence = is_conversation_history_region_component(component);
    let skip_offset_compaction = is_root || is_sequence;
    for child in &mut component.children {
        compact_child_offsets_component(child, shifted_offsets, false);
    }
    if skip_offset_compaction {
        let parent_snapshot = component.clone();
        sort_component_child_list(&parent_snapshot, &mut component.children);
        return;
    }

    let mut positive: Vec<(usize, i32)> = component
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, child)| (child.offset >= 0).then_some((index, child.offset)))
        .collect();
    positive.sort_by_key(|(_index, offset)| *offset);
    for (next, (index, from_offset)) in positive.into_iter().enumerate() {
        let to_offset = i32::try_from(next).unwrap_or(i32::MAX);
        if from_offset != to_offset {
            component.children[index].offset = to_offset;
            shifted_offsets.push(NodeOffsetShift {
                component_id: component.children[index].id.clone(),
                parent_id: Some(parent_id.clone()),
                from_offset,
                to_offset,
            });
        }
    }

    let mut negative: Vec<(usize, i32)> = component
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, child)| (child.offset < 0).then_some((index, child.offset)))
        .collect();
    negative.sort_by_key(|right| std::cmp::Reverse(right.1));
    for (next, (index, from_offset)) in negative.into_iter().enumerate() {
        let to_offset = -1 - i32::try_from(next).unwrap_or(i32::MAX);
        if from_offset != to_offset {
            component.children[index].offset = to_offset;
            shifted_offsets.push(NodeOffsetShift {
                component_id: component.children[index].id.clone(),
                parent_id: Some(parent_id.clone()),
                from_offset,
                to_offset,
            });
        }
    }
    sort_component_children(&mut component.children);
}

fn compact_historical_depths_component(root: &mut Component) -> Vec<NodeDepthShift> {
    let Some(history) = root
        .children
        .iter_mut()
        .find(|child| is_conversation_history_region_component(child))
    else {
        return Vec::new();
    };

    let mut kept = Vec::with_capacity(history.children.len());
    for child in history.children.drain(..) {
        if !is_empty_historical_turn_component(&child) {
            kept.push(child);
        }
    }
    kept.sort_by_key(|component| component.pact_depth.unwrap_or(i32::MAX));

    let mut shifted_depths = Vec::new();
    let base_depth = kept
        .first()
        .and_then(|component| component.pact_depth)
        .map(|depth| if depth > 1 { depth - 1 } else { 1 })
        .unwrap_or(1);
    for (index, child) in kept.iter_mut().enumerate() {
        let to_depth = base_depth + i32::try_from(index).unwrap_or(i32::MAX);
        if let Some(from_depth) = child.pact_depth
            && from_depth != to_depth
        {
            shifted_depths.push(NodeDepthShift {
                component_id: child.id.clone(),
                from_depth,
                to_depth,
            });
            set_subtree_depth_component(child, to_depth);
        }
    }
    history.children = kept;
    shifted_depths
}

fn is_empty_historical_turn_component(component: &Component) -> bool {
    let Some(depth) = component.pact_depth else {
        return false;
    };
    if depth <= 0 {
        return false;
    }
    component.children.is_empty()
        || component
            .children
            .iter()
            .all(is_empty_container_like_component)
}

fn is_empty_container_like_component(component: &Component) -> bool {
    component.children.is_empty() && component.component_type() == "message_container"
}

fn shift_existing_turns_for_new_active_components(
    children: &mut [Component],
    shifted_depths: &mut Vec<NodeDepthShift>,
) {
    for child in children {
        if !is_turn_container_kind(child.component_type()) {
            continue;
        }
        let Some(from_depth) = child.pact_depth else {
            continue;
        };
        if from_depth < 0 {
            continue;
        }
        shift_subtree_depth_component(child, from_depth, from_depth + 1, shifted_depths);
    }
}

fn shift_subtree_depth_component(
    component: &mut Component,
    from_depth: i32,
    to_depth: i32,
    shifted_depths: &mut Vec<NodeDepthShift>,
) {
    shifted_depths.push(NodeDepthShift {
        component_id: component.id.clone(),
        from_depth,
        to_depth,
    });
    component.pact_depth = Some(to_depth);
    for child in &mut component.children {
        shift_subtree_depth_component(child, from_depth, to_depth, shifted_depths);
    }
}

pub(crate) fn set_subtree_depth_component(component: &mut Component, depth: i32) {
    component.pact_depth = Some(depth);
    for child in &mut component.children {
        set_subtree_depth_component(child, depth);
    }
}

pub(crate) fn derive_turn_depths_component(root: &mut Component) -> usize {
    let ranked = root
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| is_derived_depth_turn(child))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let count = ranked.len();
    for (rank_from_oldest, index) in ranked.into_iter().enumerate() {
        let depth = i32::try_from(count - 1 - rank_from_oldest).unwrap_or(i32::MAX);
        set_subtree_depth_component(&mut root.children[index], depth);
    }
    count
}

fn is_derived_depth_turn(component: &Component) -> bool {
    is_turn_container_kind(component.component_type())
        && component.pact_depth.is_some_and(|depth| depth >= 0)
        && !component.aux.contains_key("deleted_at")
}

pub(crate) fn derive_turn_depths_value(root: &mut Value) -> usize {
    let Some(children) = root.get_mut("children").and_then(Value::as_array_mut) else {
        return 0;
    };
    let ranked = children
        .iter()
        .enumerate()
        .filter(|(_, child)| is_derived_depth_turn_value(child))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let count = ranked.len();
    for (rank_from_oldest, index) in ranked.into_iter().enumerate() {
        let depth = i32::try_from(count - 1 - rank_from_oldest).unwrap_or(i32::MAX);
        set_subtree_depth_value(&mut children[index], depth);
    }
    count
}

fn is_derived_depth_turn_value(value: &Value) -> bool {
    is_turn_container_value(value)
        && node_depth(value).is_some_and(|depth| depth >= 0)
        && value
            .get("aux")
            .and_then(Value::as_object)
            .is_none_or(|aux| !aux.contains_key("deleted_at"))
}

fn set_subtree_depth_value(value: &mut Value, depth: i32) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert("pact_depth".to_string(), Value::Number(depth.into()));
    if object.contains_key("depth") {
        object.insert("depth".to_string(), Value::Number(depth.into()));
    }
    if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            set_subtree_depth_value(child, depth);
        }
    }
}

fn component_subtree_contains_id(component: &Component, component_id: &str) -> bool {
    component.id == component_id
        || component
            .children
            .iter()
            .any(|child| component_subtree_contains_id(child, component_id))
}

fn detach_component_by_id(children: &mut Vec<Component>, component_id: &str) -> Option<Component> {
    if let Some(index) = children.iter().position(|child| child.id == component_id) {
        return Some(children.remove(index));
    }
    for child in children {
        if let Some(detached) = detach_component_by_id(&mut child.children, component_id) {
            return Some(detached);
        }
    }
    None
}

fn root_child_rank_component(component: &Component) -> i32 {
    if is_system_region_component(component) {
        0
    } else if is_conversation_history_region_component(component) {
        1
    } else if is_active_message_region_component(component) {
        2
    } else {
        3
    }
}

fn is_system_region_component(component: &Component) -> bool {
    component.component_type() == "system_header"
        || component.subtype.as_deref() == Some("system_header")
}

fn is_conversation_history_region_component(component: &Component) -> bool {
    component.component_type() == "conversation_history"
        || component.subtype.as_deref() == Some("conversation_history")
}

fn is_active_message_region_component(component: &Component) -> bool {
    matches!(
        component.component_type(),
        "active_message" | "active_provider_message"
    ) || matches!(
        component.subtype.as_deref(),
        Some("active_message" | "active_provider_message")
    )
}

#[allow(dead_code)]
fn root_id(tree: &Value) -> Option<String> {
    tree.as_object()
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[allow(dead_code)]
fn tool_call_ids_for_nodes(tree: &Value, target_ids: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    visit_nodes(tree, &mut |node| {
        let Some(id) = component_id(node) else {
            return;
        };
        if !target_ids.contains(&id) {
            return;
        }
        if let Some(tool_call_id) = tool_call_id(node) {
            out.insert(tool_call_id);
        }
    });
    out
}

#[allow(dead_code)]
fn add_nodes_for_tool_call_ids(
    tree: &Value,
    tool_call_ids: &BTreeSet<String>,
    target_ids: &mut BTreeSet<String>,
) {
    visit_nodes(tree, &mut |node| {
        let Some(tool_call_id) = tool_call_id(node) else {
            return;
        };
        if !tool_call_ids.contains(&tool_call_id) {
            return;
        }
        if let Some(id) = component_id(node) {
            target_ids.insert(id);
        }
    });
}

pub(crate) fn exactly_one_id(ids: BTreeSet<String>, label: &str) -> PactResult<String> {
    match ids.len() {
        1 => ids
            .into_iter()
            .next()
            .ok_or_else(|| PactError::not_found(format!("{label} matched no nodes"))),
        0 => Err(PactError::not_found(format!("{label} matched no nodes"))),
        count => Err(PactError::invalid(format!(
            "{label} matched {count} nodes; expected exactly one"
        ))),
    }
}

fn remove_promoted_attr_shadow(object: &mut serde_json::Map<String, Value>, key: &str) {
    let Some(aux) = object.get_mut("aux").and_then(Value::as_object_mut) else {
        return;
    };
    match key {
        "cad" | "cadence" => {
            aux.remove("cad");
            aux.remove("cadence");
        }
        "tags" | "tag" => {
            aux.remove("tags");
            aux.remove("tag");
        }
        "ttl" | "key" | "tool_call_id" | "tool_name" | "parameters" | "parameters_ref"
        | "success" | "result" | "result_ref" | "error_message" | "error_ref"
        | "content_capture" | "source_ref" | "content_status" => {
            aux.remove(key);
        }
        _ => {}
    }
}

#[allow(dead_code)]
fn node_mut_by_id<'a>(value: &'a mut Value, target_id: &str) -> Option<&'a mut Value> {
    if component_id(value).as_deref() == Some(target_id) {
        return Some(value);
    }
    let children = value.as_object_mut()?.get_mut("children")?.as_array_mut()?;
    for child in children {
        if let Some(found) = node_mut_by_id(child, target_id) {
            return Some(found);
        }
    }
    None
}

#[allow(dead_code)]
fn ensure_children_mut(value: &mut Value) -> PactResult<&mut Vec<Value>> {
    let Some(object) = value.as_object_mut() else {
        return Err(PactError::invalid("mutation target is not an object"));
    };
    let children = object
        .entry("children".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    children
        .as_array_mut()
        .ok_or_else(|| PactError::invalid("mutation target children is not an array"))
}

#[allow(dead_code)]
fn assign_insert_offset(
    children: &mut [Value],
    requested_offset: i32,
    parent_id: &str,
) -> (i32, Vec<NodeOffsetShift>) {
    let mut shifted_offsets = Vec::new();
    let child_offsets: Vec<i32> = children.iter().filter_map(node_offset).collect();

    let actual_offset = if requested_offset < 0 {
        let min_negative = child_offsets
            .iter()
            .copied()
            .filter(|offset| *offset < 0)
            .min()
            .unwrap_or(0);
        requested_offset.max(min_negative - 1)
    } else {
        let max_positive = child_offsets
            .iter()
            .copied()
            .filter(|offset| *offset >= 0)
            .max()
            .unwrap_or(-1);
        requested_offset.min(max_positive + 1)
    };
    for child in children {
        let Some(current_offset) = node_offset(child) else {
            continue;
        };
        let should_shift = if actual_offset < 0 {
            current_offset <= actual_offset
        } else {
            current_offset >= actual_offset
        };
        if !should_shift {
            continue;
        }
        let next_offset = if actual_offset < 0 {
            current_offset - 1
        } else {
            current_offset + 1
        };
        if let Some(object) = child.as_object_mut() {
            object.insert("offset".to_string(), Value::Number(next_offset.into()));
            if let Some(id) = object.get("id").and_then(Value::as_str) {
                shifted_offsets.push(NodeOffsetShift {
                    component_id: id.to_string(),
                    parent_id: Some(parent_id.to_string()),
                    from_offset: current_offset,
                    to_offset: next_offset,
                });
            }
        }
    }
    (actual_offset, shifted_offsets)
}

#[allow(dead_code)]
fn next_append_offset(children: &[Value]) -> i32 {
    children
        .iter()
        .filter_map(node_offset)
        .filter(|offset| *offset >= 0)
        .max()
        .map(|offset| offset + 1)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn append_offset(children: &[Value], requested_offset: i32) -> i32 {
    if requested_offset < 0 {
        children
            .iter()
            .filter_map(node_offset)
            .filter(|offset| *offset < 0)
            .min()
            .map(|offset| offset - 1)
            .unwrap_or(-1)
    } else {
        next_append_offset(children)
    }
}

fn node_offset(value: &Value) -> Option<i32> {
    value
        .as_object()
        .and_then(|object| object.get("offset"))
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NodeSortKey {
    first: i32,
    offset: i32,
    priority: i64,
    created_at_ns: u64,
    creation_index: u64,
    id: String,
}

fn node_sort_key(value: &Value) -> NodeSortKey {
    NodeSortKey {
        first: 0,
        offset: node_offset(value).unwrap_or(0),
        priority: node_priority(value),
        created_at_ns: node_created_at_ns(value),
        creation_index: node_creation_index(value),
        id: component_id(value).unwrap_or_default(),
    }
}

fn root_child_sort_key(value: &Value) -> NodeSortKey {
    NodeSortKey {
        first: root_child_rank(value),
        ..node_sort_key(value)
    }
}

fn sequence_child_sort_key(value: &Value) -> NodeSortKey {
    NodeSortKey {
        first: -node_pact_depth(value).unwrap_or(0),
        ..node_sort_key(value)
    }
}

fn sort_children(children: &mut [Value]) {
    children.sort_by_cached_key(node_sort_key);
}

#[allow(dead_code)]
fn sort_node_children(value: &mut Value) {
    let value_node_type = node_type(value);
    let value_node_type = value_node_type.as_deref();
    let is_root = value_node_type == Some("root");
    let is_sequence = is_conversation_history_region(value);
    let Some(children) = value
        .as_object_mut()
        .and_then(|object| object.get_mut("children"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if is_root {
        children.sort_by_cached_key(root_child_sort_key);
    } else if is_sequence {
        children.sort_by_cached_key(sequence_child_sort_key);
    } else {
        sort_children(children);
    }
}

fn root_child_rank(value: &Value) -> i32 {
    if is_system_region(value) {
        0
    } else if is_conversation_history_region(value) {
        1
    } else if is_active_message_region(value) {
        2
    } else {
        3
    }
}

fn is_system_region(value: &Value) -> bool {
    matches!(node_type(value).as_deref(), Some("system_header"))
        || matches!(node_kind(value).as_deref(), Some("system_header"))
}

fn is_conversation_history_region(value: &Value) -> bool {
    matches!(node_type(value).as_deref(), Some("conversation_history"))
        || matches!(node_kind(value).as_deref(), Some("conversation_history"))
}

fn is_active_message_region(value: &Value) -> bool {
    matches!(
        node_type(value).as_deref(),
        Some("active_message" | "active_provider_message")
    ) || matches!(
        node_kind(value).as_deref(),
        Some("active_message" | "active_provider_message")
    )
}

fn node_type(value: &Value) -> Option<String> {
    value.as_object().and_then(|object| {
        object
            .get("componentType")
            .or_else(|| object.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn node_kind(value: &Value) -> Option<String> {
    value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn node_pact_depth(value: &Value) -> Option<i32> {
    value
        .as_object()
        .and_then(|object| object.get("pact_depth").or_else(|| object.get("depth")))
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

fn node_priority(value: &Value) -> i64 {
    value
        .as_object()
        .and_then(|object| object.get("priority"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn node_created_at_ns(value: &Value) -> u64 {
    value
        .as_object()
        .and_then(|object| object.get("created_at_ns"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn node_creation_index(value: &Value) -> u64 {
    value
        .as_object()
        .and_then(|object| object.get("creation_index"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn visit_nodes<F>(value: &Value, visitor: &mut F)
where
    F: FnMut(&Value),
{
    if !value.is_object() {
        return;
    }
    visitor(value);
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            visit_nodes(child, visitor);
        }
    }
}

#[allow(dead_code)]
fn shift_existing_turns_for_new_active(
    children: &mut [Value],
    shifted_depths: &mut Vec<NodeDepthShift>,
) {
    for child in children {
        if !is_turn_container_value(child) {
            continue;
        }
        let Some(from_depth) = node_depth(child) else {
            continue;
        };
        if from_depth < 0 {
            continue;
        }
        shift_subtree_depth(child, from_depth, from_depth + 1, shifted_depths);
    }
}

#[allow(dead_code)]
fn shift_subtree_depth(
    value: &mut Value,
    from_depth: i32,
    to_depth: i32,
    shifted_depths: &mut Vec<NodeDepthShift>,
) {
    if let Some(component_id) = component_id(value) {
        shifted_depths.push(NodeDepthShift {
            component_id,
            from_depth,
            to_depth,
        });
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("pact_depth".to_string(), Value::Number(to_depth.into()));
        object.insert("depth".to_string(), Value::Number(to_depth.into()));
        if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                shift_subtree_depth(child, from_depth, to_depth, shifted_depths);
            }
        }
    }
}

#[allow(dead_code)]
fn is_turn_container_value(value: &Value) -> bool {
    node_type(value).is_some_and(|node_type| is_turn_container_kind(&node_type))
}

pub(crate) fn is_turn_container_kind(kind: &str) -> bool {
    matches!(
        kind,
        "turn" | "message_turn" | "active_message" | "active_provider_message"
    )
}

fn node_depth(value: &Value) -> Option<i32> {
    value
        .as_object()
        .and_then(|object| object.get("pact_depth").or_else(|| object.get("depth")))
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

fn component_id(value: &Value) -> Option<String> {
    value
        .as_object()
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[allow(dead_code)]
fn tool_call_id(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    object
        .get("tool_call_id")
        .or_else(|| object.get("aux").and_then(|aux| aux.get("tool_call_id")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests;
