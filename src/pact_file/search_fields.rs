use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchFieldLocator {
    pub component_id: String,

    pub field: String,

    pub offset: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchFieldPage {
    pub locator: SearchFieldLocator,

    pub data: Vec<u8>,

    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContentRange {
    pub content_id: String,

    pub offset: u64,

    pub data: Vec<u8>,

    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchHitLocator {
    pub component_id: String,

    pub field: String,

    pub offset: u64,

    pub max_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HydratedSearchHit {
    pub component_id: String,

    pub field: SearchFieldPage,
}

pub(crate) fn content_chunk_start(
    body_offsets: &[u64],
    body_address_count: usize,
    offset: u64,
) -> (usize, u64) {
    if body_offsets.len() != body_address_count || body_offsets.is_empty() {
        return (0, 0);
    }
    let index = body_offsets
        .partition_point(|chunk_offset| *chunk_offset <= offset)
        .saturating_sub(1);
    (index, body_offsets[index])
}
