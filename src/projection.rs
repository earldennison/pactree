use crate::components::{Component, LifecycleState, PactContentRef};
use crate::error::{PactError, PactResult};
use crate::provider_thread::{ProviderThreadOptions, ProviderThreadView};

use crate::store::{ContentStore, PactView, build_projection_from_component};
use crate::tree::ComponentTree;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ProjectionScope {
    Head,
    Selector(String),
    ComponentIds(Vec<String>),
    ProviderThread,
    Xml,
    Json,
    FullTree,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum ComponentEvent {
    BeginComponent(Component),
    EndComponent {
        component_id: String,
    },
    ContentRef {
        component_id: String,
        content_ref: PactContentRef,
    },
    Deleted {
        component_id: String,
    },
}

pub(crate) trait ProjectionSink {
    type Output;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()>;
    fn finish(self) -> PactResult<Self::Output>;
}

pub(crate) fn stream_component_tree<S: ProjectionSink>(
    tree: &ComponentTree,
    sink: S,
) -> PactResult<S::Output> {
    stream_component(tree.root(), sink)
}

pub(crate) fn stream_component<S: ProjectionSink>(
    root: &Component,
    mut sink: S,
) -> PactResult<S::Output> {
    emit_component(root, &mut sink)?;
    sink.finish()
}

fn emit_component(component: &Component, sink: &mut impl ProjectionSink) -> PactResult<()> {
    let mut event_component = component.clone();
    event_component.children.clear();
    sink.on_event(ComponentEvent::BeginComponent(event_component))?;

    if let Some(content_ref) = component.content_ref() {
        sink.on_event(ComponentEvent::ContentRef {
            component_id: component.id.clone(),
            content_ref: content_ref.clone(),
        })?;
    }
    if !matches!(component.lifecycle.state, LifecycleState::Active) {
        sink.on_event(ComponentEvent::Deleted {
            component_id: component.id.clone(),
        })?;
    }

    for child in &component.children {
        emit_component(child, sink)?;
    }
    sink.on_event(ComponentEvent::EndComponent {
        component_id: component.id.clone(),
    })
}

#[derive(Default)]
pub(crate) struct ComponentTreeSink {
    stack: Vec<Component>,
    root: Option<Component>,
}

impl ComponentTreeSink {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl ProjectionSink for ComponentTreeSink {
    type Output = ComponentTree;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        match event {
            ComponentEvent::BeginComponent(component) => {
                self.stack.push(component);
                Ok(())
            }
            ComponentEvent::EndComponent { component_id } => {
                let component = self.stack.pop().ok_or_else(|| {
                    PactError::invalid(format!(
                        "projection stream ended component {component_id:?} with an empty stack"
                    ))
                })?;
                if component.id != component_id {
                    return Err(PactError::invalid(format!(
                        "projection stream ended component {component_id:?} while {:?} was open",
                        component.id
                    )));
                }
                if let Some(parent) = self.stack.last_mut() {
                    parent.children.push(component);
                } else if self.root.replace(component).is_some() {
                    return Err(PactError::invalid(
                        "projection stream produced more than one root component",
                    ));
                }
                Ok(())
            }
            ComponentEvent::ContentRef { .. } | ComponentEvent::Deleted { .. } => Ok(()),
        }
    }

    fn finish(self) -> PactResult<Self::Output> {
        if !self.stack.is_empty() {
            return Err(PactError::invalid(
                "projection stream ended with unclosed components",
            ));
        }
        let root = self
            .root
            .ok_or_else(|| PactError::invalid("projection stream produced no root component"))?;
        ComponentTree::from_root(root)
    }
}

pub(crate) struct PactViewSink {
    branch_id: String,
    head_revision: u64,
    first_seen: BTreeMap<String, u64>,
    blob_inline_limit: usize,
    tree: ComponentTreeSink,
}

impl PactViewSink {
    pub(crate) fn new(
        branch_id: String,
        head_revision: u64,
        first_seen: BTreeMap<String, u64>,
        blob_inline_limit: usize,
    ) -> Self {
        Self {
            branch_id,
            head_revision,
            first_seen,
            blob_inline_limit,
            tree: ComponentTreeSink::new(),
        }
    }
}

impl ProjectionSink for PactViewSink {
    type Output = PactView;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        self.tree.on_event(event)
    }

    fn finish(self) -> PactResult<Self::Output> {
        let tree = self.tree.finish()?;
        let projection =
            build_projection_from_component(tree.root(), &self.first_seen, self.blob_inline_limit)?;
        Ok(PactView::from_projection(
            projection,
            self.branch_id,
            self.head_revision,
        ))
    }
}

pub(crate) struct JsonSink {
    tree: ComponentTreeSink,
}

impl JsonSink {
    pub(crate) fn new() -> Self {
        Self {
            tree: ComponentTreeSink::new(),
        }
    }
}

impl ProjectionSink for JsonSink {
    type Output = String;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        self.tree.on_event(event)
    }

    fn finish(self) -> PactResult<Self::Output> {
        let tree = self.tree.finish()?;
        serde_json::to_string(&tree.project_value())
            .map_err(|err| PactError::codec(format!("failed to encode projected JSON: {err}")))
    }
}

pub(crate) struct XmlSink {
    tree: ComponentTreeSink,
}

impl XmlSink {
    pub(crate) fn new() -> Self {
        Self {
            tree: ComponentTreeSink::new(),
        }
    }
}

impl ProjectionSink for XmlSink {
    type Output = String;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        self.tree.on_event(event)
    }

    fn finish(self) -> PactResult<Self::Output> {
        let tree = self.tree.finish()?;
        Ok(crate::render::render_tree_xml(&tree))
    }
}

pub(crate) struct ProviderThreadSink<'a> {
    tree: ComponentTreeSink,
    content_store: &'a ContentStore,
    options: ProviderThreadOptions,
}

impl<'a> ProviderThreadSink<'a> {
    pub(crate) fn new(content_store: &'a ContentStore, options: ProviderThreadOptions) -> Self {
        Self {
            tree: ComponentTreeSink::new(),
            content_store,
            options,
        }
    }
}

impl ProjectionSink for ProviderThreadSink<'_> {
    type Output = ProviderThreadView;

    fn on_event(&mut self, event: ComponentEvent) -> PactResult<()> {
        self.tree.on_event(event)
    }

    fn finish(self) -> PactResult<Self::Output> {
        let tree = self.tree.finish()?;
        crate::provider_thread::build_provider_thread_tree_with_content_store(
            &tree,
            self.content_store,
            &self.options,
        )
        .map_err(|err| PactError::resolver(err.to_string()))
    }
}

#[cfg(test)]
mod tests;
