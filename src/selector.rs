use crate::components::Component;
use crate::error::{PactError, PactResult};
use crate::tree::ComponentTree;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SelectorMatch {
    pub id: String,
    #[serde(rename = "componentType")]
    pub component_type: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub offset: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pact_depth: Option<i32>,
    pub path: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SelectedComponent {
    pub id: String,
    #[serde(rename = "componentType")]
    pub component_type: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub offset: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pact_depth: Option<i32>,
    pub path: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Map::is_empty", default, alias = "attrs")]
    pub aux: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cad: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    pub archived: bool,
    pub deleted: bool,
}

trait SelectorNode: Sized {
    fn id(&self) -> &str;
    fn component_type(&self) -> &str;
    fn kind(&self) -> Option<&str>;
    fn role(&self) -> Option<&str>;
    fn offset(&self) -> i32;
    fn pact_depth(&self) -> Option<i32>;
    fn created_at_ns(&self) -> u64;
    fn creation_index(&self) -> u64;
    fn content(&self) -> Option<&str>;
    fn children(&self) -> &[Self];
    fn attr_value(&self, name: &str) -> Option<Value>;
    fn tag_matches(&self, query: &str) -> bool;
}

impl SelectorNode for Component {
    fn id(&self) -> &str {
        &self.id
    }

    fn component_type(&self) -> &str {
        self.component_type()
    }

    fn kind(&self) -> Option<&str> {
        self.subtype.as_deref()
    }

    fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    fn offset(&self) -> i32 {
        self.offset
    }

    fn pact_depth(&self) -> Option<i32> {
        self.pact_depth
    }

    fn created_at_ns(&self) -> u64 {
        self.created_at_ns
    }

    fn creation_index(&self) -> u64 {
        self.creation_index
    }

    fn content(&self) -> Option<&str> {
        self.content()
    }

    fn children(&self) -> &[Self] {
        &self.children
    }

    fn attr_value(&self, name: &str) -> Option<Value> {
        match name {
            "id" => Some(Value::String(self.id.clone())),
            "componentType" => Some(Value::String(self.component_type().to_string())),
            "node_type" => None,
            "type" => self.subtype.clone().map(Value::String),
            "role" => self.role.clone().map(Value::String),
            "offset" => Some(Value::Number(self.offset.into())),
            "priority" => Some(Value::Number(self.priority.into())),
            "pact_depth" => self.pact_depth.map(|depth| Value::Number(depth.into())),
            "created_at_ns" => Some(Value::Number(self.created_at_ns.into())),
            "creation_index" => Some(Value::Number(self.creation_index.into())),
            "key" => self.key.clone().map(Value::String),
            "ttl" => self
                .lifecycle
                .policy
                .ttl
                .map(|ttl| Value::Number(ttl.into())),
            "cad" | "cadence" => self
                .lifecycle
                .policy
                .cad
                .map(|cad| Value::Number(cad.into())),
            "tool_call_id" => self
                .tool_call_id()
                .map(|value| Value::String(value.to_string())),
            "tool_name" => self
                .tool_name()
                .map(|value| Value::String(value.to_string())),
            "success" => self.success().map(Value::Bool),
            "error_message" => self
                .error_message()
                .map(|value| Value::String(value.to_string())),
            other => self.aux.get(other).cloned(),
        }
    }

    fn tag_matches(&self, query: &str) -> bool {
        tags_match_query(self.tags.iter().map(String::as_str), query)
    }
}

pub fn select_components(
    root_value: &Value,
    selector: &str,
    include_archived: bool,
) -> PactResult<Vec<SelectedComponent>> {
    let tree = ComponentTree::from_value(root_value)?;
    select_components_tree(&tree, selector, include_archived)
}

pub(crate) fn select_components_tree(
    tree: &ComponentTree,
    selector: &str,
    include_archived: bool,
) -> PactResult<Vec<SelectedComponent>> {
    let matches = select_tree_matches(tree, selector)?;
    let mut out = Vec::with_capacity(matches.len());
    for matched in matches {
        let Some(component) = tree.component(&matched.id) else {
            continue;
        };
        let selected = selected_component_from_component(
            component,
            tree.parent_id(&matched.id).map(str::to_string),
            &matched,
        );
        if !include_archived && (selected.archived || selected.deleted) {
            continue;
        }
        out.push(selected);
    }
    Ok(out)
}

pub(crate) fn selected_component_ids_tree(
    tree: &ComponentTree,
    selector: &str,
) -> PactResult<BTreeSet<String>> {
    Ok(select_tree_matches(tree, selector)?
        .into_iter()
        .map(|matched| matched.id)
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectorIndexSeed {
    Id(String),
    Type(String),
    Attribute { key: String, value: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchSelectorIndexSeed {
    Id(String),
    Type(String),
    Attribute { key: String, value: Value },
    Tag(String),
}

pub(crate) fn search_selector_index_seed(
    selector: &str,
) -> PactResult<Option<SearchSelectorIndexSeed>> {
    let (selector, controls) = extract_controls(selector)?;
    if controls.limit.is_some() || controls.offset != 0 || controls.mode.is_some() {
        return Ok(None);
    }
    let parts = split_top_level(&selector, ',');
    if parts.len() != 1 {
        return Ok(None);
    }
    let steps = parse_selector(parts[0].trim())?;
    if steps.len() != 1 || steps[0].combinator.is_some() {
        return Ok(None);
    }
    let simple = &steps[0].simple;
    if simple.spatial.is_some()
        || simple.depth.is_some()
        || simple.position.is_some()
        || !simple.type_tags.is_empty()
        || !simple.behaviors.is_empty()
        || !simple.pseudos.is_empty()
    {
        return Ok(None);
    }
    if let Some(component_id) = &simple.id {
        return Ok(Some(SearchSelectorIndexSeed::Id(component_id.clone())));
    }
    if let Some(component_type) = &simple.type_name {
        return Ok(Some(SearchSelectorIndexSeed::Type(component_type.clone())));
    }
    if let Some(tag) = simple.tags.first() {
        return Ok(Some(SearchSelectorIndexSeed::Tag(tag.clone())));
    }
    for attr in &simple.attrs {
        if attr.operator != AttributeOperator::Equal {
            continue;
        }
        match attr.name.as_str() {
            "componentType" | "type" => {
                return Ok(Some(SearchSelectorIndexSeed::Type(attr.value.clone())));
            }
            "key" | "role" | "tool_call_id" | "tool_name" => {
                return Ok(Some(SearchSelectorIndexSeed::Attribute {
                    key: attr.name.clone(),
                    value: Value::String(attr.value.clone()),
                }));
            }
            "success" => {
                let value = match attr.value.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => continue,
                };
                return Ok(Some(SearchSelectorIndexSeed::Attribute {
                    key: attr.name.clone(),
                    value,
                }));
            }
            _ => {}
        }
    }
    Ok(None)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DepthAnchorSelector {
    pub(crate) depth: i32,
}

pub(crate) fn depth_anchor_selector(selector: &str) -> PactResult<Option<DepthAnchorSelector>> {
    let (selector, _) = extract_controls(selector)?;
    let parts = split_top_level(&selector, ',');
    if parts.len() != 1 {
        return Ok(None);
    }
    let steps = parse_selector(parts[0].trim())?;
    if steps.len() != 1 || steps[0].combinator.is_some() {
        return Ok(None);
    }
    let simple = &steps[0].simple;
    if simple.spatial.is_some()
        || simple.type_name.is_some()
        || !simple.type_tags.is_empty()
        || simple.id.is_some()
        || !simple.attrs.is_empty()
        || !simple.behaviors.is_empty()
        || !simple.pseudos.is_empty()
        || !simple.tags.is_empty()
        || simple.wildcard
    {
        return Ok(None);
    }
    match (simple.depth, &simple.position) {
        (Some(depth), None) => Ok(Some(DepthAnchorSelector { depth })),
        (None, Some(PositionSelector::Exact(coordinates))) => match coordinates.as_slice() {
            [PositionCoordinate::Offset(depth)] => Ok(Some(DepthAnchorSelector { depth: *depth })),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

pub(crate) fn selector_index_seed(selector: &str) -> PactResult<Option<SelectorIndexSeed>> {
    let (selector, _) = extract_controls(selector)?;
    let parts = split_top_level(&selector, ',');
    if parts.len() != 1 {
        return Ok(None);
    }
    let steps = parse_selector(parts[0].trim())?;
    if steps.len() != 1 || steps[0].combinator.is_some() {
        return Ok(None);
    }
    let simple = &steps[0].simple;
    if simple.spatial.is_some()
        || simple.depth.is_some()
        || simple.position.is_some()
        || !simple.type_tags.is_empty()
        || !simple.behaviors.is_empty()
        || !simple.pseudos.is_empty()
        || !simple.tags.is_empty()
        || simple.wildcard
    {
        return Ok(None);
    }
    if let Some(component_id) = &simple.id
        && simple.type_name.is_none()
        && simple.attrs.is_empty()
    {
        return Ok(Some(SelectorIndexSeed::Id(component_id.clone())));
    }
    if simple.id.is_none()
        && simple.type_name.is_none()
        && simple.attrs.len() == 1
        && simple.attrs[0].operator == AttributeOperator::Equal
        && matches!(simple.attrs[0].name.as_str(), "key" | "role")
    {
        return Ok(Some(SelectorIndexSeed::Attribute {
            key: simple.attrs[0].name.clone(),
            value: simple.attrs[0].value.clone(),
        }));
    }
    if simple.id.is_some() || !simple.attrs.is_empty() {
        return Ok(None);
    }
    Ok(simple
        .type_name
        .as_ref()
        .map(|component_type| SelectorIndexSeed::Type(component_type.clone())))
}

fn select_tree_matches(tree: &ComponentTree, selector: &str) -> PactResult<Vec<SelectorMatch>> {
    let (selector, controls) = extract_controls(selector)?;
    let mut indexed = None;
    let mut seen = BTreeSet::new();
    let mut matches = Vec::new();
    for part in split_top_level(&selector, ',') {
        let steps = parse_selector(part.trim())?;
        let part_matches = if let Some(matches) = indexed_selector_matches(tree, &steps) {
            matches
        } else {
            let indexed = indexed.get_or_insert_with(|| IndexedTree::new(tree.root()));
            execute_steps(indexed, &steps)?
                .into_iter()
                .map(|node_ref| node_ref.to_match(indexed))
                .collect()
        };
        for matched in part_matches {
            if seen.insert(matched.id.clone()) {
                matches.push(matched);
            }
        }
    }
    apply_controls(matches, &controls)
}

fn indexed_selector_matches(
    tree: &ComponentTree,
    steps: &[SelectorStep],
) -> Option<Vec<SelectorMatch>> {
    let seed_ids = indexed_selector_seed_ids(tree, steps)?;
    let simple = &steps[0].simple;
    Some(
        seed_ids
            .into_iter()
            .filter_map(|component_id| {
                let component = tree.component(&component_id)?;
                if let Some(id) = &simple.id
                    && &component.id != id
                {
                    return None;
                }
                if let Some(type_name) = &simple.type_name
                    && !type_matches(component, type_name)
                {
                    return None;
                }
                if !simple
                    .attrs
                    .iter()
                    .all(|attr| attr_matches(component, attr))
                    || !simple
                        .tags
                        .iter()
                        .all(|tag| SelectorNode::tag_matches(component, tag))
                {
                    return None;
                }
                let path = tree.component_path(&component_id)?.path.clone();
                Some(SelectorMatch {
                    id: component.id.clone(),
                    component_type: component.component_type().to_string(),
                    kind: component.subtype.clone(),
                    role: component.role.clone(),
                    offset: component.offset,
                    pact_depth: component.pact_depth,
                    path,
                    content: component.content().map(str::to_string),
                })
            })
            .collect(),
    )
}

fn indexed_selector_seed_ids(tree: &ComponentTree, steps: &[SelectorStep]) -> Option<Vec<String>> {
    if steps.len() != 1 || steps[0].combinator.is_some() {
        return None;
    }
    let simple = &steps[0].simple;
    if simple.spatial.is_some()
        || simple.depth.is_some()
        || simple.position.is_some()
        || !simple.type_tags.is_empty()
        || !simple.behaviors.is_empty()
        || !simple.pseudos.is_empty()
    {
        return None;
    }
    if let Some(component_id) = &simple.id {
        return tree
            .contains(component_id)
            .then(|| vec![component_id.clone()]);
    }
    if let Some(component_type) = &simple.type_name
        && simple.attrs.is_empty()
        && simple.tags.is_empty()
    {
        return Some(tree.ids_by_type(component_type).to_vec());
    }
    if simple.attrs.len() == 1 && simple.tags.is_empty() && simple.type_name.is_none() {
        let attr = &simple.attrs[0];
        if attr.operator == AttributeOperator::Equal {
            return match attr.name.as_str() {
                "key" => Some(tree.ids_by_key(&attr.value).to_vec()),
                "role" => Some(tree.ids_by_role(&attr.value).to_vec()),
                "componentType" | "type" => Some(tree.ids_by_type(&attr.value).to_vec()),
                _ => None,
            };
        }
    }
    None
}

pub fn select_components_json(
    pact_json: &str,
    selector: &str,
    include_archived: bool,
) -> PactResult<String> {
    let value: Value = serde_json::from_str(pact_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))?;
    let components = select_components(&value, selector, include_archived)?;
    serde_json::to_string_pretty(&components)
        .map_err(|err| PactError::codec(format!("failed to encode selected components: {err}")))
}

fn selected_component_from_component(
    component: &Component,
    indexed_parent_id: Option<String>,
    matched: &SelectorMatch,
) -> SelectedComponent {
    let aux = component
        .aux
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    SelectedComponent {
        id: matched.id.clone(),
        component_type: matched.component_type.clone(),
        kind: matched.kind.clone(),
        parent_id: indexed_parent_id.or_else(|| component.parent_id.clone()),
        role: matched.role.clone(),
        offset: matched.offset,
        pact_depth: matched.pact_depth,
        path: matched.path.clone(),
        content: matched.content.clone(),
        aux,
        content_ref: component
            .content_ref()
            .and_then(|content_ref| serde_json::to_value(content_ref).ok()),
        source_ref: component
            .source_ref()
            .and_then(|source_ref| serde_json::to_value(source_ref).ok()),
        tool_call_id: component.tool_call_id().map(str::to_string),
        tool_name: component.tool_name().map(str::to_string),
        parameters: component.parameters().cloned(),
        result: component.result().cloned(),
        success: component.success(),
        error_message: component.error_message().map(str::to_string),
        key: component.key.clone(),
        ttl: component.lifecycle.policy.ttl,
        cad: component.lifecycle.policy.cad,
        tags: component.tags.clone(),
        visibility: component
            .aux
            .get("visibility")
            .and_then(Value::as_str)
            .map(str::to_string),
        archived: component
            .aux
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        deleted: component
            .aux
            .get("deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    Child,
    Single,
    Recursive { within: Option<usize> },
}

impl Combinator {
    fn with_within(self, within: usize) -> Self {
        match self {
            Combinator::Child => Combinator::Child,
            Combinator::Single => Combinator::Single,
            Combinator::Recursive { .. } => Combinator::Recursive {
                within: Some(within),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectorStep {
    combinator: Option<Combinator>,
    simple: SimpleSelector,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SimpleSelector {
    spatial: Option<String>,
    depth: Option<i32>,
    position: Option<PositionSelector>,
    type_name: Option<String>,
    type_tags: Vec<String>,
    id: Option<String>,
    attrs: Vec<AttributeSelector>,
    behaviors: Vec<BehaviorSelector>,
    pseudos: Vec<PseudoSelector>,
    tags: Vec<String>,
    wildcard: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AttributeSelector {
    name: String,
    operator: AttributeOperator,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BehaviorSelector {
    name: String,
    operator: Option<AttributeOperator>,
    value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PseudoSelector {
    name: String,
    argument: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PositionSelector {
    Exact(Vec<PositionCoordinate>),
    DepthRange {
        start: i32,
        end: Option<i32>,
        tail: Vec<PositionCoordinate>,
    },
    Set(Vec<PositionSelector>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PositionCoordinate {
    Offset(i32),
    All,
    Range(PositionBound, PositionBound),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PositionBound {
    Offset(i32),
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttributeOperator {
    Equal,
    NotEqual,
    Regex,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SelectorControls {
    limit: Option<usize>,
    offset: usize,
    mode: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParsedSelectorControls {
    pub(crate) limit: Option<usize>,
    pub(crate) offset: usize,
    pub(crate) mode: Option<String>,
}

pub(crate) fn parse_selector_controls(
    selector: &str,
) -> PactResult<(String, ParsedSelectorControls)> {
    let (cleaned, controls) = extract_controls(selector)?;
    if let Some(mode) = controls.mode.as_deref() {
        match mode {
            "all" | "latest" => {}
            _ => {
                return Err(PactError::invalid(format!(
                    "unknown selector mode {mode:?}; expected all or latest"
                )));
            }
        }
    }
    Ok((
        cleaned,
        ParsedSelectorControls {
            limit: controls.limit,
            offset: controls.offset,
            mode: controls.mode,
        },
    ))
}

pub fn select_json(pact_json: &str, selector: &str) -> PactResult<String> {
    let value: Value = serde_json::from_str(pact_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))?;
    let tree = ComponentTree::from_value(&value)?;
    let matches = select_tree_matches(&tree, selector)?;
    serde_json::to_string_pretty(&matches)
        .map_err(|err| PactError::codec(format!("failed to encode matches: {err}")))
}

pub fn select_subtrees_json(pact_json: &str, selector: &str) -> PactResult<String> {
    let value: Value = serde_json::from_str(pact_json)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))?;
    let tree = ComponentTree::from_value(&value)?;
    let matches = select_tree_matches(&tree, selector)?;
    let mut selected = Vec::new();
    for matched in matches {
        let node = value_at_selector_path(&value, &matched.path).ok_or_else(|| {
            PactError::selector(format!(
                "selector path {:?} is not present in PACT JSON",
                matched.path
            ))
        })?;
        selected.push(node.clone());
    }
    serde_json::to_string_pretty(&selected)
        .map_err(|err| PactError::codec(format!("failed to encode selected PACT subtrees: {err}")))
}

pub(crate) fn selector_requires_structure(selector: &str) -> PactResult<bool> {
    let (selector, _controls) = extract_controls(selector)?;
    for part in split_top_level(&selector, ',') {
        let steps = parse_selector(part.trim())?;
        if steps.len() > 1 {
            return Ok(true);
        }
        for step in &steps {
            for pseudo in &step.simple.pseudos {
                if matches!(
                    pseudo.name.as_str(),
                    "has" | "first" | "first-child" | "last" | "last-child" | "nth"
                ) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub(crate) fn search_selector_requires_tree(selector: &str) -> PactResult<bool> {
    let (selector, _controls) = extract_controls(selector)?;
    for part in split_top_level(&selector, ',') {
        let steps = parse_selector(part.trim())?;
        if steps.len() > 1 {
            return Ok(true);
        }
        for step in &steps {
            if step.simple.position.is_some()
                || step.simple.pseudos.iter().any(|pseudo| {
                    matches!(
                        pseudo.name.as_str(),
                        "has" | "first" | "first-child" | "last" | "last-child" | "nth"
                    )
                })
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn selector_reads_depth(selector: &str) -> PactResult<bool> {
    let (selector, _controls) = extract_controls(selector)?;
    for part in split_top_level(&selector, ',') {
        for step in parse_selector(part.trim())? {
            if step.simple.depth.is_some()
                || step.simple.position.is_some()
                || step
                    .simple
                    .attrs
                    .iter()
                    .any(|attr| matches!(attr.name.as_str(), "pact_depth" | "depth"))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn match_node_value(node_value: &Value, selector: &str) -> PactResult<bool> {
    let tree = ComponentTree::from_value(node_value)?;
    let root_id = tree.root_id().to_string();
    Ok(select_tree_matches(&tree, selector)?
        .into_iter()
        .any(|matched| matched.id == root_id))
}

fn value_at_selector_path<'a>(root: &'a Value, path: &[usize]) -> Option<&'a Value> {
    let mut current = root;
    for index in path {
        let mut children = current
            .get("children")
            .and_then(Value::as_array)?
            .iter()
            .collect::<Vec<_>>();
        children.sort_by(|left, right| {
            (
                child_offset(left),
                child_created_at_ns(left),
                child_creation_index(left),
                child_id(left),
            )
                .cmp(&(
                    child_offset(right),
                    child_created_at_ns(right),
                    child_creation_index(right),
                    child_id(right),
                ))
        });
        current = *children.get(*index)?;
    }
    Some(current)
}

fn child_offset(value: &Value) -> i32 {
    value.get("offset").and_then(Value::as_i64).unwrap_or(0) as i32
}

fn child_created_at_ns(value: &Value) -> u64 {
    value
        .get("created_at_ns")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn child_creation_index(value: &Value) -> u64 {
    value
        .get("creation_index")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn child_id(value: &Value) -> &str {
    value.get("id").and_then(Value::as_str).unwrap_or("")
}

fn tags_match_query<'a>(mut tags: impl Iterator<Item = &'a str>, query: &str) -> bool {
    let query_segments: Vec<&str> = query.split(':').collect();
    let has_wildcard = query_segments.contains(&"*");
    tags.any(|tag| {
        let tag_segments: Vec<&str> = tag.split(':').collect();
        if query_segments.len() > tag_segments.len() {
            return false;
        }
        if has_wildcard && query_segments.len() != tag_segments.len() {
            return false;
        }
        query_segments
            .iter()
            .zip(tag_segments.iter())
            .all(|(query_segment, tag_segment)| {
                *query_segment == "*" || query_segment == tag_segment
            })
    })
}

pub(crate) fn tag_matches_query(tag: &str, query: &str) -> bool {
    tags_match_query(std::iter::once(tag), query)
}

struct ComponentRef<'a, N: SelectorNode> {
    node: &'a N,
    path_index: usize,
}

impl<N: SelectorNode> Copy for ComponentRef<'_, N> {}

impl<N: SelectorNode> Clone for ComponentRef<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, N: SelectorNode> ComponentRef<'a, N> {
    fn to_match(self, indexed: &IndexedTree<'_, N>) -> SelectorMatch {
        SelectorMatch {
            id: self.node.id().to_string(),
            component_type: self.node.component_type().to_string(),
            kind: self.node.kind().map(str::to_string),
            role: self.node.role().map(str::to_string),
            offset: self.node.offset(),
            pact_depth: self.node.pact_depth(),
            path: indexed.nodes[self.path_index].1.clone(),
            content: self.node.content().map(str::to_string),
        }
    }
}

struct IndexedTree<'a, N: SelectorNode> {
    nodes: Vec<(&'a N, Vec<usize>)>,
}

impl<'a, N: SelectorNode> IndexedTree<'a, N> {
    fn new(root: &'a N) -> Self {
        #[cfg(test)]
        crate::tests::instrumentation::SELECTOR_FULL_INDEX_COUNT
            .with(|count| count.set(count.get() + 1));
        let mut nodes = Vec::new();
        collect_components(root, &mut Vec::new(), &mut nodes);
        Self { nodes }
    }

    fn root_ref(&'a self) -> ComponentRef<'a, N> {
        ComponentRef {
            node: self.nodes[0].0,
            path_index: 0,
        }
    }

    fn all_nodes(&'a self) -> Vec<ComponentRef<'a, N>> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, (node, _))| ComponentRef {
                node: *node,
                path_index: index,
            })
            .collect()
    }

    fn direct_children(&'a self, node_ref: ComponentRef<'a, N>) -> Vec<ComponentRef<'a, N>> {
        let parent_path = &self.nodes[node_ref.path_index].1;
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, (_, path))| {
                path.len() == parent_path.len() + 1 && path.starts_with(parent_path)
            })
            .map(|(index, (node, _))| ComponentRef {
                node: *node,
                path_index: index,
            })
            .collect()
    }

    fn descendants(&'a self, node_ref: ComponentRef<'a, N>) -> Vec<ComponentRef<'a, N>> {
        let parent_path = &self.nodes[node_ref.path_index].1;
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, (_, path))| {
                path.len() > parent_path.len() && path.starts_with(parent_path)
            })
            .map(|(index, (node, _))| ComponentRef {
                node: *node,
                path_index: index,
            })
            .collect()
    }

    fn descendants_within(
        &'a self,
        node_ref: ComponentRef<'a, N>,
        within: Option<usize>,
    ) -> Vec<ComponentRef<'a, N>> {
        let parent_path = &self.nodes[node_ref.path_index].1;
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, (_, path))| {
                path.len() > parent_path.len()
                    && path.starts_with(parent_path)
                    && within
                        .map(|limit| path.len() - parent_path.len() <= limit)
                        .unwrap_or(true)
            })
            .map(|(index, (node, _))| ComponentRef {
                node: *node,
                path_index: index,
            })
            .collect()
    }

    fn resolve_positions(&'a self, position: &PositionSelector) -> Vec<ComponentRef<'a, N>> {
        match position {
            PositionSelector::Exact(position) => {
                self.resolve_exact_position(position).into_iter().collect()
            }
            PositionSelector::DepthRange { start, end, tail } => {
                let max_depth = self.max_pact_depth().unwrap_or(*start);
                let end = end.unwrap_or(max_depth);
                self.all_nodes()
                    .into_iter()
                    .filter(|node_ref| {
                        node_ref.is_pact_depth_anchor()
                            && node_ref
                                .node
                                .pact_depth()
                                .map(|depth| depth >= *start && depth <= end)
                                .unwrap_or(false)
                    })
                    .flat_map(|node_ref| self.resolve_tail(node_ref, tail))
                    .collect()
            }
            PositionSelector::Set(selectors) => selectors
                .iter()
                .flat_map(|selector| self.resolve_positions(selector))
                .collect(),
        }
    }

    fn resolve_exact_position(
        &'a self,
        position: &[PositionCoordinate],
    ) -> Vec<ComponentRef<'a, N>> {
        let Some(PositionCoordinate::Offset(depth)) = position.first() else {
            return Vec::new();
        };
        let anchor = self
            .all_nodes()
            .into_iter()
            .find(|node_ref| node_ref.matches_pact_depth_anchor(*depth));
        anchor
            .map(|anchor| self.resolve_tail(anchor, &position[1..]))
            .unwrap_or_default()
    }

    fn resolve_tail(
        &'a self,
        current: ComponentRef<'a, N>,
        tail: &[PositionCoordinate],
    ) -> Vec<ComponentRef<'a, N>> {
        if tail.is_empty() {
            return vec![current];
        }
        let mut current_nodes = vec![current];
        for coordinate in tail {
            let mut next_nodes = Vec::new();
            for node_ref in current_nodes {
                next_nodes.extend(self.resolve_child_coordinate(node_ref, coordinate));
            }
            if next_nodes.is_empty() {
                return Vec::new();
            }
            current_nodes = next_nodes;
        }
        current_nodes
    }

    fn resolve_child_coordinate(
        &'a self,
        node_ref: ComponentRef<'a, N>,
        coordinate: &PositionCoordinate,
    ) -> Vec<ComponentRef<'a, N>> {
        let children = self.direct_children(node_ref);
        match coordinate {
            PositionCoordinate::Offset(offset) => children
                .into_iter()
                .find(|child| child.node.offset() == *offset)
                .into_iter()
                .collect(),
            PositionCoordinate::All => children,
            PositionCoordinate::Range(start, end) => {
                let Some(start) = resolve_position_bound(&children, start) else {
                    return Vec::new();
                };
                let Some(end) = resolve_position_bound(&children, end) else {
                    return Vec::new();
                };
                let low = start.min(end);
                let high = start.max(end);
                children
                    .into_iter()
                    .filter(|child| child.node.offset() >= low && child.node.offset() <= high)
                    .collect()
            }
        }
    }

    fn max_pact_depth(&'a self) -> Option<i32> {
        self.all_nodes()
            .into_iter()
            .filter(|node_ref| node_ref.is_pact_depth_anchor())
            .filter_map(|node_ref| node_ref.node.pact_depth())
            .max()
    }
}

fn resolve_position_bound<N: SelectorNode>(
    children: &[ComponentRef<'_, N>],
    bound: &PositionBound,
) -> Option<i32> {
    match bound {
        PositionBound::Offset(offset) => Some(*offset),
        PositionBound::Min => children.iter().map(|child| child.node.offset()).min(),
        PositionBound::Max => children.iter().map(|child| child.node.offset()).max(),
    }
}

impl<N: SelectorNode> ComponentRef<'_, N> {
    fn matches_pact_depth_anchor(&self, depth: i32) -> bool {
        self.is_pact_depth_anchor() && self.node.pact_depth() == Some(depth)
    }

    fn is_pact_depth_anchor(&self) -> bool {
        matches!(
            self.node.component_type(),
            "system_header" | "message_turn" | "active_message" | "active_provider_message"
        ) || matches!(
            self.node.kind(),
            Some("system_header" | "message_turn" | "active_message" | "active_provider_message")
        )
    }
}

fn collect_components<'a, N: SelectorNode>(
    node: &'a N,
    path: &mut Vec<usize>,
    out: &mut Vec<(&'a N, Vec<usize>)>,
) {
    out.push((node, path.clone()));
    let mut children = node.children().iter().collect::<Vec<_>>();
    children.sort_by(|left, right| {
        (
            left.offset(),
            left.created_at_ns(),
            left.creation_index(),
            left.id(),
        )
            .cmp(&(
                right.offset(),
                right.created_at_ns(),
                right.creation_index(),
                right.id(),
            ))
    });
    for (index, child) in children.into_iter().enumerate() {
        path.push(index);
        collect_components(child, path, out);
        path.pop();
    }
}

fn execute_steps<'a, N: SelectorNode>(
    indexed: &'a IndexedTree<'a, N>,
    steps: &[SelectorStep],
) -> PactResult<Vec<ComponentRef<'a, N>>> {
    if steps.is_empty() {
        return Ok(Vec::new());
    }
    let mut current = apply_simple(indexed, &[indexed.root_ref()], &steps[0].simple, true);
    for step in steps.iter().skip(1) {
        let bases = match step.combinator {
            Some(Combinator::Child) | Some(Combinator::Single) => current
                .iter()
                .flat_map(|node_ref| indexed.direct_children(*node_ref))
                .collect::<Vec<_>>(),
            Some(Combinator::Recursive { within }) => current
                .iter()
                .flat_map(|node_ref| indexed.descendants_within(*node_ref, within))
                .collect::<Vec<_>>(),
            None => {
                return Err(PactError::invalid(
                    "selector step missing combinator".to_string(),
                ));
            }
        };
        current = apply_simple(indexed, &bases, &step.simple, false);
    }
    Ok(current)
}

fn apply_simple<'a, N: SelectorNode>(
    indexed: &'a IndexedTree<'a, N>,
    bases: &[ComponentRef<'a, N>],
    simple: &SimpleSelector,
    expand_descendants: bool,
) -> Vec<ComponentRef<'a, N>> {
    let candidates = if let Some(position) = &simple.position {
        indexed.resolve_positions(position)
    } else if let Some(depth) = simple.depth {
        indexed
            .all_nodes()
            .into_iter()
            .filter(|node_ref| node_ref.matches_pact_depth_anchor(depth))
            .collect()
    } else if expand_descendants {
        indexed.all_nodes()
    } else {
        bases.to_vec()
    };

    candidates
        .into_iter()
        .filter(|node_ref| simple_matches(indexed, *node_ref, simple))
        .collect()
}

fn simple_matches<N: SelectorNode>(
    indexed: &IndexedTree<'_, N>,
    node_ref: ComponentRef<'_, N>,
    simple: &SimpleSelector,
) -> bool {
    let node = node_ref.node;
    if let Some(spatial) = &simple.spatial
        && !spatial_matches(node, spatial)
    {
        return false;
    }
    if let Some(id) = &simple.id
        && node.id() != id
    {
        return false;
    }
    if let Some(type_name) = &simple.type_name
        && !type_matches(node, type_name)
    {
        return false;
    }
    if !simple.type_tags.iter().all(|type_tag| {
        node.component_type() == *type_tag || node.kind() == Some(type_tag.as_str())
    }) {
        return false;
    }
    if !simple.attrs.iter().all(|attr| attr_matches(node, attr)) {
        return false;
    }
    if !simple
        .behaviors
        .iter()
        .all(|behavior| behavior_matches(node, behavior))
    {
        return false;
    }
    if !simple.tags.iter().all(|query| node.tag_matches(query)) {
        return false;
    }
    if !simple
        .pseudos
        .iter()
        .all(|pseudo| pseudo_matches(indexed, node_ref, pseudo))
    {
        return false;
    }
    simple.wildcard
        || simple.spatial.is_some()
        || simple.depth.is_some()
        || simple.position.is_some()
        || simple.type_name.is_some()
        || !simple.type_tags.is_empty()
        || simple.id.is_some()
        || !simple.attrs.is_empty()
        || !simple.behaviors.is_empty()
        || !simple.pseudos.is_empty()
        || !simple.tags.is_empty()
}

fn spatial_matches<N: SelectorNode>(_node: &N, _spatial: &str) -> bool {
    false
}

fn type_matches<N: SelectorNode>(node: &N, raw_type_name: &str) -> bool {
    let type_name = match raw_type_name {
        "b" => "block",
        "c" => "cont",
        other => other,
    };
    match type_name {
        "seg" => {
            matches!(
                node.component_type(),
                "context_root"
                    | "system_header"
                    | "conversation_history"
                    | "active_message"
                    | "message_turn"
                    | "client_message"
                    | "provider_message"
                    | "active_provider_message"
            ) || matches!(
                node.kind(),
                Some(
                    "context_root"
                        | "system_header"
                        | "conversation_history"
                        | "active_message"
                        | "message_turn"
                        | "client_message"
                        | "provider_message"
                        | "active_provider_message"
                )
            )
        }
        "cont" => {
            node.component_type() == "message_container" || node.kind() == Some("message_container")
        }
        "block" => {
            matches!(
                node.component_type(),
                "text_content"
                    | "media_content"
                    | "image_content"
                    | "audio_content"
                    | "video_content"
                    | "document_content"
                    | "tool_result"
            ) || matches!(
                node.kind(),
                Some(
                    "text_content"
                        | "media_content"
                        | "image_content"
                        | "audio_content"
                        | "video_content"
                        | "document_content"
                        | "tool_result"
                )
            )
        }
        other => node.component_type() == other || node.kind() == Some(other),
    }
}

fn pseudo_matches<N: SelectorNode>(
    indexed: &IndexedTree<'_, N>,
    node_ref: ComponentRef<'_, N>,
    pseudo: &PseudoSelector,
) -> bool {
    match pseudo.name.as_str() {
        "first" | "first-child" => sibling_index(indexed, node_ref) == Some(0),
        "last" | "last-child" => {
            let Some(index) = sibling_index(indexed, node_ref) else {
                return false;
            };
            let Some(count) = sibling_count(indexed, node_ref) else {
                return false;
            };
            index + 1 == count
        }
        "nth" => {
            let Some(argument) = pseudo.argument.as_deref() else {
                return false;
            };
            let Ok(expected) = argument.trim().parse::<usize>() else {
                return false;
            };
            expected > 0 && sibling_index(indexed, node_ref) == Some(expected - 1)
        }
        "has" => {
            let Some(argument) = pseudo.argument.as_deref() else {
                return false;
            };
            let Ok(steps) = parse_selector(argument) else {
                return false;
            };
            if steps.len() != 1 {
                return false;
            }
            indexed
                .descendants(node_ref)
                .into_iter()
                .any(|descendant| simple_matches(indexed, descendant, &steps[0].simple))
        }
        _ => false,
    }
}

fn sibling_index<N: SelectorNode>(
    indexed: &IndexedTree<'_, N>,
    node_ref: ComponentRef<'_, N>,
) -> Option<usize> {
    indexed.nodes[node_ref.path_index].1.last().copied()
}

fn sibling_count<N: SelectorNode>(
    indexed: &IndexedTree<'_, N>,
    node_ref: ComponentRef<'_, N>,
) -> Option<usize> {
    let path = &indexed.nodes[node_ref.path_index].1;
    if path.is_empty() {
        return Some(1);
    }
    let parent_path = &path[..path.len() - 1];
    Some(
        indexed
            .nodes
            .iter()
            .filter(|(_, candidate_path)| {
                candidate_path.len() == path.len() && candidate_path.starts_with(parent_path)
            })
            .count(),
    )
}

fn attr_matches<N: SelectorNode>(node: &N, attr: &AttributeSelector) -> bool {
    let Some(value) = node.attr_value(&attr.name) else {
        return false;
    };
    if attr.operator != AttributeOperator::Equal {
        return compare_attr(&value, attr.operator, &attr.value);
    }
    match &value {
        Value::String(text) => text == &attr.value,
        Value::Number(number) => number.to_string() == attr.value,
        Value::Bool(value) => value.to_string() == attr.value,
        Value::Null => attr.value == "null",
        _ => false,
    }
}

fn behavior_matches<N: SelectorNode>(node: &N, behavior: &BehaviorSelector) -> bool {
    match behavior.name.as_str() {
        "ttl" | "cad" | "cadence" => {
            let Some(value) = node.attr_value(behavior_attr_name(&behavior.name)) else {
                return false;
            };
            if let Some(operator) = behavior.operator {
                let Some(expected) = behavior.value.as_deref() else {
                    return false;
                };
                if operator == AttributeOperator::Equal {
                    return attr_equal(&value, expected);
                }
                return compare_attr(&value, operator, expected);
            }
            true
        }
        "ephemeral" => {
            let is_ephemeral = attr_i64(node, "ttl") == Some(1);
            behavior_bool_matches(is_ephemeral, behavior)
        }
        "sticky" => {
            let is_sticky =
                attr_i64(node, "ttl") == Some(1) && attr_i64(node, "cadence") == Some(1);
            behavior_bool_matches(is_sticky, behavior)
        }
        _ => false,
    }
}

fn behavior_bool_matches(actual: bool, behavior: &BehaviorSelector) -> bool {
    match (behavior.operator, behavior.value.as_deref()) {
        (None, None) => actual,
        (Some(AttributeOperator::Equal), Some(expected)) => {
            let expected = matches!(expected.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
            actual == expected
        }
        _ => false,
    }
}

fn behavior_attr_name(name: &str) -> &str {
    if name == "cadence" { "cad" } else { name }
}

fn attr_i64<N: SelectorNode>(node: &N, name: &str) -> Option<i64> {
    node.attr_value(name).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

fn is_behavior_name(name: &str) -> bool {
    matches!(name, "ttl" | "cad" | "cadence" | "ephemeral" | "sticky")
}

fn parse_selector(selector: &str) -> PactResult<Vec<SelectorStep>> {
    let tokens = split_chain(selector)?;
    let mut out = Vec::new();
    let mut pending_combinator = None;
    for token in tokens {
        match token.as_str() {
            ">" => pending_combinator = Some(Combinator::Child),
            "?" => pending_combinator = Some(Combinator::Single),
            "??" | "~" => pending_combinator = Some(Combinator::Recursive { within: None }),
            token if token.starts_with("within(") => {
                let Some(combinator) = pending_combinator.take() else {
                    return Err(PactError::invalid(
                        "within(N) selector scope must follow ? or ??".to_string(),
                    ));
                };
                pending_combinator = Some(combinator.with_within(parse_within(token)?));
            }
            simple => {
                out.push(SelectorStep {
                    combinator: pending_combinator.take(),
                    simple: parse_simple(simple)?,
                });
            }
        }
    }
    if out.first().and_then(|step| step.combinator).is_some() {
        return Err(PactError::invalid(
            "selector cannot start with a combinator".to_string(),
        ));
    }
    if pending_combinator.is_some() {
        return Err(PactError::invalid(
            "selector cannot end with a combinator".to_string(),
        ));
    }
    Ok(out)
}

fn parse_simple(input: &str) -> PactResult<SimpleSelector> {
    let mut rest = input.trim();
    let mut simple = SimpleSelector::default();
    loop {
        rest = rest.trim();
        if rest.is_empty() {
            break;
        }
        if let Some(next) = rest.strip_prefix('*') {
            simple.wildcard = true;
            rest = next;
            continue;
        }
        if rest.starts_with('^') {
            return Err(
                PactError::invalid("deprecated symbolic PACT region selectors are not supported; use depth selectors such as d-1, d0, or d1".to_string())
            );
        }
        if let Some(next) = rest.strip_prefix('.') {
            let (name, remainder) = take_identifier(next);
            if name.is_empty() {
                return Err(PactError::invalid(format!(
                    "invalid type selector in {input:?}"
                )));
            }
            simple.type_name = Some(name.to_string());
            rest = remainder;
            while let Some(next) = rest.strip_prefix(':') {
                let (type_tag, remainder) = take_identifier(next);
                if type_tag.is_empty() {
                    return Err(PactError::invalid(format!(
                        "invalid type tag selector in {input:?}"
                    )));
                }
                if pseudo_takes_type_slot(type_tag, remainder) {
                    let (argument, final_remainder) = if remainder.starts_with('(') {
                        let (inside, after) = take_wrapped(remainder, '(', ')')?;
                        (Some(inside.to_string()), after)
                    } else {
                        (None, remainder)
                    };
                    simple.pseudos.push(PseudoSelector {
                        name: type_tag.to_string(),
                        argument,
                    });
                    rest = final_remainder;
                } else {
                    simple.type_tags.push(type_tag.to_string());
                    rest = remainder;
                }
            }
            continue;
        }
        if let Some(next) = rest.strip_prefix(':') {
            let (name, remainder) = take_identifier(next);
            if name.is_empty() {
                return Err(PactError::invalid(format!(
                    "invalid pseudo selector in {input:?}"
                )));
            }
            let (argument, final_remainder) = if remainder.starts_with('(') {
                let (inside, after) = take_wrapped(remainder, '(', ')')?;
                (Some(inside.to_string()), after)
            } else {
                (None, remainder)
            };
            simple.pseudos.push(PseudoSelector {
                name: name.to_string(),
                argument,
            });
            rest = final_remainder;
            continue;
        }
        if let Some(next) = rest.strip_prefix('#') {
            let (id, remainder) = take_id(next);
            if id.is_empty() {
                return Err(PactError::invalid(format!(
                    "invalid id selector in {input:?}"
                )));
            }
            simple.id = Some(id.to_string());
            rest = remainder;
            continue;
        }
        if let Some(next) = rest.strip_prefix('+') {
            let (tag, remainder) = take_tag(next);
            if tag.is_empty() {
                return Err(PactError::invalid(format!(
                    "invalid tag selector in {input:?}"
                )));
            }
            simple.tags.push(tag.to_string());
            rest = remainder;
            continue;
        }
        if rest.starts_with("((") {
            let (inside, remainder) = take_wrapped(rest, '(', ')')?;
            simple.position = Some(parse_position_set(inside)?);
            rest = remainder;
            continue;
        }
        if rest.starts_with("(d") {
            let (inside, remainder) = take_wrapped(rest, '(', ')')?;
            simple.position = Some(parse_position(inside)?);
            rest = remainder;
            continue;
        }
        if let Some(next) = rest.strip_prefix('d') {
            let (depth, remainder) = take_signed_int(next);
            if depth.is_empty() {
                return Err(PactError::invalid(format!(
                    "invalid depth selector in {input:?}"
                )));
            }
            simple.depth = Some(
                depth
                    .parse::<i32>()
                    .map_err(|_| PactError::invalid(format!("invalid depth selector d{depth}")))?,
            );
            rest = remainder;
            continue;
        }
        if rest.starts_with('{') {
            let (inside, remainder) = take_wrapped(rest, '{', '}')?;
            simple.attrs.extend(parse_attrs(inside)?);
            rest = remainder;
            continue;
        }
        if rest.starts_with('[') {
            let (inside, remainder) = take_wrapped(rest, '[', ']')?;
            let (attrs, behaviors) = parse_bracket_filters(inside)?;
            simple.attrs.extend(attrs);
            simple.behaviors.extend(behaviors);
            rest = remainder;
            continue;
        }
        return Err(PactError::invalid(format!(
            "unsupported selector fragment {rest:?} in {input:?}"
        )));
    }
    Ok(simple)
}

fn parse_position(input: &str) -> PactResult<PositionSelector> {
    let mut parts = input
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let Some(first) = parts.next() else {
        return Err(PactError::invalid("empty position selector".to_string()));
    };
    if let Some((start, end)) = parse_depth_range(first)? {
        let mut tail = Vec::new();
        for part in parts {
            tail.push(parse_position_coordinate(part)?);
        }
        return Ok(PositionSelector::DepthRange { start, end, tail });
    }

    let mut out = Vec::new();
    let Some(depth) = first.strip_prefix('d') else {
        return Err(PactError::invalid(format!(
            "invalid depth coordinate {first:?}"
        )));
    };
    out.push(PositionCoordinate::Offset(depth.parse::<i32>().map_err(
        |_| PactError::invalid(format!("invalid depth coordinate {first:?}")),
    )?));
    for part in parts {
        out.push(parse_position_coordinate(part)?);
    }
    if out.is_empty() {
        return Err(PactError::invalid("empty position selector".to_string()));
    }
    Ok(PositionSelector::Exact(out))
}

fn parse_position_coordinate(input: &str) -> PactResult<PositionCoordinate> {
    let token = input.trim();
    if token == "all" || token == "+all" {
        return Ok(PositionCoordinate::All);
    }
    if token.contains("all") {
        return Err(PactError::invalid(format!(
            "invalid position range coordinate {token:?}"
        )));
    }
    if let Some((start, end)) = token.split_once("..") {
        return Ok(PositionCoordinate::Range(
            parse_position_bound(start)?,
            parse_position_bound(end)?,
        ));
    }
    match parse_position_bound(token)? {
        PositionBound::Offset(offset) => Ok(PositionCoordinate::Offset(offset)),
        bound => Ok(PositionCoordinate::Range(bound.clone(), bound)),
    }
}

fn parse_position_bound(input: &str) -> PactResult<PositionBound> {
    match input.trim() {
        "min" | "first" | "-last" => Ok(PositionBound::Min),
        "max" | "last" | "+last" => Ok(PositionBound::Max),
        raw => raw
            .strip_prefix('+')
            .unwrap_or(raw)
            .parse::<i32>()
            .map(PositionBound::Offset)
            .map_err(|_| PactError::invalid(format!("invalid position coordinate {input:?}"))),
    }
}

fn parse_position_set(input: &str) -> PactResult<PositionSelector> {
    let mut selectors = Vec::new();
    for raw_part in split_top_level(input, ',') {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }
        let inside = if part.starts_with('(') {
            let (inside, remainder) = take_wrapped(part, '(', ')')?;
            if !remainder.trim().is_empty() {
                return Err(PactError::invalid(format!(
                    "unsupported coordinate set fragment {part:?}"
                )));
            }
            inside
        } else {
            part
        };
        selectors.push(parse_position(inside)?);
    }
    if selectors.is_empty() {
        return Err(PactError::invalid("empty coordinate set".to_string()));
    }
    Ok(PositionSelector::Set(selectors))
}

fn parse_depth_range(input: &str) -> PactResult<Option<(i32, Option<i32>)>> {
    let Some((start, end)) = input.split_once("..") else {
        return Ok(None);
    };
    let start = parse_depth_token(start)?;
    let end = if end == "d*" {
        None
    } else {
        Some(parse_depth_token(end)?)
    };
    if let Some(end) = end
        && end < start
    {
        return Err(PactError::invalid(format!(
            "invalid descending depth range {input:?}"
        )));
    }
    Ok(Some((start, end)))
}

fn parse_depth_token(input: &str) -> PactResult<i32> {
    let Some(depth) = input.strip_prefix('d') else {
        return Err(PactError::invalid(format!(
            "invalid depth coordinate {input:?}"
        )));
    };
    depth
        .parse::<i32>()
        .map_err(|_| PactError::invalid(format!("invalid depth coordinate {input:?}")))
}

fn parse_within(input: &str) -> PactResult<usize> {
    let Some(rest) = input.strip_prefix("within(") else {
        return Err(PactError::invalid(format!(
            "invalid within selector scope {input:?}"
        )));
    };
    let Some(number) = rest.strip_suffix(')') else {
        return Err(PactError::invalid(format!(
            "invalid within selector scope {input:?}"
        )));
    };
    let value = number
        .parse::<usize>()
        .map_err(|_| PactError::invalid(format!("invalid within selector scope {input:?}")))?;
    if value == 0 {
        return Err(PactError::invalid(
            "within selector scope must be greater than zero".to_string(),
        ));
    }
    Ok(value)
}

fn parse_bracket_filters(
    input: &str,
) -> PactResult<(Vec<AttributeSelector>, Vec<BehaviorSelector>)> {
    let mut attrs = Vec::new();
    let mut behaviors = Vec::new();
    for raw_part in split_filter_items(input) {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((name, operator, value)) = split_attr_operator(part) {
            let name = name.trim();
            if is_behavior_name(name) {
                behaviors.push(BehaviorSelector {
                    name: name.to_string(),
                    operator: Some(operator),
                    value: Some(unquote(value.trim()).to_string()),
                });
            } else {
                attrs.push(AttributeSelector {
                    name: name.to_string(),
                    operator,
                    value: unquote(value.trim()).to_string(),
                });
            }
        } else if is_behavior_name(part) {
            behaviors.push(BehaviorSelector {
                name: part.to_string(),
                operator: None,
                value: None,
            });
        } else {
            attrs.push(AttributeSelector {
                name: part.to_string(),
                operator: AttributeOperator::Equal,
                value: "true".to_string(),
            });
        }
    }
    Ok((attrs, behaviors))
}

fn pseudo_takes_type_slot(name: &str, remainder: &str) -> bool {
    remainder.starts_with('(')
        || matches!(
            name,
            "first" | "last" | "first-child" | "last-child" | "nth" | "has"
        )
}

fn parse_attrs(input: &str) -> PactResult<Vec<AttributeSelector>> {
    let mut out = Vec::new();
    for raw_part in split_top_level(input, ',') {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, operator, value)) = split_attr_operator(part) else {
            return Err(PactError::invalid(format!(
                "unsupported attribute selector {part:?}"
            )));
        };
        out.push(AttributeSelector {
            name: name.trim().to_string(),
            operator,
            value: unquote(value.trim()).to_string(),
        });
    }
    Ok(out)
}

fn split_attr_operator(part: &str) -> Option<(&str, AttributeOperator, &str)> {
    for (needle, operator) in [
        ("~=", AttributeOperator::Regex),
        ("!=", AttributeOperator::NotEqual),
        ("<=", AttributeOperator::LessThanOrEqual),
        (">=", AttributeOperator::GreaterThanOrEqual),
        ("=", AttributeOperator::Equal),
        ("<", AttributeOperator::LessThan),
        (">", AttributeOperator::GreaterThan),
    ] {
        if let Some((left, right)) = part.split_once(needle) {
            return Some((left, operator, right));
        }
    }
    None
}

fn compare_attr(value: &Value, operator: AttributeOperator, expected: &str) -> bool {
    if operator == AttributeOperator::NotEqual {
        return !attr_equal(value, expected);
    }
    if operator == AttributeOperator::Regex {
        return regex_attr(value, expected);
    }
    let Some(left) = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    else {
        return false;
    };
    let Ok(right) = expected.parse::<f64>() else {
        return false;
    };
    match operator {
        AttributeOperator::Equal => attr_equal(value, expected),
        AttributeOperator::NotEqual => !attr_equal(value, expected),
        AttributeOperator::Regex => regex_attr(value, expected),
        AttributeOperator::LessThan => left < right,
        AttributeOperator::LessThanOrEqual => left <= right,
        AttributeOperator::GreaterThan => left > right,
        AttributeOperator::GreaterThanOrEqual => left >= right,
    }
}

fn regex_attr(value: &Value, expected: &str) -> bool {
    let Ok(pattern) = regex::Regex::new(expected) else {
        return false;
    };
    pattern.is_match(&attr_string(value))
}

fn attr_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn attr_equal(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(text) => text == expected,
        Value::Number(number) => number.to_string() == expected,
        Value::Bool(value) => value.to_string() == expected,
        Value::Null => expected == "null",
        _ => false,
    }
}

fn apply_controls(
    mut matches: Vec<SelectorMatch>,
    controls: &SelectorControls,
) -> PactResult<Vec<SelectorMatch>> {
    if let Some(mode) = controls.mode.as_deref() {
        match mode {
            "all" | "latest" => {}
            _ => {
                return Err(PactError::invalid(format!(
                    "unknown selector mode {mode:?}; expected all or latest"
                )));
            }
        }
    }
    if controls.offset > 0 {
        matches = matches.into_iter().skip(controls.offset).collect();
    }
    if let Some(limit) = controls.limit {
        matches.truncate(limit);
    }
    Ok(matches)
}

fn extract_controls(selector: &str) -> PactResult<(String, SelectorControls)> {
    let mut cleaned = String::with_capacity(selector.len());
    let mut controls = SelectorControls::default();
    let chars: Vec<char> = selector.chars().collect();
    let mut depth = 0i32;
    let mut quote = None;
    let mut index = 0usize;
    while index < chars.len() {
        if let Some(active_quote) = quote {
            cleaned.push(chars[index]);
            if chars[index] == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        match chars[index] {
            '"' | '\'' => {
                quote = Some(chars[index]);
                cleaned.push(chars[index]);
                index += 1;
            }
            '(' | '{' | '[' => {
                depth += 1;
                cleaned.push(chars[index]);
                index += 1;
            }
            ')' | '}' | ']' => {
                depth -= 1;
                cleaned.push(chars[index]);
                index += 1;
            }
            '@' if depth == 0 => {
                let rest: String = chars[index..].iter().collect();
                if let Some(next) = rest.strip_prefix("@limit=") {
                    let (number, consumed) = take_control_number(next);
                    controls.limit = Some(number.parse::<usize>().map_err(|_| {
                        PactError::invalid(format!("invalid @limit value {number:?}"))
                    })?);
                    index += "@limit=".chars().count() + consumed;
                } else if let Some(next) = rest.strip_prefix("@offset=") {
                    let (number, consumed) = take_control_number(next);
                    controls.offset = number.parse::<usize>().map_err(|_| {
                        PactError::invalid(format!("invalid @offset value {number:?}"))
                    })?;
                    index += "@offset=".chars().count() + consumed;
                } else if let Some(next) = rest.strip_prefix("@mode=") {
                    let (mode, remainder) = take_identifier(next);
                    if mode.is_empty() {
                        return Err(PactError::invalid(
                            "empty @mode selector control".to_string(),
                        ));
                    }
                    controls.mode = Some(mode.to_string());
                    index += "@mode=".chars().count() + (next.len() - remainder.len());
                } else {
                    cleaned.push(chars[index]);
                    index += 1;
                }
            }
            ch => {
                cleaned.push(ch);
                index += 1;
            }
        }
        if depth < 0 {
            return Err(PactError::invalid(format!(
                "unbalanced selector {selector:?}"
            )));
        }
    }
    if depth != 0 {
        return Err(PactError::invalid(format!(
            "unbalanced selector {selector:?}"
        )));
    }
    Ok((cleaned.trim().to_string(), controls))
}

fn take_control_number(input: &str) -> (&str, usize) {
    let end = input
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .last()
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    (&input[..end], end)
}

fn split_chain(selector: &str) -> PactResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote = None;
    let chars: Vec<char> = selector.chars().collect();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some(active_quote) = quote {
            if chars[index] == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        match chars[index] {
            '"' | '\'' => quote = Some(chars[index]),
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            '>' | '?' | '~' if depth == 0 => {
                let before = selector[start..index].trim();
                if !before.is_empty() {
                    tokens.push(before.to_string());
                }
                if chars[index] == '?' && chars.get(index + 1) == Some(&'?') {
                    tokens.push("??".to_string());
                    index += 1;
                } else {
                    tokens.push(chars[index].to_string());
                }
                start = index + 1;
            }
            ch if ch.is_whitespace() && depth == 0 => {
                let before = selector[start..index].trim();
                let mut after = index + 1;
                while after < chars.len() && chars[after].is_whitespace() {
                    after += 1;
                }
                let next = chars.get(after).copied();
                if before.starts_with("within(") {
                    tokens.push(before.to_string());
                    start = after;
                } else if !before.is_empty() && next.is_some() && !matches!(next, Some('>' | '?')) {
                    tokens.push(before.to_string());
                    tokens.push("??".to_string());
                    start = after;
                } else if before.is_empty() {
                    start = after;
                }
                index = after.saturating_sub(1);
            }
            _ => {}
        }
        if depth < 0 {
            return Err(PactError::invalid(format!(
                "unbalanced selector {selector:?}"
            )));
        }
        index += 1;
    }
    if depth != 0 {
        return Err(PactError::invalid(format!(
            "unbalanced selector {selector:?}"
        )));
    }
    let tail = selector[start..].trim();
    if !tail.is_empty() {
        tokens.push(tail.to_string());
    }
    Ok(tokens)
}

fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote = None;
    for (index, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            value if value == delimiter && depth == 0 => {
                parts.push(&input[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&input[start..]);
    parts
}

fn split_filter_items(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote = None;
    for (index, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&input[start..index]);
                start = index + ch.len_utf8();
            }
            ch if ch.is_whitespace() && depth == 0 => {
                let part = input[start..index].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

fn take_identifier(input: &str) -> (&str, &str) {
    let end = input
        .char_indices()
        .find(|(_, ch)| !(*ch == '_' || *ch == '-' || ch.is_ascii_alphanumeric()))
        .map(|(index, _)| index)
        .unwrap_or(input.len());
    (&input[..end], &input[end..])
}

fn take_id(input: &str) -> (&str, &str) {
    let (head, rest) = take_identifier(input);
    let Some(after_colon) = rest.strip_prefix(':') else {
        return (head, rest);
    };
    let (local_id, after_local_id) = take_identifier(after_colon);
    if is_scoped_local_component_id(local_id) {
        let end = head.len() + 1 + local_id.len();
        return (&input[..end], after_local_id);
    }
    (head, rest)
}

fn is_scoped_local_component_id(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    !matches!(
        value,
        "has" | "first" | "first-child" | "last" | "last-child" | "nth"
    )
}

fn take_tag(input: &str) -> (&str, &str) {
    let end = input
        .char_indices()
        .find(|(_, ch)| {
            !(*ch == '_' || *ch == '-' || *ch == ':' || *ch == '*' || ch.is_ascii_alphanumeric())
        })
        .map(|(index, _)| index)
        .unwrap_or(input.len());
    (&input[..end], &input[end..])
}

fn take_signed_int(input: &str) -> (&str, &str) {
    let end = input
        .char_indices()
        .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
        .last()
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    (&input[..end], &input[end..])
}

fn take_wrapped(input: &str, open: char, close: char) -> PactResult<(&str, &str)> {
    let mut depth = 0i32;
    let mut quote = None;
    for (index, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Ok((&input[1..index], &input[index + ch.len_utf8()..]));
            }
        }
    }
    Err(PactError::invalid(format!("unclosed {open} in {input:?}")))
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests;
