//! Public working-view facade for Pact context trees.
//!
//! `ContextTree` is the runtime object callers use to inspect, mutate,
//! preview, send, receive, and discard staged Pact work. It deliberately wraps
//! resolver/ledger machinery instead of exposing those substrate handles.
//!
//! ```no_run
//! use pactree::{Component, ContextTree, Placement, ProviderThreadOptions};
//! use serde_json::json;
//!
//! # fn main() -> pactree::PactResult<()> {
//! let mut tree = ContextTree::new()?;
//! // `from_component_value` is an import/deserialization edge. `insert` mints
//! // the authoritative scoped component id when the component enters the tree.
//! let component = Component::from_component_value(&json!({
//!     "id": "import_placeholder",
//!     "componentType": "text_content",
//!     "content": "Be concise."
//! }))?;
//!
//! tree.insert(component, Placement::from("root"))?;
//! let preview = tree.view()?;
//! let provider_thread = tree.send(&ProviderThreadOptions::default())?;
//! # let _ = (preview, provider_thread);
//! # Ok(())
//! # }
//! ```

use crate::components::{AuthoringComponentOptions, Component, PactAttrs};
use crate::error::{PactError, PactResult};
use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
use crate::projection::{
    JsonSink, PactViewSink, ProviderThreadSink, XmlSink, stream_component_tree,
};
use crate::provider_thread::{ProviderThreadOptions, ProviderThreadView};
use crate::resolver::PactResolver;
use crate::selector::SelectedComponent;
use crate::store::{PactRef, PactTransactionTrace, PactView};
use crate::trace::{
    BoundaryKind, CommitRef, ResolveOutcome, ResolvedOperationTrace, StagedTraceTarget,
};
use crate::transaction_trace::TraceOpKind;
use crate::tree::ComponentTree;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Current,
    Head,

    Archive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectionRequest {
    View { mode: ViewMode },
    Json { mode: ViewMode },
    ProviderThread { options: ProviderThreadOptions },
    Xml { mode: ViewMode },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ProjectionOutput {
    View(PactView),
    Json(String),
    ProviderThread(ProviderThreadView),
    Xml(String),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ComponentChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aux: PactAttrs,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContextTreeOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deterministic_authoring: Option<DeterministicAuthoringOptions>,

    #[serde(default)]
    pub derived_depth: bool,
}

impl ContextTreeOptions {
    fn validate(&self) -> PactResult<()> {
        if let Some(options) = &self.deterministic_authoring {
            options.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeterministicAuthoringOptions {
    local_id_prefix: String,
    start_timestamp_ns: u64,
    timestamp_step_ns: u64,
}

impl DeterministicAuthoringOptions {
    pub fn new(
        local_id_prefix: impl Into<String>,
        start_timestamp_ns: u64,
        timestamp_step_ns: u64,
    ) -> PactResult<Self> {
        let options = Self {
            local_id_prefix: local_id_prefix.into(),
            start_timestamp_ns,
            timestamp_step_ns,
        };
        options.validate()?;
        Ok(options)
    }

    pub(crate) fn validate(&self) -> PactResult<()> {
        if self.timestamp_step_ns == 0 {
            return Err(PactError::invalid(
                "deterministic authoring timestamp step must be >= 1",
            ));
        }
        Ok(())
    }

    pub fn local_id_prefix(&self) -> &str {
        &self.local_id_prefix
    }

    pub fn start_timestamp_ns(&self) -> u64 {
        self.start_timestamp_ns
    }

    pub fn timestamp_step_ns(&self) -> u64 {
        self.timestamp_step_ns
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TraceRef {
    pub trace_id: String,
    pub op_kind: String,
    pub created_at_ns: u64,
    #[serde(default)]
    pub produced_component_ids: Vec<String>,
    #[serde(default)]
    pub replaced_component_ids: Vec<String>,
    #[serde(default)]
    pub deleted_component_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PlacementTarget {
    Root,
    Depth(i32),
    ComponentId(String),
    Selector(String),
}

#[allow(clippy::derivable_impls)]
impl Default for PlacementTarget {
    fn default() -> Self {
        Self::Root
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Placement {
    pub target: PlacementTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
}

impl Placement {
    fn into_insert_parts(
        self,
        root_id: &str,
    ) -> (Option<String>, Option<String>, Option<i32>, Option<i32>) {
        match self.target {
            PlacementTarget::Root => (None, Some(root_id.to_string()), None, self.offset),
            PlacementTarget::Depth(depth) => (None, None, Some(depth), self.offset),
            PlacementTarget::ComponentId(component_id) => {
                (None, Some(component_id), None, self.offset)
            }
            PlacementTarget::Selector(selector) => (Some(selector), None, None, self.offset),
        }
    }

    fn into_append_parts(self, root_id: &str) -> (Option<String>, Option<String>, Option<i32>) {
        match self.target {
            PlacementTarget::Root => (Some(root_id.to_string()), None, self.offset),
            PlacementTarget::Depth(depth) => (None, Some(format!("d{depth}")), self.offset),
            PlacementTarget::ComponentId(component_id) => (Some(component_id), None, self.offset),
            PlacementTarget::Selector(selector) => (None, Some(selector), self.offset),
        }
    }
}

impl From<(i32, i32)> for Placement {
    fn from((depth, offset): (i32, i32)) -> Self {
        Self {
            target: PlacementTarget::Depth(depth),
            offset: Some(offset),
        }
    }
}

impl From<&str> for Placement {
    fn from(value: &str) -> Self {
        Self {
            target: placement_target_from_str(value),
            offset: None,
        }
    }
}

impl From<String> for Placement {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&String> for Placement {
    fn from(value: &String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<(&str, i32)> for Placement {
    fn from((value, offset): (&str, i32)) -> Self {
        Self {
            target: placement_target_from_str(value),
            offset: Some(offset),
        }
    }
}

impl From<(String, i32)> for Placement {
    fn from((value, offset): (String, i32)) -> Self {
        Self::from((value.as_str(), offset))
    }
}

fn placement_target_from_str(value: &str) -> PlacementTarget {
    let value = value.trim();
    if value.is_empty() || value == "root" {
        return PlacementTarget::Root;
    }
    if let Some(depth) = value
        .strip_prefix('d')
        .and_then(|depth| depth.parse::<i32>().ok())
    {
        return PlacementTarget::Depth(depth);
    }
    if value.starts_with('.')
        || value.starts_with('#')
        || value.starts_with('{')
        || value.starts_with('[')
        || value.starts_with('+')
        || value.starts_with('*')
        || value.contains(char::is_whitespace)
        || value.contains(',')
    {
        return PlacementTarget::Selector(value.to_string());
    }
    PlacementTarget::ComponentId(value.to_string())
}

pub type ReceiveReport = Value;

pub struct ContextTree {
    resolver: PactResolver,
    context_tree_id: String,
    branch_id: String,
    composed_tree_cache: Mutex<ComposedTreeCache>,

    full_history_cache: Mutex<Option<std::sync::Arc<crate::store::ContextTreeLedger>>>,
    reference_fork: Option<ReferenceFork>,
}

struct ReferenceFork {
    parent_branch_id: String,
    parent_revision: u64,
    virtual_head: PactView,
}

#[derive(Default)]
struct ComposedTreeCache {
    current: Option<ComponentTree>,
}

impl ComposedTreeCache {
    fn clear(&mut self) {
        self.current = None;
    }
}

impl ContextTree {
    pub fn new() -> PactResult<Self> {
        Self::with_id(mint_context_tree_id())
    }

    pub fn new_with_options(options: ContextTreeOptions) -> PactResult<Self> {
        Self::with_id_and_options(mint_context_tree_id(), options)
    }

    pub fn with_id(context_tree_id: impl Into<String>) -> PactResult<Self> {
        Self::with_id_and_options(context_tree_id, ContextTreeOptions::default())
    }

    pub fn with_id_and_options(
        context_tree_id: impl Into<String>,
        options: ContextTreeOptions,
    ) -> PactResult<Self> {
        options.validate()?;
        let context_tree_id = context_tree_id.into();
        let mut resolver = PactResolver::new(&context_tree_id, Some("main"))?;
        resolver.set_deterministic_authoring(options.deterministic_authoring)?;
        if options.derived_depth {
            resolver.mark_derived_depth("main");
        }
        Ok(Self {
            resolver,
            context_tree_id,
            branch_id: "main".to_string(),
            composed_tree_cache: Mutex::default(),
            full_history_cache: Mutex::default(),
            reference_fork: None,
        })
    }

    pub fn open_file(path: impl Into<PathBuf>, branch_id: Option<&str>) -> PactResult<Self> {
        Self::open_file_with_options(path, branch_id, ContextTreeOptions::default())
    }

    pub fn open_file_with_options(
        path: impl Into<PathBuf>,
        branch_id: Option<&str>,
        options: ContextTreeOptions,
    ) -> PactResult<Self> {
        options.validate()?;
        let mut resolver = PactResolver::open_file(path.into())?;
        resolver.set_deterministic_authoring(options.deterministic_authoring)?;
        let context_tree_id = resolver.ledger().root_id.clone();
        let branch_id = branch_id
            .map(str::to_string)
            .unwrap_or_else(|| resolver.ledger().current_branch_id.clone());
        if options.derived_depth {
            resolver.mark_derived_depth(&branch_id);
        }
        Ok(Self {
            resolver,
            context_tree_id,
            branch_id,
            composed_tree_cache: Mutex::default(),
            full_history_cache: Mutex::default(),
            reference_fork: None,
        })
    }

    pub(crate) fn from_file_snapshot(
        ledger: std::sync::Arc<crate::store::ContextTreeLedger>,
        path: PathBuf,
        branch_id: &str,
    ) -> PactResult<Self> {
        if !ledger.branches.contains_key(branch_id) {
            return Err(PactError::not_found(format!(
                "branch not found: {branch_id}"
            )));
        }
        Ok(Self {
            context_tree_id: ledger.root_id.clone(),
            branch_id: branch_id.to_string(),
            resolver: PactResolver::from_shared_file_ledger(ledger, path),
            composed_tree_cache: Mutex::default(),
            full_history_cache: Mutex::default(),
            reference_fork: None,
        })
    }

    pub(crate) fn set_automatic_compaction(&mut self, enabled: bool) {
        self.resolver.set_automatic_compaction(enabled);
    }

    #[cfg(test)]
    pub(crate) fn set_deterministic_authoring(
        &mut self,
        options: Option<DeterministicAuthoringOptions>,
    ) -> PactResult<()> {
        self.resolver.set_deterministic_authoring(options)
    }

    #[cfg(test)]
    pub(crate) fn resolver_ledger_is_lazy(&self) -> bool {
        self.resolver.ledger().is_lazy()
    }

    #[cfg(test)]
    pub(crate) fn resolver_trace_count(&self) -> usize {
        self.resolver.ledger().trace_count()
    }

    pub fn next_authoring_component_options(&mut self) -> AuthoringComponentOptions {
        self.resolver.next_authoring_component_options()
    }

    pub fn bind_file(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        let branch_id = self.branch_id.clone();
        match std::fs::metadata(&path) {
            Ok(_) => {
                let _ = self.resolver.attach_file(path, &branch_id);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.resolver.bind_file(path);
            }
            Err(_) => {}
        }
    }

    pub fn save_file(&mut self, path: impl Into<PathBuf>) -> PactResult<()> {
        if self.resolver.ledger().is_lazy() {
            let full = self.full_ledger()?;
            self.resolver.replace_ledger(full.as_ref().clone());
        }
        let path = path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| {
                PactError::storage(format!("save_file could not create {parent:?}: {err}"))
            })?;
        }
        let branch_id = self.branch_id.clone();
        match std::fs::read(&path) {
            Ok(_) => self.resolver.attach_file(path, &branch_id),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let bytes = self.resolver.encode_new_file_baseline()?;
                match crate::atomic_output::publish_bytes(
                    &path,
                    &bytes,
                    crate::atomic_output::PublishMode::CreateNew,
                ) {
                    Ok(()) => {
                        self.resolver.bind_created_file(path);
                        Ok(())
                    }
                    Err(PactError::Io(error))
                        if error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        self.resolver.attach_file(path, &branch_id)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(PactError::Io(error)),
        }
    }

    pub fn fork_reference(&self) -> PactResult<Self> {
        let parent_branch_id = self.effective_branch_id().to_string();
        let parent = self
            .resolver
            .ledger()
            .branches
            .get(&parent_branch_id)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {parent_branch_id}")))?;
        let parent_revision = parent.head_revision;
        let branch_id = crate::store::next_branch_id(
            self.resolver.ledger(),
            &parent_branch_id,
            parent_revision,
        );
        let virtual_head = PactView {
            tree: crate::store::ContextTreeView {
                root_id: parent.current.tree.root_id.clone(),
                stats: crate::store::PactStoreStats {
                    node_count: 0,
                    edge_count: 0,
                    blob_count: 0,
                    content_chars: 0,
                },
                root: parent.current.tree.root.clone_without_children(),
                index: crate::store::ComponentViewIndex::default(),
                first_seen: BTreeMap::new(),
                tool_pairs: BTreeMap::new(),
            },
            branch_id: branch_id.clone(),
            head_revision: 0,
            lifecycle_seen: BTreeMap::new(),
            message_order: Vec::new(),
            branch_refs: Vec::new(),
            content_refs: BTreeMap::new(),
        };
        let mut resolver = PactResolver::from_shared_ledger(self.resolver.ledger.clone());
        resolver.blob_inline_limit = self.resolver.blob_inline_limit;
        resolver.copy_authoring_source_from(&self.resolver);
        Ok(Self {
            resolver,
            context_tree_id: self.context_tree_id.clone(),
            branch_id,
            composed_tree_cache: Mutex::default(),
            full_history_cache: Mutex::default(),
            reference_fork: Some(ReferenceFork {
                parent_branch_id,
                parent_revision,
                virtual_head,
            }),
        })
    }

    fn effective_branch_id(&self) -> &str {
        self.reference_fork
            .as_ref()
            .map_or(&self.branch_id, |reference| &reference.parent_branch_id)
    }

    fn activate_reference_fork(&mut self) -> PactResult<()> {
        let Some(reference) = self.reference_fork.take() else {
            return Ok(());
        };
        let ledger = self.resolver.ledger().clone();
        match ledger.fork_reference_at(
            &reference.parent_branch_id,
            Some(reference.parent_revision),
            Some(self.branch_id.clone()),
            BTreeMap::new(),
        ) {
            Ok((forked, _)) => {
                self.resolver.replace_ledger(forked);
                self.invalidate_composed_tree_cache();
                Ok(())
            }
            Err(error) => {
                self.reference_fork = Some(reference);
                Err(error)
            }
        }
    }

    #[cfg(test)]
    fn shares_ledger_allocation_with(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.resolver.ledger, &other.resolver.ledger)
    }

    #[cfg(test)]
    fn reference_fork_is_active(&self) -> bool {
        self.reference_fork.is_none()
    }

    pub fn branch_id(&self) -> &str {
        &self.branch_id
    }

    pub fn id(&self) -> &str {
        &self.context_tree_id
    }

    pub fn context_tree_id(&self) -> &str {
        &self.context_tree_id
    }

    pub fn head(&self) -> PactResult<&PactView> {
        if let Some(reference) = &self.reference_fork {
            return Ok(&reference.virtual_head);
        }
        let branch_id = self.effective_branch_id();
        self.resolver
            .ledger()
            .branches
            .get(branch_id)
            .map(|branch| &branch.current)
            .ok_or_else(|| PactError::not_found(format!("branch not found: {branch_id}")))
    }

    pub fn view(&self) -> PactResult<PactView> {
        self.preview()
    }

    pub fn preview(&self) -> PactResult<PactView> {
        if let Some(reference) = &self.reference_fork {
            return Ok(reference.virtual_head.clone());
        }
        let mut view = self.resolver.preview_view(self.effective_branch_id())?;
        view.branch_id = self.branch_id.clone();
        Ok(view)
    }

    pub fn project(&self, request: ProjectionRequest) -> PactResult<ProjectionOutput> {
        match request {
            ProjectionRequest::View { mode } => {
                let (tree, head_revision, first_seen) = self.tree_for_view(mode)?;
                Ok(ProjectionOutput::View(stream_component_tree(
                    &tree,
                    PactViewSink::new(
                        self.branch_id.clone(),
                        head_revision,
                        first_seen,
                        crate::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
                    ),
                )?))
            }
            ProjectionRequest::Json { mode } => {
                let tree = self.tree_for(mode)?;
                Ok(ProjectionOutput::Json(stream_component_tree(
                    &tree,
                    JsonSink::new(),
                )?))
            }
            ProjectionRequest::ProviderThread { options } => {
                let tree = self.tree_for(ViewMode::Current)?;
                let thread = stream_component_tree(
                    &tree,
                    ProviderThreadSink::new(&self.resolver.ledger().content, options),
                )?;
                Ok(ProjectionOutput::ProviderThread(thread))
            }
            ProjectionRequest::Xml { mode } => {
                let tree = self.tree_for(mode)?;
                Ok(ProjectionOutput::Xml(stream_component_tree(
                    &tree,
                    XmlSink::new(),
                )?))
            }
        }
    }

    pub fn project_at_revision(
        &self,
        revision: u64,
    ) -> PactResult<crate::file_format::PactFileProjection> {
        if self.resolver.ledger().base.is_some() {
            return Err(PactError::Unsupported(
                "at-revision projection not supported on overlay handles yet".to_string(),
            ));
        }

        let full = self.full_ledger()?;
        let ledger = full.as_ref();
        let effective_branch_id = self.effective_branch_id();
        let branch = ledger.branches.get(effective_branch_id).ok_or_else(|| {
            PactError::not_found(format!("branch not found: {effective_branch_id}"))
        })?;
        if revision > branch.head_revision {
            return Err(PactError::invalid(format!(
                "requested revision {revision} is after branch {} head {}",
                self.branch_id, branch.head_revision
            )));
        }

        let branch_projection = ledger.project_branch_with_scope(
            effective_branch_id,
            Some(revision),
            self.resolver.blob_inline_limit,
            crate::store::ProjectionTreeScope::Head,
        )?;
        Ok(crate::file_format::PactFileProjection {
            schema: crate::file_format::PACT_FILE_PROJECTION_SCHEMA.to_string(),
            branch_id: self.branch_id.clone(),
            revision: branch_projection.head_revision,
            scope: crate::store::ProjectionTreeScope::Head.as_str().to_string(),
            filters: crate::file_format::PactFileProjectionFilters::default(),
            branch: branch_projection,
            range: None,
            view_record: None,
            source_branch_id: None,
            projection_branch_id: None,
            projection_revision: None,
        })
    }

    pub fn project_component(&self, component_id: &str) -> PactResult<Value> {
        let tree = self.tree_for(ViewMode::Current)?;
        let resolved_id = if tree.contains(component_id) {
            component_id.to_string()
        } else {
            self.resolve_target_id(component_id)
        };
        let component = tree
            .component(&resolved_id)
            .ok_or_else(|| PactError::not_found(format!("component {component_id} not found")))?;
        Ok(component.project_component_value())
    }

    pub fn content_payload(&self, content_id: &str) -> PactResult<Value> {
        let content = self
            .resolver
            .ledger()
            .content
            .records
            .get(content_id)
            .ok_or_else(|| PactError::not_found(format!("content {content_id} not found")))?;
        let text = if let Some(inline) = content.storage.inline.as_ref() {
            inline.clone()
        } else if let Some(bytes) = content.storage.inline_bytes.as_ref() {
            String::from_utf8(bytes.clone()).map_err(|err| {
                PactError::codec(format!("content {content_id} is not UTF-8: {err}"))
            })?
        } else {
            return Err(PactError::not_found(format!(
                "content {content_id} has no projectable chunks"
            )));
        };

        Ok(json!({
            "schema": format!("{}.content_payload.v1", crate::file_format::PACT_FILE_SCHEMA),
            "content_id": content.id,
            "kind": content.kind,
            "hash": content.hash,
            "bytes": text.len(),
            "chars": text.chars().count(),
            "text": text,
        }))
    }

    pub fn traces(&self) -> PactResult<Value> {
        let ledger = self.full_ledger()?;
        let traces = ledger
            .event_index
            .transaction_traces
            .values()
            .filter(|trace| trace.branch_id == self.branch_id)
            .collect::<Vec<_>>();
        serde_json::to_value(traces)
            .map_err(|err| PactError::codec(format!("failed to encode transaction traces: {err}")))
    }

    fn full_ledger(&self) -> PactResult<std::sync::Arc<crate::store::ContextTreeLedger>> {
        let current = self.resolver.shared_ledger();
        if !current.is_lazy() {
            return Ok(current);
        }
        let Some(path) = self.resolver.file_path() else {
            return Err(PactError::storage(
                "head-only ledger is not bound to a file".to_string(),
            ));
        };
        let head_of = |ledger: &crate::store::ContextTreeLedger| {
            ledger
                .branches
                .get(&ledger.current_branch_id)
                .map(|branch| branch.head_revision)
        };
        let mut cache = self
            .full_history_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.as_ref()
            && head_of(cached) == head_of(&current)
        {
            return Ok(std::sync::Arc::clone(cached));
        }
        #[cfg(test)]
        crate::tests::instrumentation::FULL_HISTORY_LOADS.with(|count| count.set(count.get() + 1));
        let full = crate::pact_file::open_shared_ledger(path)?;
        *cache = Some(std::sync::Arc::clone(&full));
        Ok(full)
    }

    pub fn rewind_targets(&self) -> PactResult<Value> {
        let view = match self.project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })? {
            ProjectionOutput::View(view) => view,
            other => {
                return Err(PactError::resolver(format!(
                    "current view projection returned unexpected output: {other:?}"
                )));
            }
        };
        let full = self.full_ledger()?;
        let ledger = full.as_ref();
        let traces = ledger
            .event_index
            .transaction_traces
            .values()
            .filter(|trace| trace.branch_id == self.branch_id)
            .collect::<Vec<_>>();
        let messages = view
            .message_order
            .iter()
            .filter_map(|message_id| {
                let message = view.component(message_id)?;
                Some((message_id.clone(), message.role.clone()))
            })
            .collect::<Vec<_>>();
        Ok(build_rewind_targets_value(
            &ledger.root_id,
            &self.branch_id,
            view.head_revision,
            messages,
            traces,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stream(
        &self,
        mode: &str,
        anchor_row_id: Option<&str>,
        anchor_seq: Option<u64>,
        direction: &str,
        limit: usize,
        path_display: Option<&str>,
    ) -> PactResult<Value> {
        const READ_HANDLE_CURRENT_MODE: &str = "__pactree_read_handle_current_surface";
        let read_handle_current = mode == READ_HANDLE_CURRENT_MODE;
        let normalized_mode = if read_handle_current {
            "current_surface"
        } else {
            crate::file_format::normalize_stream_mode(mode)?
        };
        let view = match self.project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })? {
            ProjectionOutput::View(view) => view,
            other => {
                return Err(PactError::resolver(format!(
                    "current view projection returned unexpected output: {other:?}"
                )));
            }
        };
        let mut rows = if read_handle_current {
            crate::file_format::stream_rows_from_held_pact_view(&view, view.head_revision)
        } else {
            crate::file_format::stream_rows_from_pact_view(&view, view.head_revision)
        };
        if normalized_mode == "activity_surface" {
            let traces = self.traces()?;
            let traces = traces.as_array().cloned().unwrap_or_default();
            rows.extend(crate::file_format::stream_rows_from_activity_entries(
                &crate::file_format::activity_entries_from_traces(&traces),
            ));
        }
        let effective_path = path_display.map(str::to_string).or_else(|| {
            self.resolver
                .file_path
                .as_ref()
                .map(|path| path.display().to_string())
        });
        let head = self.stream_head(&view, effective_path.as_deref());
        Ok(crate::file_format::stream_window_value(
            rows,
            normalized_mode,
            "held_tree",
            false,
            head,
            effective_path.as_deref(),
            anchor_row_id,
            anchor_seq,
            direction,
            limit,
        ))
    }

    pub fn insert<P: Into<Placement>>(
        &mut self,
        component: Component,
        placement: P,
    ) -> PactResult<TraceRef> {
        let component = self.author_component(component)?;
        let (target_selector, parent_id, depth, offset) = placement
            .into()
            .into_insert_parts(&self.resolver.ledger().root_id);
        let op = PactOperation::Insert {
            envelope: host_operation_envelope(&self.branch_id),
            target_selector,
            parent_id,
            depth,
            offset,
            component,
        };
        self.resolve_op(op)
    }

    pub fn append<P: Into<Placement>>(
        &mut self,
        component: Component,
        placement: P,
    ) -> PactResult<TraceRef> {
        let component = self.author_component(component)?;
        let (target_component_id, target_selector, offset) = placement
            .into()
            .into_append_parts(&self.resolver.ledger().root_id);
        let op = PactOperation::Append {
            envelope: host_operation_envelope(&self.branch_id),
            target_component_id,
            target_selector,
            offset,
            component,
        };
        self.resolve_op(op)
    }

    pub fn update(
        &mut self,
        component_id: &str,
        changes: ComponentChanges,
    ) -> PactResult<TraceRef> {
        let op = PactOperation::Update {
            envelope: host_operation_envelope(&self.branch_id),
            target_component_id: Some(self.resolve_target_id(component_id)),
            target_selector: None,
            content: changes.content,
            content_ref: changes.content_ref,
            fields: changes.fields,
            aux: changes.aux,
        };
        self.resolve_op(op)
    }

    pub fn replace(&mut self, component: Component, component_id: &str) -> PactResult<TraceRef> {
        let op = PactOperation::Replace {
            envelope: host_operation_envelope(&self.branch_id),
            target_component_id: Some(self.resolve_target_id(component_id)),
            target_selector: None,
            component,
        };
        self.resolve_op(op)
    }

    pub fn upsert<P: Into<Placement>>(
        &mut self,
        component: Component,
        component_id: Option<&str>,
        placement: P,
    ) -> PactResult<TraceRef> {
        let component = self.author_component(component)?;
        let (target_selector, parent_id, depth, offset) = placement
            .into()
            .into_insert_parts(&self.resolver.ledger().root_id);
        let op = PactOperation::Upsert {
            envelope: host_operation_envelope(&self.branch_id),
            target_component_id: component_id.map(|id| self.resolve_target_id(id)),
            target_selector,
            parent_id,
            depth,
            offset,
            component,
        };
        self.resolve_op(op)
    }

    pub fn prune(&mut self, target: impl Into<Placement>) -> PactResult<TraceRef> {
        let (target_component_id, target_selector) = match target.into().target {
            PlacementTarget::Root => {
                return Err(PactError::invalid(
                    "prune requires a component target (id or selector), not root",
                ));
            }
            PlacementTarget::Depth(depth) => (None, Some(format!("d{depth}"))),
            PlacementTarget::ComponentId(component_id) => {
                (Some(self.resolve_target_id(&component_id)), None)
            }
            PlacementTarget::Selector(selector) => (None, Some(selector)),
        };
        let op = PactOperation::Delete {
            envelope: host_operation_envelope(&self.branch_id),
            target_component_id,
            target_selector,
        };
        self.resolve_op(op)
    }

    pub fn move_component(
        &mut self,
        target: impl Into<Placement>,
        destination: impl Into<Placement>,
        mode: Option<&str>,
        offset: Option<i32>,
    ) -> PactResult<TraceRef> {
        let (target_component_id, target_selector) = match target.into().target {
            PlacementTarget::Root => {
                return Err(PactError::invalid(
                    "move requires a component target (id or selector), not root",
                ));
            }
            PlacementTarget::Depth(depth) => (None, Some(format!("d{depth}"))),
            PlacementTarget::ComponentId(component_id) => {
                (Some(self.resolve_target_id(&component_id)), None)
            }
            PlacementTarget::Selector(selector) => (None, Some(selector)),
        };

        let destination_selector = match destination.into().target {
            PlacementTarget::Root => "root".to_string(),
            PlacementTarget::Depth(depth) => format!("d{depth}"),
            PlacementTarget::ComponentId(component_id) => {
                format!("#{}", self.resolve_target_id(&component_id))
            }
            PlacementTarget::Selector(selector) => selector,
        };
        let op = PactOperation::Move {
            envelope: host_operation_envelope(&self.branch_id),
            target_component_id,
            target_selector,
            destination_selector,
            offset,
            mode: mode.map(str::to_string),
        };
        self.resolve_op(op)
    }

    fn resolve_target_id(&self, component_id: &str) -> String {
        if component_id.contains(':') {
            component_id.to_string()
        } else {
            format!("{}:{component_id}", self.context_tree_id)
        }
    }

    pub fn select(
        &self,
        selector: &str,
        include_archived: bool,
    ) -> PactResult<Vec<SelectedComponent>> {
        let mut selected =
            self.resolver
                .select(self.effective_branch_id(), selector, include_archived)?;

        let content = &self.resolver.ledger().content;
        for item in &mut selected {
            if item.content.as_deref().is_none_or(str::is_empty)
                && let Some(ref content_ref) = item.content_ref
                && let Some(record) = content_ref
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| content.records.get(id))
                && let Some(ref inline) = record.storage.inline
            {
                item.content = Some(inline.clone());
            }
        }
        Ok(selected)
    }

    pub fn staged_traces(&self) -> PactResult<Vec<ResolvedOperationTrace>> {
        Ok(self.resolver.staged_traces(&self.branch_id))
    }

    pub fn discard_staged(&mut self) -> PactResult<bool> {
        self.invalidate_composed_tree_cache();
        Ok(self.resolver.discard_staged(&self.branch_id))
    }

    pub fn discard_trace(&mut self, trace_id: &str) -> PactResult<bool> {
        self.invalidate_composed_tree_cache();
        self.resolver
            .discard_staged_trace(&self.branch_id, trace_id)
    }

    pub fn discard_traces(&mut self, target: StagedTraceTarget) -> PactResult<usize> {
        self.invalidate_composed_tree_cache();
        self.resolver.discard_staged_traces(&self.branch_id, target)
    }

    pub fn has_staged(&self) -> bool {
        self.resolver.has_staged(&self.branch_id)
    }

    pub fn stage_extensions(&mut self, records: Vec<crate::PactExtensionRecord>) -> PactResult<()> {
        self.resolver.stage_extensions(&self.branch_id, records)
    }

    pub fn seal_staged(&mut self, boundary: BoundaryKind) -> PactResult<Option<CommitRef>> {
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        self.resolver.seal_staged(&self.branch_id, boundary)
    }

    pub fn send(&mut self, options: &ProviderThreadOptions) -> PactResult<ProviderThreadView> {
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        let report = self.resolver.seal_pre_flight(&self.branch_id, options)?;
        report
            .get("thread")
            .cloned()
            .ok_or_else(|| PactError::resolver("send report did not include provider thread"))
    }

    pub fn receive(&mut self, parts: Vec<Component>, aux: PactAttrs) -> PactResult<ReceiveReport> {
        let parts = self.author_components(parts)?;
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        self.resolver.receive(&self.branch_id, parts, aux)
    }

    pub fn receive_values(
        &mut self,
        parts: Vec<Value>,
        aux: PactAttrs,
    ) -> PactResult<ReceiveReport> {
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        self.resolver
            .receive_json(&self.branch_id, Some(parts), aux)
    }

    #[allow(dead_code)]
    pub(crate) fn resolver_mut(&mut self) -> &mut PactResolver {
        &mut self.resolver
    }

    pub fn add_message(
        &mut self,
        role: &str,
        parts: Vec<Component>,
        aux: PactAttrs,
    ) -> PactResult<Value> {
        let parts = self.author_components(parts)?;
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        let outcome = self
            .resolver
            .add_message(&self.branch_id, role, parts, aux)?;
        Ok(json!({
            "schema": "pact.ingest_outcome.v1",
            "accepted": outcome.accepted,
            "deduped": outcome.deduped,
            "rejected": outcome.rejected,
        }))
    }

    pub fn add_message_values(
        &mut self,
        role: &str,
        parts: Vec<Value>,
        aux: PactAttrs,
    ) -> PactResult<Value> {
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        let outcome = self
            .resolver
            .add_message_json(&self.branch_id, role, parts, aux)?;
        Ok(json!({
            "schema": "pact.ingest_outcome.v1",
            "accepted": outcome.accepted,
            "deduped": outcome.deduped,
            "rejected": outcome.rejected,
        }))
    }

    pub fn receive_staged(&mut self, aux: PactAttrs) -> PactResult<ReceiveReport> {
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        self.resolver.receive_staged(&self.branch_id, aux)
    }

    pub fn unpaired_tool_call_ids(&self) -> Vec<String> {
        self.resolver.unpaired_tool_call_ids()
    }

    fn tree_for_view(
        &self,
        mode: ViewMode,
    ) -> PactResult<(ComponentTree, u64, BTreeMap<String, u64>)> {
        match mode {
            ViewMode::Current => {
                let effective_branch_id = self.effective_branch_id();
                let tree = self.tree_for(ViewMode::Current)?;
                if let Some(reference) = &self.reference_fork {
                    return Ok((
                        tree,
                        reference.virtual_head.head_revision,
                        reference.virtual_head.tree.first_seen.clone(),
                    ));
                }
                let branch = self
                    .resolver
                    .ledger()
                    .branches
                    .get(effective_branch_id)
                    .ok_or_else(|| {
                        PactError::not_found(format!("branch not found: {}", self.branch_id))
                    })?;
                let has_staged = self.resolver.has_resolved_staged_state(effective_branch_id);
                let head_revision = if has_staged {
                    branch.head_revision + 1
                } else {
                    branch.current.head_revision
                };
                let mut first_seen = branch.current.tree.first_seen.clone();
                if has_staged {
                    for component_id in tree.component_ids() {
                        first_seen
                            .entry(component_id.to_string())
                            .or_insert(head_revision);
                    }
                }
                Ok((tree, head_revision, first_seen))
            }
            ViewMode::Head => {
                let tree = self.tree_for(ViewMode::Head)?;
                let view = self.head()?;
                Ok((tree, view.head_revision, view.tree.first_seen.clone()))
            }
            ViewMode::Archive => {
                let effective_branch_id = self.effective_branch_id();
                let branch = self
                    .resolver
                    .ledger()
                    .branches
                    .get(effective_branch_id)
                    .ok_or_else(|| {
                        PactError::not_found(format!("branch not found: {}", self.branch_id))
                    })?;
                let (mut tree, first_seen) = crate::store::project_branch_archive_component_tree(
                    self.resolver.ledger(),
                    branch,
                    None,
                )?;
                tree.edit(|root| {
                    root.hydrate_content(&self.resolver.ledger().content);
                    Ok(())
                })?;
                Ok((tree, self.head()?.head_revision, first_seen))
            }
        }
    }

    fn tree_for(&self, mode: ViewMode) -> PactResult<ComponentTree> {
        if matches!(mode, ViewMode::Current) && self.resolver.ledger().base.is_none() {
            if let Some(tree) = self.composed_tree_cache().current.clone() {
                return Ok(tree);
            }
            let tree = self.tree_for_uncached(mode)?;
            self.composed_tree_cache().current = Some(tree.clone());
            return Ok(tree);
        }
        self.tree_for_uncached(mode)
    }

    fn tree_for_uncached(&self, mode: ViewMode) -> PactResult<ComponentTree> {
        if matches!(mode, ViewMode::Current) && self.resolver.ledger().base.is_some() {
            let ledger = self.resolver.ledger();
            let child_path = self.resolver.file_path.as_deref();
            let config = crate::file_format::ParentResolverConfig {
                base_dir: child_path.and_then(Path::parent).map(Path::to_path_buf),
                search_paths: Vec::new(),
                explicit_parents: std::collections::BTreeMap::new(),
            };
            let projection = crate::file_format::project_delta_overlay(
                child_path,
                ledger,
                Some(&self.branch_id),
                &config,
            )?;
            let mut tree = ComponentTree::from_value(&projection.tree)?;
            tree.edit(|root| {
                root.hydrate_content(&projection.resolved_base.ledger.content);
                root.hydrate_content(&ledger.content);
                Ok(())
            })?;
            return Ok(tree);
        }

        let mut tree = match mode {
            ViewMode::Current => self
                .resolver
                .branch_component_tree(self.effective_branch_id())?,
            ViewMode::Head => ComponentTree::from_root(self.head()?.tree.root.clone())?,
            ViewMode::Archive => self
                .resolver
                .archive_component_tree(self.effective_branch_id())?,
        };

        tree.edit(|root| {
            root.hydrate_content(&self.resolver.ledger().content);
            Ok(())
        })?;
        Ok(tree)
    }

    fn invalidate_composed_tree_cache(&self) {
        self.composed_tree_cache().clear();
    }

    fn composed_tree_cache(&self) -> MutexGuard<'_, ComposedTreeCache> {
        self.composed_tree_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stream_head(
        &self,
        view: &PactView,
        path_display: Option<&str>,
    ) -> crate::file_format::PactFileHead {
        let ledger = self.resolver.ledger();
        let manifest = self.resolver.file_manifest.as_ref();
        let revision = view.head_revision;
        let transaction_id = self.latest_transaction_for_branch_revision(revision);
        let virtual_reference = self.reference_fork.is_some();
        crate::file_format::PactFileHead {
            schema: "pact.head.v1".to_string(),
            file_path: path_display.unwrap_or_default().to_string(),
            file_kind: manifest
                .map(|manifest| manifest.file_kind.clone())
                .unwrap_or_default(),
            root_id: ledger.root_id.clone(),
            branch_id: self.branch_id.clone(),
            current_branch_id: if virtual_reference {
                self.branch_id.clone()
            } else {
                ledger.current_branch_id.clone()
            },
            is_current_branch: virtual_reference || self.branch_id == ledger.current_branch_id,
            revision,
            transaction_id: transaction_id.clone(),
            pact_ref: PactRef {
                root_id: ledger.root_id.clone(),
                branch_id: self.branch_id.clone(),
                revision: Some(revision),
                transaction_id,
                component_id: None,
                content_ref: None,
            },
            branch_count: manifest
                .map(|manifest| manifest.branch_count)
                .unwrap_or(ledger.branches.len() + usize::from(virtual_reference)),
            trace_count: manifest
                .map(|manifest| manifest.trace_count)
                .unwrap_or(ledger.trace_count()),
            record_count: manifest
                .map(|manifest| manifest.record_count)
                .unwrap_or_else(|| {
                    1 + ledger.branches.len()
                        + ledger.trace_count()
                        + view.tree.index.by_id.len()
                        + ledger.content.records.len()
                        + usize::from(virtual_reference)
                }),
            node_count: manifest
                .map(|manifest| manifest.node_count)
                .unwrap_or(view.tree.index.by_id.len()),
        }
    }

    fn latest_transaction_for_branch_revision(&self, revision: u64) -> Option<String> {
        let current = self.resolver.shared_ledger();
        let head = current
            .branches
            .get(&self.branch_id)
            .map(|branch| branch.head_revision);
        let ledger = if current.is_lazy() && head != Some(revision) {
            self.full_ledger().ok()?
        } else {
            current
        };
        ledger
            .event_index
            .transaction_traces
            .values()
            .filter(|trace| trace.branch_id == self.branch_id)
            .filter_map(|trace| {
                let trace_revision = trace_head_revision(trace)?;
                (trace_revision <= revision)
                    .then_some((trace_revision, trace.transaction_id.clone()))
            })
            .max_by_key(|(trace_revision, _)| *trace_revision)
            .map(|(_, transaction_id)| transaction_id)
    }

    fn resolve_op(&mut self, op: PactOperation) -> PactResult<TraceRef> {
        let op_kind = op.kind().to_string();
        self.activate_reference_fork()?;
        self.invalidate_composed_tree_cache();
        let outcome = self
            .resolver
            .resolve(vec![op], &self.branch_id, BoundaryKind::Authored)?;
        trace_ref_from_outcome(&op_kind, outcome)
    }

    fn author_components(&mut self, parts: Vec<Component>) -> PactResult<Vec<Component>> {
        let mut used = self.used_component_ids()?;
        parts
            .into_iter()
            .map(|component| self.author_component_with_used(component, &mut used, None))
            .collect()
    }

    fn author_component(&mut self, component: Component) -> PactResult<Component> {
        let mut used = self.used_component_ids()?;
        self.author_component_with_used(component, &mut used, None)
    }

    fn author_component_with_used(
        &mut self,
        mut component: Component,
        used: &mut BTreeSet<String>,
        parent_id: Option<&str>,
    ) -> PactResult<Component> {
        let local = if component.id.is_empty() {
            self.resolver.next_authoring_component_local_id()
        } else {
            local_part(&component.id).to_string()
        };
        component.id = self.mint_component_id(local, used);
        component.parent_id = parent_id.map(str::to_string);
        let parent_id = component.id.clone();
        let children = std::mem::take(&mut component.children);
        component.children = children
            .into_iter()
            .map(|child| self.author_component_with_used(child, used, Some(&parent_id)))
            .collect::<PactResult<Vec<_>>>()?;
        Ok(component)
    }

    fn used_component_ids(&self) -> PactResult<BTreeSet<String>> {
        self.resolver
            .branch_component_id_set(self.effective_branch_id())
    }

    fn mint_component_id(&mut self, local_id: String, used: &mut BTreeSet<String>) -> String {
        let id = format!("{}:{local_id}", self.context_tree_id);
        if used.insert(id.clone()) {
            return id;
        }

        loop {
            let local_id = self.resolver.next_authoring_component_local_id();
            let id = format!("{}:{local_id}", self.context_tree_id);
            if used.insert(id.clone()) {
                return id;
            }
        }
    }
}

fn trace_ref_from_outcome(op_kind: &str, outcome: ResolveOutcome) -> PactResult<TraceRef> {
    if !outcome.rejected.is_empty() {
        return Err(PactError::resolver(format!(
            "{op_kind} rejected by Pact: {}",
            outcome
                .rejected
                .iter()
                .map(|item| item.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    outcome
        .traces
        .last()
        .map(|trace| TraceRef {
            trace_id: trace.trace_id.clone(),
            op_kind: trace.op_kind.clone(),
            created_at_ns: trace.created_at_ns,
            produced_component_ids: trace
                .produced_refs
                .iter()
                .filter_map(|pact_ref| pact_ref.component_id.clone())
                .collect(),
            replaced_component_ids: outcome.replaced_component_ids,
            deleted_component_ids: outcome.deleted_component_ids,
        })
        .ok_or_else(|| PactError::resolver(format!("{op_kind} did not produce a staged trace")))
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

static CONTEXT_TREE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn local_part(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

pub(crate) fn mint_context_tree_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = CONTEXT_TREE_ID_COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    format!("{:032x}", nanos ^ (counter << 64))
}

#[derive(Clone, Debug)]
struct RewindTraceTarget {
    transaction_id: String,
    boundary: Option<String>,
    from_revision: u64,
    to_revision: u64,
}

pub(crate) fn build_rewind_targets_value<'a>(
    root_id: &str,
    branch_id: &str,
    head_revision: u64,
    messages: impl IntoIterator<Item = (String, Option<String>)>,
    traces: impl IntoIterator<Item = &'a PactTransactionTrace>,
) -> Value {
    let mut target_by_message_id = BTreeMap::new();
    for trace in traces {
        let Some(target) = rewind_target_from_trace(trace) else {
            continue;
        };
        for message_id in produced_message_turn_ids(trace) {
            target_by_message_id
                .entry(message_id)
                .or_insert_with(|| target.clone());
        }
    }
    let targets = messages
        .into_iter()
        .enumerate()
        .map(|(order, (message_id, role))| {
            let target = target_by_message_id.get(&message_id);
            json!({
                "message_id": message_id,
                "role": role,
                "order": order,
                "transaction_id": target.map(|target| target.transaction_id.clone()),
                "boundary": target.and_then(|target| target.boundary.clone()),
                "from_revision": target.map(|target| target.from_revision),
                "to_revision": target.map(|target| target.to_revision),
                "at_ref": target.map(|target| pact_ref_value(
                    root_id,
                    branch_id,
                    target.to_revision,
                    &target.transaction_id,
                )),
                "before_ref": target.map(|target| pact_ref_value(
                    root_id,
                    branch_id,
                    target.from_revision,
                    &target.transaction_id,
                )),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": "pact.rewind_targets.v1",
        "branch_id": branch_id,
        "head_revision": head_revision,
        "targets": targets,
    })
}

fn rewind_target_from_trace(trace: &PactTransactionTrace) -> Option<RewindTraceTarget> {
    let to_revision = trace
        .replay_trace
        .as_ref()
        .map(|replay| replay.to_rev)
        .or_else(|| {
            trace
                .commit
                .as_ref()
                .and_then(|commit| commit.pact_ref.as_ref())
                .and_then(|pact_ref| pact_ref.revision)
        })
        .or_else(|| trace_head_revision(trace))?;
    let from_revision = trace
        .replay_trace
        .as_ref()
        .map(|replay| replay.from_rev)
        .or(trace.pre_request_ref.revision)
        .unwrap_or_else(|| to_revision.saturating_sub(1));
    let boundary = trace
        .commit
        .as_ref()
        .map(|commit| commit.phase.clone())
        .filter(|phase| !phase.is_empty());
    Some(RewindTraceTarget {
        transaction_id: trace.transaction_id.clone(),
        boundary,
        from_revision,
        to_revision,
    })
}

fn produced_message_turn_ids(trace: &PactTransactionTrace) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(events) = trace.aux.get("events") {
        collect_produced_ref_ids(events, &mut ids, &mut seen);
    }

    if let Some(replay) = trace.replay_trace.as_ref() {
        for op in &replay.ops {
            if let TraceOpKind::InsertComponent { component, .. } = &op.kind
                && component.get("componentType").and_then(Value::as_str) == Some("message_turn")
            {
                let component_id = component
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&op.component_id);
                push_unique(component_id, &mut ids, &mut seen);
            }
        }
    }

    ids
}

fn collect_produced_ref_ids(value: &Value, ids: &mut Vec<String>, seen: &mut BTreeSet<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_produced_ref_ids(item, ids, seen);
            }
        }
        Value::Object(object) => {
            if let Some(produced_refs) = object.get("produced_refs").and_then(Value::as_array) {
                for produced_ref in produced_refs {
                    if let Some(component_id) =
                        produced_ref.get("component_id").and_then(Value::as_str)
                    {
                        push_unique(component_id, ids, seen);
                    }
                }
            }
            for item in object.values() {
                collect_produced_ref_ids(item, ids, seen);
            }
        }
        _ => {}
    }
}

fn push_unique(id: &str, ids: &mut Vec<String>, seen: &mut BTreeSet<String>) {
    if seen.insert(id.to_string()) {
        ids.push(id.to_string());
    }
}

fn pact_ref_value(root_id: &str, branch_id: &str, revision: u64, transaction_id: &str) -> Value {
    json!({
        "root_id": root_id,
        "branch_id": branch_id,
        "revision": revision,
        "transaction_id": transaction_id,
    })
}

fn trace_head_revision(trace: &PactTransactionTrace) -> Option<u64> {
    [
        trace.post_response_ref.as_ref(),
        trace.post_response_alteration_ref.as_ref(),
        trace.response_ref.as_ref(),
        trace.request_ref.as_ref(),
        Some(&trace.post_request_alteration_ref),
        Some(&trace.pre_request_ref),
        Some(&trace.authored_ref),
    ]
    .into_iter()
    .flatten()
    .find_map(|pact_ref| pact_ref.revision)
}

#[cfg(test)]
mod tests;
