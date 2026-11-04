use super::trace_op::{TraceOp, TraceOpKind};
use serde_json::Value;

pub(crate) fn inline_content(node: &Value) -> Option<&str> {
    node.get(super::trace_op::CONTENT_KEY)
        .and_then(Value::as_str)
}

fn common_prefix_len(old: &str, new: &str) -> usize {
    let max = old.len().min(new.len());
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut i = 0;
    let mut boundary = 0;
    while i < max && ob[i] == nb[i] {
        i += 1;
        if old.is_char_boundary(i) {
            boundary = i;
        }
    }
    boundary
}

fn common_suffix_len(old: &str, new: &str, prefix: usize) -> usize {
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let max = (old.len() - prefix).min(new.len() - prefix);
    let mut i = 0;
    let mut boundary = 0;
    while i < max && ob[old.len() - 1 - i] == nb[new.len() - 1 - i] {
        i += 1;
        if old.is_char_boundary(old.len() - i) && new.is_char_boundary(new.len() - i) {
            boundary = i;
        }
    }
    boundary
}

pub(crate) fn content_op(component_id: &str, old: &Value, new: &Value) -> Option<TraceOp> {
    let old_content = old.get(super::trace_op::CONTENT_KEY);
    let new_content = new.get(super::trace_op::CONTENT_KEY);
    let old_ref = old.get(super::trace_op::CONTENT_REF_KEY);
    let new_ref = new.get(super::trace_op::CONTENT_REF_KEY);
    let new_preview = new.get(super::trace_op::CONTENT_PREVIEW_KEY);

    if old_content == new_content && old_ref == new_ref {
        return None;
    }

    if let (Some(old_str), Some(new_str)) = (inline_content(old), inline_content(new)) {
        let prefix = common_prefix_len(old_str, new_str);
        let suffix = common_suffix_len(old_str, new_str, prefix);
        let start = prefix;
        let end = old_str.len() - suffix;
        let replacement = new_str[prefix..new_str.len() - suffix].to_string();

        if replacement.len() < new_str.len() {
            return Some(TraceOp::new(
                component_id,
                TraceOpKind::ReplaceContentRange {
                    start,
                    end,
                    replacement,
                },
            ));
        }
    }

    Some(TraceOp::new(
        component_id,
        TraceOpKind::SetContent {
            content: new_content.and_then(Value::as_str).map(str::to_string),
            content_ref: new_ref.cloned(),
            content_preview: new_preview.and_then(Value::as_str).map(str::to_string),
        },
    ))
}
