use super::*;
use crate::error::PactResult;

pub(crate) fn head_state_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    file_head_json(path, branch_id, revision)
}

pub(crate) fn head_state_xml(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
) -> PactResult<String> {
    file_head_xml(path, branch_id, revision)
}

pub(crate) fn archived_state_json(
    path: &Path,
    branch_id: Option<&str>,
    revision: Option<u64>,
    blob_inline_limit: usize,
) -> PactResult<String> {
    file_projection_json_with_scope(
        path,
        branch_id,
        revision,
        blob_inline_limit,
        None,
        None,
        crate::store::ProjectionTreeScope::Archive,
    )
}
