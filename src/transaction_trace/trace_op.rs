use crate::error::{PactError, PactResult};
use serde_json::Value;

pub(crate) const CHILDREN_KEY: &str = "children";

pub(crate) const ATTRS_KEY: &str = "aux";
pub(crate) const CONTENT_KEY: &str = "content";
pub(crate) const CONTENT_REF_KEY: &str = "content_ref";
pub(crate) const CONTENT_PREVIEW_KEY: &str = "content_preview";
pub(crate) const LIFECYCLE_KEY: &str = "lifecycle";
pub(crate) const ID_KEY: &str = "id";

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraceOp {
    pub component_id: String,
    #[serde(flatten)]
    pub kind: TraceOpKind,
}

impl TraceOp {
    pub fn new(component_id: impl Into<String>, kind: TraceOpKind) -> Self {
        Self {
            component_id: component_id.into(),
            kind,
        }
    }

    pub fn validate(&self) -> PactResult<()> {
        if self.component_id.is_empty() {
            return Err(PactError::invalid("trace op requires component_id"));
        }
        self.kind.validate_for_component(&self.component_id)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TraceOpKind {
    InsertComponent {
        parent_id: String,
        index: usize,
        #[serde(alias = "node")]
        component: Value,
    },

    DeleteComponent,

    MoveComponent {
        parent_id: String,
        index: usize,
    },

    SetField {
        key: String,
        value: Value,
    },

    UnsetField {
        key: String,
    },

    SetAttr {
        key: String,
        value: Value,
    },

    UnsetAttr {
        key: String,
    },

    SetContent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_ref: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_preview: Option<String>,
    },

    ReplaceContentRange {
        start: usize,
        end: usize,
        replacement: String,
    },

    SetLifecycle {
        value: Value,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum ComponentChange {
    SetField {
        key: String,
        value: Value,
    },
    UnsetField {
        key: String,
    },
    SetAux {
        key: String,
        value: Value,
    },
    UnsetAux {
        key: String,
    },
    SetContent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_ref: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_preview: Option<String>,
    },
    SetLifecycle {
        value: Value,
    },
}

impl ComponentChange {
    pub fn into_trace_op_kind(self) -> TraceOpKind {
        match self {
            Self::SetField { key, value } => TraceOpKind::SetField { key, value },
            Self::UnsetField { key } => TraceOpKind::UnsetField { key },
            Self::SetAux { key, value } => TraceOpKind::SetAttr { key, value },
            Self::UnsetAux { key } => TraceOpKind::UnsetAttr { key },
            Self::SetContent {
                content,
                content_ref,
                content_preview,
            } => TraceOpKind::SetContent {
                content,
                content_ref,
                content_preview,
            },
            Self::SetLifecycle { value } => TraceOpKind::SetLifecycle { value },
        }
    }
}

impl TraceOpKind {
    fn validate_for_component(&self, component_id: &str) -> PactResult<()> {
        match self {
            TraceOpKind::InsertComponent {
                parent_id,
                component,
                ..
            } => {
                if parent_id.is_empty() {
                    return Err(PactError::invalid(
                        "insert_component trace op requires parent_id",
                    ));
                }
                let object = component.as_object().ok_or_else(|| {
                    PactError::invalid("insert_component trace op requires component object")
                })?;
                if let Some(id) = object.get(ID_KEY).and_then(Value::as_str)
                    && id != component_id
                {
                    return Err(PactError::invalid(format!(
                        "insert_component payload id {id} does not match component_id {component_id}"
                    )));
                }
                if object
                    .get(CHILDREN_KEY)
                    .and_then(Value::as_array)
                    .is_some_and(|children| !children.is_empty())
                {
                    return Err(PactError::invalid(
                        "insert_component trace op payload must not include child snapshots",
                    ));
                }
            }
            TraceOpKind::MoveComponent { parent_id, .. } => {
                if parent_id.is_empty() {
                    return Err(PactError::invalid(
                        "move_component trace op requires parent_id",
                    ));
                }
            }
            TraceOpKind::SetField { key, .. }
            | TraceOpKind::UnsetField { key }
            | TraceOpKind::SetAttr { key, .. }
            | TraceOpKind::UnsetAttr { key } => {
                if key.is_empty() {
                    return Err(PactError::invalid("trace op key must not be empty"));
                }
            }
            TraceOpKind::ReplaceContentRange { start, end, .. } => {
                if start > end {
                    return Err(PactError::invalid(
                        "replace_content_range trace op start must be <= end",
                    ));
                }
            }
            TraceOpKind::DeleteComponent
            | TraceOpKind::SetContent { .. }
            | TraceOpKind::SetLifecycle { .. } => {}
        }
        Ok(())
    }
}

pub(crate) fn is_reserved_field(key: &str) -> bool {
    matches!(
        key,
        CHILDREN_KEY
            | ATTRS_KEY
            | CONTENT_KEY
            | CONTENT_REF_KEY
            | CONTENT_PREVIEW_KEY
            | LIFECYCLE_KEY
            | ID_KEY
    )
}
