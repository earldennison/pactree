use crate::store::{ContextTreeLedger, PactBranch, PactRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const VIEW_REF_SCHEMA: &str = "pact.view_ref.v1";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    ReadOnlyLive,

    ReadOnlyPinned,

    MutableConsumer,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ViewRef {
    #[serde(default = "default_view_ref_schema")]
    pub schema: String,
    pub source_root_id: String,
    pub source_branch_id: String,
    pub kind: ViewKind,

    pub source_revision_at_creation: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer_branch_id: Option<String>,
}

fn default_view_ref_schema() -> String {
    VIEW_REF_SCHEMA.to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum ViewError {
    SourceBranchNotFound(String),
    ConsumerBranchAlreadyExists(String),
    InvalidPinnedRevision { source_head: u64, requested: u64 },
    ConsumerBranchCloneFailed(String),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::SourceBranchNotFound(id) => {
                write!(f, "source branch {id:?} not found")
            }
            ViewError::ConsumerBranchAlreadyExists(id) => {
                write!(f, "consumer branch {id:?} already exists")
            }
            ViewError::InvalidPinnedRevision {
                source_head,
                requested,
            } => write!(
                f,
                "pinned revision {requested} exceeds source head {source_head}"
            ),
            ViewError::ConsumerBranchCloneFailed(reason) => {
                write!(f, "consumer branch clone failed: {reason}")
            }
        }
    }
}

impl std::error::Error for ViewError {}

pub fn open_read_only_live(
    root_store: &ContextTreeLedger,
    source_branch_id: &str,
) -> Result<ViewRef, ViewError> {
    let source = root_store
        .branches
        .get(source_branch_id)
        .ok_or_else(|| ViewError::SourceBranchNotFound(source_branch_id.to_string()))?;
    Ok(ViewRef {
        schema: default_view_ref_schema(),
        source_root_id: root_store.root_id.clone(),
        source_branch_id: source_branch_id.to_string(),
        kind: ViewKind::ReadOnlyLive,
        source_revision_at_creation: source.head_revision,
        consumer_branch_id: None,
    })
}

pub fn open_read_only_pinned(
    root_store: &ContextTreeLedger,
    source_branch_id: &str,
    revision: Option<u64>,
) -> Result<ViewRef, ViewError> {
    let source = root_store
        .branches
        .get(source_branch_id)
        .ok_or_else(|| ViewError::SourceBranchNotFound(source_branch_id.to_string()))?;
    let pinned = revision.unwrap_or(source.head_revision);
    if pinned > source.head_revision {
        return Err(ViewError::InvalidPinnedRevision {
            source_head: source.head_revision,
            requested: pinned,
        });
    }
    Ok(ViewRef {
        schema: default_view_ref_schema(),
        source_root_id: root_store.root_id.clone(),
        source_branch_id: source_branch_id.to_string(),
        kind: ViewKind::ReadOnlyPinned,
        source_revision_at_creation: pinned,
        consumer_branch_id: None,
    })
}

pub fn open_mutable_consumer(
    root_store: &mut ContextTreeLedger,
    source_branch_id: &str,
    consumer_branch_id: &str,
) -> Result<ViewRef, ViewError> {
    if root_store.branches.contains_key(consumer_branch_id) {
        return Err(ViewError::ConsumerBranchAlreadyExists(
            consumer_branch_id.to_string(),
        ));
    }
    let source = root_store
        .branches
        .get(source_branch_id)
        .ok_or_else(|| ViewError::SourceBranchNotFound(source_branch_id.to_string()))?
        .clone();

    let fork_point = source.head_revision;
    let fork_ref = PactRef {
        root_id: source.root_id.clone(),
        branch_id: source.branch_id.clone(),
        revision: Some(fork_point),
        transaction_id: None,
        component_id: None,
        content_ref: None,
    };

    let mut consumer_aux: BTreeMap<String, Value> = source.aux.clone();
    consumer_aux.insert(
        "view".to_string(),
        serde_json::json!({
            "source_branch_id": source_branch_id,
            "source_revision": fork_point,
            "kind": "mutable_consumer",
        }),
    );

    let mut consumer_current = source.current.clone();
    consumer_current.branch_id = consumer_branch_id.to_string();

    let consumer = PactBranch {
        branch_id: consumer_branch_id.to_string(),
        root_id: source.root_id.clone(),
        parent_branch_id: Some(source.branch_id.clone()),
        fork_ref: Some(fork_ref.clone()),
        base_ref: source.base_ref.clone(),
        head_revision: source.head_revision,
        lifecycle_tick: source.lifecycle_tick,
        transaction_traces: source.transaction_traces.clone(),
        current: consumer_current,
        aux: consumer_aux,
    };

    root_store
        .branches
        .insert(consumer_branch_id.to_string(), consumer);

    Ok(ViewRef {
        schema: default_view_ref_schema(),
        source_root_id: root_store.root_id.clone(),
        source_branch_id: source_branch_id.to_string(),
        kind: ViewKind::MutableConsumer,
        source_revision_at_creation: fork_point,
        consumer_branch_id: Some(consumer_branch_id.to_string()),
    })
}

pub fn resolve_view(
    root_store: &ContextTreeLedger,
    view_ref: &ViewRef,
) -> Result<PactRef, ViewError> {
    match view_ref.kind {
        ViewKind::ReadOnlyLive => {
            let source = root_store
                .branches
                .get(&view_ref.source_branch_id)
                .ok_or_else(|| {
                    ViewError::SourceBranchNotFound(view_ref.source_branch_id.clone())
                })?;
            Ok(PactRef {
                root_id: view_ref.source_root_id.clone(),
                branch_id: view_ref.source_branch_id.clone(),
                revision: Some(source.head_revision),
                transaction_id: None,
                component_id: None,
                content_ref: None,
            })
        }
        ViewKind::ReadOnlyPinned => Ok(PactRef {
            root_id: view_ref.source_root_id.clone(),
            branch_id: view_ref.source_branch_id.clone(),
            revision: Some(view_ref.source_revision_at_creation),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        }),
        ViewKind::MutableConsumer => {
            let consumer_id = view_ref.consumer_branch_id.as_deref().ok_or_else(|| {
                ViewError::ConsumerBranchCloneFailed(
                    "mutable consumer view missing consumer_branch_id".to_string(),
                )
            })?;
            let consumer = root_store
                .branches
                .get(consumer_id)
                .ok_or_else(|| ViewError::SourceBranchNotFound(consumer_id.to_string()))?;
            Ok(PactRef {
                root_id: view_ref.source_root_id.clone(),
                branch_id: consumer_id.to_string(),
                revision: Some(consumer.head_revision),
                transaction_id: None,
                component_id: None,
                content_ref: None,
            })
        }
    }
}

#[cfg(test)]
mod tests;
