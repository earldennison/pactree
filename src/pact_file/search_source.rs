use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchSourceField {
    pub field: String,

    pub is_json: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchSourceComponent {
    pub ordinal: u64,
    pub component_id: String,
    pub parent_id: Option<String>,
    pub component_type: String,
    pub role: Option<String>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub success: Option<bool>,
    pub cycle: Option<u64>,

    pub fields: Vec<PactSearchSourceField>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchSourcePage {
    pub total_components: u64,
    pub components: Vec<PactSearchSourceComponent>,

    pub next_ordinal: Option<u64>,
}
