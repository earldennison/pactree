use crate::components::Component;
use crate::error::{PactError, PactResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const PACT_OPERATION_SCHEMA: &str = "pact.operation.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceActor {
    User,
    Assistant,
    Host,
    System,
}

impl SourceActor {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceActor::User => "user",
            SourceActor::Assistant => "assistant",
            SourceActor::Host => "host",
            SourceActor::System => "system",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PactOperationEnvelope {
    pub branch_id: String,

    pub source: SourceActor,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_timestamp_ns: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,

    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PactOperation {
    Insert {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<i32>,
        component: Component,
    },

    Update {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_component_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_ref: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        fields: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        aux: BTreeMap<String, Value>,
    },

    Replace {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_component_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
        component: Component,
    },

    Append {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_component_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<i32>,
        component: Component,
    },

    Upsert {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_component_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<i32>,
        component: Component,
    },

    Delete {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_component_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
    },

    Move {
        envelope: PactOperationEnvelope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_component_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_selector: Option<String>,
        destination_selector: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
}

impl PactOperation {
    pub fn envelope(&self) -> &PactOperationEnvelope {
        match self {
            PactOperation::Insert { envelope, .. }
            | PactOperation::Update { envelope, .. }
            | PactOperation::Replace { envelope, .. }
            | PactOperation::Append { envelope, .. }
            | PactOperation::Upsert { envelope, .. }
            | PactOperation::Delete { envelope, .. }
            | PactOperation::Move { envelope, .. } => envelope,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            PactOperation::Insert { .. } => "insert",
            PactOperation::Update { .. } => "update",
            PactOperation::Replace { .. } => "replace",
            PactOperation::Append { .. } => "append",
            PactOperation::Upsert { .. } => "upsert",
            PactOperation::Delete { .. } => "delete",
            PactOperation::Move { .. } => "move",
        }
    }

    pub fn validate(&self) -> PactResult<()> {
        let env = self.envelope();
        if env.branch_id.is_empty() {
            return Err(PactError::invalid(format!(
                "{}: branch_id is empty",
                self.kind()
            )));
        }
        match self {
            PactOperation::Insert {
                target_selector,
                parent_id,
                depth,
                component,
                ..
            } => {
                if target_selector.as_deref().unwrap_or("").is_empty()
                    && parent_id.as_deref().unwrap_or("").is_empty()
                    && depth.is_none()
                {
                    return Err(PactError::invalid(
                        "insert: target_selector, parent_id, or depth is required",
                    ));
                }
                if component.id.is_empty() {
                    return Err(PactError::invalid("insert: component id is required"));
                }
            }
            PactOperation::Update {
                target_component_id,
                target_selector,
                content,
                content_ref,
                fields,
                aux,
                ..
            } => {
                if target_component_id.as_deref().unwrap_or("").is_empty()
                    && target_selector.as_deref().unwrap_or("").is_empty()
                {
                    return Err(PactError::invalid(
                        "update: target_component_id or target_selector is required",
                    ));
                }
                if content.is_none() && content_ref.is_none() && fields.is_empty() && aux.is_empty()
                {
                    return Err(PactError::invalid(
                        "update: content, content_ref, fields, or aux must be set",
                    ));
                }
            }
            PactOperation::Replace {
                target_component_id,
                target_selector,
                component,
                ..
            } => {
                if target_component_id.as_deref().unwrap_or("").is_empty()
                    && target_selector.as_deref().unwrap_or("").is_empty()
                {
                    return Err(PactError::invalid(
                        "replace: target_component_id or target_selector is required",
                    ));
                }
                if component.id.is_empty() {
                    return Err(PactError::invalid("replace: component id is required"));
                }
            }
            PactOperation::Append {
                target_component_id,
                target_selector,
                component,
                ..
            } => {
                if target_component_id.as_deref().unwrap_or("").is_empty()
                    && target_selector.as_deref().unwrap_or("").is_empty()
                {
                    return Err(PactError::invalid(
                        "append: target_component_id or target_selector is required",
                    ));
                }
                if component.id.is_empty() {
                    return Err(PactError::invalid("append: component id is required"));
                }
            }
            PactOperation::Upsert {
                target_component_id,
                target_selector,
                parent_id,
                depth,
                component,
                ..
            } => {
                if target_component_id.as_deref().unwrap_or("").is_empty()
                    && target_selector.as_deref().unwrap_or("").is_empty()
                    && parent_id.as_deref().unwrap_or("").is_empty()
                    && depth.is_none()
                {
                    return Err(PactError::invalid(
                        "upsert: target_component_id, target_selector, parent_id, or depth is required",
                    ));
                }
                if component.id.is_empty() {
                    return Err(PactError::invalid("upsert: component id is required"));
                }
            }
            PactOperation::Delete {
                target_component_id,
                target_selector,
                ..
            } => {
                if target_component_id.as_deref().unwrap_or("").is_empty()
                    && target_selector.as_deref().unwrap_or("").is_empty()
                {
                    return Err(PactError::invalid(
                        "delete: target_component_id or target_selector is required",
                    ));
                }
            }
            PactOperation::Move {
                target_component_id,
                target_selector,
                destination_selector,
                mode,
                ..
            } => {
                if target_component_id.as_deref().unwrap_or("").is_empty()
                    && target_selector.as_deref().unwrap_or("").is_empty()
                {
                    return Err(PactError::invalid(
                        "move: target_component_id or target_selector is required",
                    ));
                }
                if destination_selector.is_empty() {
                    return Err(PactError::invalid("move: destination_selector is required"));
                }
                if let Some(mode) = mode.as_deref()
                    && !matches!(mode, "insert" | "append" | "replace")
                {
                    return Err(PactError::invalid(format!(
                        "move: unsupported mode `{mode}`; use insert, append, or replace"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
