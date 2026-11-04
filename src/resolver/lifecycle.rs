use crate::components::Component;
use crate::error::PactResult;
use crate::tree::{ComponentTree, ToolPairIds};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn lifecycle_expired_top_components(
    tree: &ComponentTree,
    lifecycle_seen: &BTreeMap<String, u64>,
    lifecycle_tick: u64,
) -> Vec<String> {
    let expired: BTreeSet<String> = tree
        .ttl_candidate_ids()
        .iter()
        .filter_map(|component_id| tree.component(component_id))
        .filter(|component| {
            tree.parent_id(&component.id).is_some()
                && component.lifecycle.policy.ttl.is_some_and(|ttl| {
                    let born = lifecycle_birth_tick(tree, lifecycle_seen, component);
                    let age = lifecycle_tick.saturating_sub(born) as i64;
                    age >= ttl
                })
        })
        .map(|component| component.id.clone())
        .collect();

    let mut top = expired
        .iter()
        .filter(|component_id| {
            if !is_tool_pair_lifecycle_representative(tree, &expired, component_id) {
                return false;
            }
            let mut parent = tree.parent_id(component_id);
            while let Some(parent_id) = parent {
                if expired.contains(parent_id) {
                    return false;
                }
                parent = tree.parent_id(parent_id);
            }
            true
        })
        .cloned()
        .collect::<Vec<_>>();
    top.sort_by_key(|component_id| {
        tree.component(component_id)
            .map(|component| {
                (
                    component.pact_depth.unwrap_or(0),
                    tree.parent_id(&component.id)
                        .unwrap_or_default()
                        .to_string(),
                    component.offset,
                    component.id.clone(),
                )
            })
            .unwrap_or_else(|| (0, String::new(), 0, component_id.clone()))
    });
    top
}

fn is_tool_pair_lifecycle_representative(
    tree: &ComponentTree,
    expired: &BTreeSet<String>,
    component_id: &str,
) -> bool {
    let Some(tool_call_id) = tree
        .component(component_id)
        .and_then(Component::tool_call_id)
    else {
        return true;
    };
    let pair = tree.tool_pair_ids(tool_call_id);
    let mut pair_ids = pair
        .call_ids
        .iter()
        .chain(pair.result_ids.iter())
        .filter(|id| expired.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    pair_ids.sort();
    pair_ids
        .first()
        .is_none_or(|representative| representative == component_id)
}

fn lifecycle_birth_tick(
    tree: &ComponentTree,
    lifecycle_seen: &BTreeMap<String, u64>,
    component: &Component,
) -> u64 {
    let own_born = lifecycle_seen.get(&component.id).copied().unwrap_or(0);
    let Some(tool_call_id) = component.tool_call_id() else {
        return own_born;
    };
    let ToolPairIds {
        call_ids,
        result_ids,
    } = tree.tool_pair_ids(tool_call_id);
    if call_ids.is_empty() && result_ids.is_empty() {
        return own_born;
    }
    call_ids
        .iter()
        .chain(result_ids.iter())
        .filter(|id| tree.contains(id))
        .filter_map(|id| lifecycle_seen.get(id).copied())
        .max()
        .unwrap_or(own_born)
}

pub(super) fn lifecycle_cadence_clone_tree(
    tree: &ComponentTree,
    component_id: &str,
    revision: u64,
    mut created_at_ns: impl FnMut() -> u64,
) -> PactResult<Option<(String, i32, Component)>> {
    let Some(component) = tree.component(component_id) else {
        return Ok(None);
    };
    let Some(cad) = component.lifecycle.policy.cad else {
        return Ok(None);
    };
    if cad <= 0 {
        return Ok(None);
    }
    let Some(parent_id) = tree.parent_id(component_id).map(str::to_string) else {
        return Ok(None);
    };
    let offset = component.offset;
    let mut clone = component.clone();
    rewrite_lifecycle_component_instance_ids(
        &mut clone,
        component_id,
        &parent_id,
        revision,
        &mut created_at_ns,
    );
    Ok(Some((parent_id, offset, clone)))
}

pub(super) fn rewrite_lifecycle_component_instance_ids(
    component: &mut Component,
    old_root_id: &str,
    new_parent_id: &str,
    revision: u64,
    created_at_ns: &mut impl FnMut() -> u64,
) {
    fn rewrite(
        component: &mut Component,
        parent_id: Option<&str>,
        revision: u64,
        created_at_ns: &mut impl FnMut() -> u64,
    ) {
        component.id = format!("{}@r{revision}", component.id);
        if let Some(parent_id) = parent_id {
            component.parent_id = Some(parent_id.to_string());
        }
        component.created_at_ns = created_at_ns();
        component.creation_index = revision;

        let child_parent = component.id.clone();
        for child in &mut component.children {
            rewrite(child, Some(&child_parent), revision, created_at_ns);
        }
    }

    rewrite(component, Some(new_parent_id), revision, created_at_ns);
    let key = component
        .key
        .clone()
        .unwrap_or_else(|| old_root_id.to_string());
    component.aux.insert(
        "lifecycle_prior_instance_id".to_string(),
        Value::String(old_root_id.to_string()),
    );
    component
        .aux
        .insert("lifecycle_key".to_string(), Value::String(key));
}
