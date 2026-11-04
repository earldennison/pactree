#![allow(dead_code, unused_imports)]

mod api;
mod branch;
mod branch_projection;
mod content;
mod model;
mod projection;
mod trace;
mod util;
mod view;

pub use api::externalize_large_content_in_value;
pub use api::{
    bootstrap_ledger, bootstrap_ledger_json, checkpoint_store_json, content_store_put_inline_json,
    empty_context_root_value, fork_store_json, ledger_create_branch_at_node_json,
    ledger_create_branch_at_revision_json, ledger_create_branch_at_transaction_json,
    ledger_delete_json, ledger_from_store_json, ledger_inspect_branch_json, ledger_inspect_json,
    ledger_project_branch, ledger_project_branch_json, ledger_project_branch_json_with_scope,
    ledger_project_branch_tree_json, ledger_project_branch_tree_json_with_scope,
    ledger_project_branch_with_scope, ledger_record_transaction_trace_json, new_ledger_json,
    new_ledger_json_without_base, new_store_json, project_store_json, transaction_trace_build_json,
    transaction_trace_validate_json,
};
pub(crate) use branch_projection::next_branch_id;
pub(crate) use branch_projection::project_branch_archive_tree_value;
pub use branch_projection::{ProjectionTreeScope, decode_context_tree_ledger};
pub(crate) use branch_projection::{
    branch_has_native_component_projection_at, project_branch_archive_component_tree,
    project_branch_component_tree, project_branch_component_window, project_branch_tree_value,
    reject_removed_patch_history,
};
pub(crate) use model::ComponentRecord;
pub use model::{
    AlterationSource, BlobRecord, BranchEventIndex, CHECKPOINT_SCHEMA, ComponentPath,
    ComponentViewIndex, ContentRecord, ContentRecordSummary, ContentStorageRef, ContentStore,
    ContextTreeLedger, ContextTreeView, LedgerHistory, PACT_DERIVED_DEPTH_AUX_KEY, PactAlteration,
    PactAlterationRef, PactBaseMode, PactBaseProvenance, PactBoundary, PactBoundaryView,
    PactBranch, PactCommit, PactParentLocator, PactParentProvenance, PactProjection, PactRef,
    PactRefSpec, PactStore, PactStoreStats, PactTransactionTrace, PactTransactionTraceBuildRequest,
    PactView, PactViewMode, PactViewRow, PactViewRows, ROOT_STORE_SCHEMA, STORE_SCHEMA,
    TRANSACTION_TRACE_BUILD_REQUEST_SCHEMA, TRANSACTION_TRACE_SCHEMA, ToolPairRecord,
};
pub(crate) use projection::build_projection_from_owned_component;
pub use projection::{build_projection, build_projection_from_component};
pub(crate) use util::stable_content_hash;

#[cfg(test)]
mod tests;
