use serde::{Deserialize, Serialize};

pub const TOOL_RECORD_SCHEMA: &str = "pact.tool_record.v1";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Requested,

    Running,

    Completed,

    Failed,

    Cancelled,
}

impl ToolCallStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolCallStatus::Requested => "requested",
            ToolCallStatus::Running => "running",
            ToolCallStatus::Completed => "completed",
            ToolCallStatus::Failed => "failed",
            ToolCallStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_paired(self) -> bool {
        matches!(
            self,
            ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
        )
    }

    pub fn is_provider_safe(self) -> bool {
        self.is_paired()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolCallRecord {
    #[serde(default = "default_record_schema")]
    pub schema: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolCallStatus,
    pub branch_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_node_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_node_id: Option<String>,

    pub parent_message_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

fn default_record_schema() -> String {
    TOOL_RECORD_SCHEMA.to_string()
}
