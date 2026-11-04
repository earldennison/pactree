use crate::components::Component;
use crate::error::{PactError, PactResult};
use crate::mutation::{MutationKind, MutationRequest, MutationTarget, apply_mutation_tree};
use crate::ops::PactOperation;
use crate::tree::ComponentTree;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn is_empty_message_turn_noop(op: &PactOperation) -> bool {
    let _ = op;
    false
}

pub(super) fn prune_hollow_message_turns(
    tree: &mut ComponentTree,
    recorded_mutations: &mut Vec<MutationRequest>,
) -> PactResult<Vec<String>> {
    let mut removed = Vec::new();
    loop {
        let mut node_ids = Vec::new();
        collect_hollow_context_container_ids(tree.root(), &mut node_ids);
        if node_ids.is_empty() {
            break;
        }

        for component_id in node_ids {
            let mut request = MutationRequest {
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
            };
            let report = apply_mutation_tree(tree, &mut request)?;
            removed.extend(report.removed_ids.iter().cloned());
            recorded_mutations.push(request);
        }
    }
    removed.sort();
    removed.dedup();
    Ok(removed)
}

pub(crate) fn is_hollow_context_container(
    component_type: &str,
    has_children: bool,
    has_content: bool,
    has_content_ref: bool,
) -> bool {
    matches!(component_type, "message_turn" | "message_container")
        && !has_children
        && !has_content
        && !has_content_ref
}

pub(super) fn collect_hollow_context_container_ids(component: &Component, out: &mut Vec<String>) {
    let has_children = !component.children.is_empty();
    let has_content = component.content().is_some_and(|text| !text.is_empty());
    let has_content_ref = component.content_ref().is_some();

    if is_hollow_context_container(
        component.component_type(),
        has_children,
        has_content,
        has_content_ref,
    ) {
        out.push(component.id.clone());
    }
    for child in &component.children {
        collect_hollow_context_container_ids(child, out);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ToolPairLifecycle {
    ttl: Option<i64>,
    cad: Option<i64>,
}

pub(super) fn sync_tool_pair_lifecycle(
    tree: &mut ComponentTree,
    recorded_mutations: &mut Vec<MutationRequest>,
) -> PactResult<Vec<String>> {
    let mut groups: BTreeMap<String, ToolPairLifecycle> = BTreeMap::new();
    let tool_call_ids = tree
        .tool_call_ids()
        .chain(tree.tool_result_call_ids())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for tool_call_id in tool_call_ids {
        let pair = tree.tool_pair_ids(&tool_call_id);
        for component_id in pair.call_ids.iter().chain(pair.result_ids.iter()) {
            let Some(component) = tree.component(component_id) else {
                continue;
            };
            let lifecycle = groups.entry(tool_call_id.clone()).or_default();
            merge_lifecycle_slot(
                &mut lifecycle.ttl,
                component.lifecycle.policy.ttl,
                &tool_call_id,
                "ttl",
            )?;
            merge_lifecycle_slot(
                &mut lifecycle.cad,
                component.lifecycle.policy.cad,
                &tool_call_id,
                "cad",
            )?;
        }
    }
    if groups.is_empty() {
        return Ok(Vec::new());
    }

    let mut changed_nodes = tree.edit(|root| {
        let mut changed_nodes = Vec::new();
        apply_tool_pair_lifecycle(root, &groups, &mut changed_nodes)?;
        Ok(changed_nodes)
    })?;
    changed_nodes.sort();
    changed_nodes.dedup();

    for component_id in &changed_nodes {
        let node = tree
            .component(component_id)
            .ok_or_else(|| {
                PactError::resolver(format!("tool-pair lifecycle sync lost node {component_id}"))
            })?
            .clone();
        recorded_mutations.push(MutationRequest {
            schema: crate::mutation::MUTATION_REQUEST_SCHEMA.to_string(),
            request_id: None,
            transaction_id: None,
            kind: MutationKind::Update,
            target: MutationTarget {
                selector: None,
                parent_id: None,
                component_id: Some(component_id.clone()),
                offset: None,
                depth: None,
            },
            node: Some(node),
            mode: Some("replace".to_string()),
            content: None,
            content_ref: None,
            fields: BTreeMap::new(),
            attrs: BTreeMap::new(),
        });
    }

    Ok(changed_nodes)
}

fn merge_lifecycle_slot(
    target: &mut Option<i64>,
    value: Option<i64>,
    tool_call_id: &str,
    field: &str,
) -> PactResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    match target {
        Some(existing) if *existing != value => Err(PactError::resolver(format!(
            "tool pair {tool_call_id} has conflicting {field} values: {existing} vs {value}"
        ))),
        Some(_) => Ok(()),
        None => {
            *target = Some(value);
            Ok(())
        }
    }
}

fn apply_tool_pair_lifecycle(
    component: &mut Component,
    groups: &BTreeMap<String, ToolPairLifecycle>,
    changed: &mut Vec<String>,
) -> PactResult<()> {
    if let Some(tool_call_id) = component.tool_call_id().map(str::to_string)
        && let Some(lifecycle) = groups.get(&tool_call_id)
    {
        let mut node_changed = false;
        if let Some(ttl) = lifecycle.ttl
            && component.lifecycle.policy.ttl.is_none()
        {
            component.lifecycle.policy.ttl = Some(ttl);
            node_changed = true;
        }
        if let Some(cad) = lifecycle.cad
            && component.lifecycle.policy.cad.is_none()
        {
            component.lifecycle.policy.cad = Some(cad);
            node_changed = true;
        }
        if node_changed {
            changed.push(component.id.clone());
        }
    }
    for child in &mut component.children {
        apply_tool_pair_lifecycle(child, groups, changed)?;
    }
    Ok(())
}
