use crate::components::{Component, PactContentRef};
use crate::error::{PactError, PactResult};
use crate::mutation::{
    MutationKind, MutationRequest, MutationTarget, resolve_parent_id_tree, resolve_target_id_tree,
};
use crate::ops::PactOperation;
use crate::tree::ComponentTree;
use std::collections::BTreeMap;

pub(super) fn op_to_mutations(
    op: &PactOperation,
    tree: &ComponentTree,
) -> PactResult<Vec<MutationRequest>> {
    match op {
        PactOperation::Insert {
            target_selector,
            parent_id,
            depth,
            offset,
            component,
            ..
        } => {
            let target = MutationTarget {
                selector: target_selector.clone(),
                parent_id: parent_id.clone(),
                component_id: None,
                offset: *offset,
                depth: *depth,
            };
            insert_node_mutations(tree, target, component.clone())
        }
        PactOperation::Update {
            target_component_id,
            target_selector,
            content,
            content_ref,
            fields,
            aux,
            ..
        } => {
            let target = resolved_mutation_target(
                tree,
                target_component_id.clone(),
                target_selector.clone(),
            )?;
            Ok(vec![MutationRequest {
                schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                request_id: None,
                transaction_id: None,
                kind: MutationKind::Update,
                target,
                node: None,
                mode: None,
                content: content.clone(),
                content_ref: content_ref.as_ref().map(|id| PactContentRef {
                    id: id.clone(),
                    field: None,
                    chars: None,
                    preview: None,
                    source: None,
                }),
                fields: fields.clone(),
                attrs: aux.clone(),
            }])
        }
        PactOperation::Replace {
            target_component_id,
            target_selector,
            component,
            ..
        } => {
            let target = resolved_mutation_target(
                tree,
                target_component_id.clone(),
                target_selector.clone(),
            )?;
            Ok(vec![MutationRequest {
                schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                request_id: None,
                transaction_id: None,
                kind: MutationKind::Update,
                target,
                node: Some(component.clone()),
                mode: Some("replace".to_string()),
                content: None,
                content_ref: None,
                fields: BTreeMap::new(),
                attrs: BTreeMap::new(),
            }])
        }
        PactOperation::Append {
            target_component_id,
            target_selector,
            offset,
            component,
            ..
        } => {
            let target = MutationTarget {
                selector: target_selector.clone(),
                parent_id: None,
                component_id: target_component_id.clone(),
                offset: *offset,
                depth: None,
            };
            let mut requests = if managed_slot_key(component).is_some() {
                let _ = resolve_target_id_tree(tree, &target)?;
                keyed_slot_delete_mutations(tree, component)
            } else {
                Vec::new()
            };
            requests.push(MutationRequest {
                schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                request_id: None,
                transaction_id: None,
                kind: MutationKind::Update,
                target,
                node: Some(component.clone()),
                mode: Some("append".to_string()),
                content: None,
                content_ref: None,
                fields: BTreeMap::new(),
                attrs: BTreeMap::new(),
            });
            Ok(requests)
        }
        PactOperation::Upsert {
            target_component_id,
            target_selector,
            parent_id,
            depth,
            offset,
            component,
            ..
        } => {
            if let Some(target_id) = upsert_existing_target_id(
                tree,
                target_component_id.clone(),
                target_selector.clone(),
                component,
            )? {
                return Ok(vec![MutationRequest {
                    schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                    request_id: None,
                    transaction_id: None,
                    kind: MutationKind::Update,
                    target: MutationTarget {
                        selector: None,
                        parent_id: None,
                        component_id: Some(target_id),
                        offset: None,
                        depth: None,
                    },
                    node: Some(component.clone()),
                    mode: Some("replace".to_string()),
                    content: None,
                    content_ref: None,
                    fields: BTreeMap::new(),
                    attrs: BTreeMap::new(),
                }]);
            }
            let target = MutationTarget {
                selector: None,
                parent_id: parent_id.clone(),
                component_id: None,
                offset: *offset,
                depth: *depth,
            };
            insert_node_mutations(tree, target, component.clone())
        }
        PactOperation::Delete {
            target_component_id,
            target_selector,
            ..
        } => delete_node_mutations(tree, target_component_id.clone(), target_selector.clone()),
        PactOperation::Move {
            target_component_id,
            target_selector,
            destination_selector,
            offset,
            mode,
            ..
        } => {
            let source = resolved_mutation_target(
                tree,
                target_component_id.clone(),
                target_selector.clone(),
            )?;

            if let Some(source_id) = source.component_id.as_deref()
                && let Some(node) = tree.component(source_id)
                && matches!(node.component_type(), "message_turn" | "system_header")
            {
                return Err(PactError::invalid(format!(
                    "move: `{source_id}` is a message container; only elements within containers move (v1)"
                )));
            }
            let mode = mode.as_deref().unwrap_or("insert");
            let mut requests = Vec::new();

            let mut destination = MutationTarget {
                selector: Some(destination_selector.clone()),
                parent_id: None,
                component_id: source.component_id.clone(),
                offset: *offset,
                depth: None,
            };
            if mode == "replace" {
                let occupant_ids =
                    crate::selector::selected_component_ids_tree(tree, destination_selector)?;
                let occupant =
                    crate::mutation::exactly_one_id(occupant_ids, "move replace destination")?;
                if Some(&occupant) != source.component_id.as_ref() {
                    let occupant_parent = tree
                        .parent_id(&occupant)
                        .map(str::to_string)
                        .ok_or_else(|| {
                            PactError::invalid(format!(
                                "move replace destination `{occupant}` has no parent"
                            ))
                        })?;
                    let occupant_offset =
                        tree.component(&occupant).map(|component| component.offset);
                    destination = MutationTarget {
                        selector: None,
                        parent_id: Some(occupant_parent),
                        component_id: source.component_id.clone(),
                        offset: offset.or(occupant_offset),
                        depth: None,
                    };
                    requests.extend(delete_node_mutations(tree, Some(occupant), None)?);
                }
            }
            requests.push(MutationRequest {
                schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
                request_id: None,
                transaction_id: None,
                kind: MutationKind::Move,
                target: destination,
                node: None,
                mode: Some(mode.to_string()),
                content: None,
                content_ref: None,
                fields: BTreeMap::new(),
                attrs: BTreeMap::new(),
            });
            Ok(requests)
        }
    }
}

fn upsert_existing_target_id(
    tree: &ComponentTree,
    target_component_id: Option<String>,
    target_selector: Option<String>,
    component: &Component,
) -> PactResult<Option<String>> {
    if let Some(component_id) = target_component_id.filter(|id| !id.is_empty()) {
        return Ok(tree.contains(&component_id).then_some(component_id));
    }
    if let Some(selector) = target_selector.filter(|selector| !selector.is_empty()) {
        let selected: Vec<String> = crate::selector::selected_component_ids_tree(tree, &selector)?
            .into_iter()
            .collect();
        return match selected.len() {
            0 => Ok(None),
            1 => Ok(selected.into_iter().next()),
            count => Err(PactError::invalid(format!(
                "upsert target selector matched {count} nodes; expected at most one"
            ))),
        };
    }
    Ok(tree.contains(&component.id).then(|| component.id.clone()))
}

fn resolved_mutation_target(
    tree: &ComponentTree,
    target_component_id: Option<String>,
    target_selector: Option<String>,
) -> PactResult<MutationTarget> {
    if target_component_id
        .as_deref()
        .is_some_and(|id| !id.is_empty())
    {
        return Ok(MutationTarget {
            selector: target_selector,
            parent_id: None,
            component_id: target_component_id,
            offset: None,
            depth: None,
        });
    }
    let target = MutationTarget {
        selector: target_selector,
        parent_id: None,
        component_id: None,
        offset: None,
        depth: None,
    };
    let component_id = resolve_target_id_tree(tree, &target)?;
    Ok(MutationTarget {
        component_id: Some(component_id),
        ..target
    })
}

fn delete_node_mutations(
    tree: &ComponentTree,
    target_component_id: Option<String>,
    target_selector: Option<String>,
) -> PactResult<Vec<MutationRequest>> {
    let target_ids = if let Some(component_id) = target_component_id.filter(|id| !id.is_empty()) {
        vec![component_id]
    } else if let Some(selector) = target_selector.filter(|selector| !selector.is_empty()) {
        crate::selector::selected_component_ids_tree(tree, &selector)?
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };

    let target_ids = coalesce_paired_tool_delete_targets(tree, target_ids);

    Ok(target_ids
        .into_iter()
        .map(|component_id| MutationRequest {
            schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
            request_id: None,
            transaction_id: None,
            kind: MutationKind::Delete,
            target: MutationTarget {
                selector: None,
                parent_id: None,
                component_id: Some(component_id),
                offset: None,
                depth: None,
            },
            node: None,
            mode: None,
            content: None,
            content_ref: None,
            fields: BTreeMap::new(),
            attrs: BTreeMap::new(),
        })
        .collect())
}

fn coalesce_paired_tool_delete_targets(
    tree: &ComponentTree,
    target_ids: Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut pair_indexes: BTreeMap<String, usize> = BTreeMap::new();
    for component_id in target_ids {
        let tool_call_id = tree
            .component(&component_id)
            .and_then(Component::tool_call_id)
            .map(str::to_string);
        let Some(tool_call_id) = tool_call_id else {
            if !out.contains(&component_id) {
                out.push(component_id);
            }
            continue;
        };

        if let Some(index) = pair_indexes.get(&tool_call_id).copied() {
            if tree.component(&component_id).is_some_and(is_tool_call_node) {
                out[index] = component_id;
            }
            continue;
        }

        pair_indexes.insert(tool_call_id, out.len());
        out.push(component_id);
    }
    out
}

fn is_tool_call_node(node: &Component) -> bool {
    let node_type = node.component_type();
    let kind = node.subtype.as_deref().unwrap_or(node_type);
    matches!(node_type, "tool_call" | "ToolCall") || matches!(kind, "tool_call" | "ToolCall")
}

pub(super) fn insert_node_mutations(
    tree: &ComponentTree,
    target: MutationTarget,
    node: Component,
) -> PactResult<Vec<MutationRequest>> {
    let mut requests = if managed_slot_key(&node).is_some() {
        let _ = resolve_parent_id_tree(tree, &target)?;
        keyed_slot_delete_mutations(tree, &node)
    } else {
        Vec::new()
    };
    requests.push(MutationRequest {
        schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
        request_id: None,
        transaction_id: None,
        kind: MutationKind::Insert,
        target,
        node: Some(node),
        mode: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        attrs: BTreeMap::new(),
    });
    Ok(requests)
}

fn managed_slot_key(node: &Component) -> Option<&str> {
    if is_tool_protocol_node(node) {
        return None;
    }
    node.key.as_deref().filter(|key| !key.is_empty())
}

fn is_tool_protocol_node(node: &Component) -> bool {
    let node_type = node.component_type();
    let kind = node.subtype.as_deref().unwrap_or(node_type);
    if node.tool_call_id().is_some() {
        return true;
    }
    matches!(
        node_type,
        "tool_call" | "ToolCall" | "tool_result" | "ToolResult"
    ) || matches!(
        kind,
        "tool_call" | "ToolCall" | "tool_result" | "ToolResult"
    )
}

fn keyed_slot_delete_mutations(
    tree: &ComponentTree,
    inserted_node: &Component,
) -> Vec<MutationRequest> {
    let Some(slot_key) = managed_slot_key(inserted_node) else {
        return Vec::new();
    };
    let mut requests = Vec::new();
    collect_keyed_slot_delete_mutations(tree.root(), inserted_node, slot_key, &mut requests);
    requests
}

fn collect_keyed_slot_delete_mutations(
    component: &Component,
    inserted_node: &Component,
    slot_key: &str,
    requests: &mut Vec<MutationRequest>,
) {
    if component.key.as_deref().is_some_and(|key| key == slot_key)
        && component.id != inserted_node.id
    {
        requests.push(MutationRequest {
            schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
            request_id: None,
            transaction_id: None,
            kind: MutationKind::Delete,
            target: MutationTarget {
                selector: None,
                parent_id: None,
                component_id: Some(component.id.clone()),
                offset: None,
                depth: None,
            },
            node: None,
            mode: None,
            content: None,
            content_ref: None,
            fields: BTreeMap::new(),
            attrs: BTreeMap::new(),
        });
    }
    for child in &component.children {
        collect_keyed_slot_delete_mutations(child, inserted_node, slot_key, requests);
    }
}
