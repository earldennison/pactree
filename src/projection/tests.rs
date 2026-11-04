use super::*;
use serde_json::json;

fn sample_tree() -> ComponentTree {
    ComponentTree::from_value(&json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "parent",
            "componentType": "message_turn",
            "children": [{
                "id": "leaf",
                "componentType": "text_content",
                "content_ref": {"id": "cnt_1", "preview": "hello"}
            }]
        }, {
            "id": "archived",
            "componentType": "text_content",
            "content": "gone",
            "lifecycle": {"state": "closed", "revision": 2}
        }]
    }))
    .expect("tree")
}

#[derive(Default)]
struct EventLogSink {
    events: Vec<String>,
}

impl ProjectionSink for EventLogSink {
    type Output = Vec<String>;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        match event {
            ComponentEvent::BeginComponent(component) => {
                self.events.push(format!("begin:{}", component.id));
                assert!(component.children.is_empty());
            }
            ComponentEvent::EndComponent { component_id } => {
                self.events.push(format!("end:{component_id}"));
            }
            ComponentEvent::ContentRef { component_id, .. } => {
                self.events.push(format!("content_ref:{component_id}"));
            }
            ComponentEvent::Deleted { component_id } => {
                self.events.push(format!("deleted:{component_id}"));
            }
        }
        Ok(())
    }

    fn finish(self) -> PactResult<Self::Output> {
        Ok(self.events)
    }
}

#[test]
fn component_events_preserve_depth_first_parent_child_order() {
    let events = stream_component_tree(&sample_tree(), EventLogSink::default()).expect("events");
    assert_eq!(
        events,
        vec![
            "begin:root",
            "begin:archived",
            "deleted:archived",
            "end:archived",
            "begin:parent",
            "begin:leaf",
            "content_ref:leaf",
            "end:leaf",
            "end:parent",
            "end:root",
        ]
    );
}

#[test]
fn component_tree_sink_reconstructs_streamed_tree() {
    let tree = sample_tree();
    let streamed = stream_component_tree(&tree, ComponentTreeSink::new()).expect("streamed");
    assert_eq!(streamed, tree);
}

#[test]
fn selector_sink_selects_from_component_event_stream() {
    let selected =
        stream_component_tree(&sample_tree(), SelectorSink::new("#leaf", false)).expect("selected");
    let ids = selected
        .iter()
        .map(|component| component.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["leaf"]);
}

#[test]
fn component_id_hydration_sink_returns_requested_components() {
    let hydrated = stream_component_tree(
        &sample_tree(),
        ComponentIdHydrationSink::new(["leaf".to_string(), "missing".to_string()]),
    )
    .expect("hydrated");
    let ids = hydrated
        .iter()
        .map(|component| component.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["leaf"]);
    assert_eq!(
        hydrated[0]
            .content_ref()
            .map(|content_ref| content_ref.id.as_str()),
        Some("cnt_1")
    );
}

use crate::selector::{SelectedComponent, select_components_tree};

use std::collections::BTreeSet;

pub(crate) struct SelectorSink {
    tree: ComponentTreeSink,
    selector: String,
    include_archived: bool,
}

impl SelectorSink {
    pub(crate) fn new(selector: impl Into<String>, include_archived: bool) -> Self {
        Self {
            tree: ComponentTreeSink::new(),
            selector: selector.into(),
            include_archived,
        }
    }
}

impl ProjectionSink for SelectorSink {
    type Output = Vec<SelectedComponent>;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        self.tree.on_event(event)
    }

    fn finish(self) -> PactResult<Self::Output> {
        let tree = self.tree.finish()?;
        select_components_tree(&tree, &self.selector, self.include_archived)
    }
}

pub(crate) struct ComponentIdHydrationSink {
    tree: ComponentTreeSink,
    component_ids: BTreeSet<String>,
}

impl ComponentIdHydrationSink {
    pub(crate) fn new(component_ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            tree: ComponentTreeSink::new(),
            component_ids: component_ids.into_iter().collect(),
        }
    }
}

impl ProjectionSink for ComponentIdHydrationSink {
    type Output = Vec<Component>;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        self.tree.on_event(event)
    }

    fn finish(self) -> PactResult<Self::Output> {
        let tree = self.tree.finish()?;
        Ok(self
            .component_ids
            .iter()
            .filter_map(|component_id| tree.component(component_id).cloned())
            .collect())
    }
}
