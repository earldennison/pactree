use super::*;
use crate::error::{PactError, PactResult};

pub fn view_head_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    super::state::head_state_json(path, branch_id, revision)
}

pub fn view_head_xml(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    super::state::head_state_xml(path, branch_id, revision)
}

pub fn view_head_rows_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    file_stream_json(
        path,
        "current_surface",
        branch_id,
        revision,
        None,
        None,
        "before",
        500,
        blob_inline_limit,
    )
}

pub fn view_archived_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    super::state::archived_state_json(path, branch_id, revision, blob_inline_limit)
}

pub fn view_archived_rows_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    let projection = super::file_projection_with_scope(
        path,
        branch_id,
        revision,
        blob_inline_limit,
        None,
        None,
        crate::store::ProjectionTreeScope::Archive,
    )?;
    let rows = super::stream_rows_from_file_projection(&projection);
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.view.archived.rows.v1"),
        "scope": "archive",
        "row_count": rows.len(),
        "rows": rows,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode archived rows: {err}")))
}
