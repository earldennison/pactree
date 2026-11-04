use crate::components::Component;
use crate::normalize::NormalizedNode;
use crate::tree::ComponentTree;
use serde_json::Value;

pub fn render_xml(root: &NormalizedNode) -> String {
    let mut out = String::new();
    render_node(root, 0, &mut out);
    out
}

pub fn render_component_xml(root: &Component) -> String {
    let mut out = String::new();
    let root = XmlComponent::from_component(root, 0);
    render_xml_component(&root, 0, &mut out);
    out
}

pub(crate) fn render_tree_xml(tree: &ComponentTree) -> String {
    render_component_xml(tree.root())
}

struct XmlComponent<'a> {
    element_name: String,
    attrs: Vec<(String, String)>,
    content: Option<&'a str>,
    children: Vec<XmlComponent<'a>>,
}

impl<'a> XmlComponent<'a> {
    fn from_component(component: &'a Component, depth: i32) -> Self {
        let component_type = component.component_type();
        let mut attrs = Vec::new();
        push_xml_attr(&mut attrs, "id", component.id.clone());
        push_xml_attr(&mut attrs, "componentType", component_type.to_string());
        if let Some(kind) = &component.subtype {
            push_xml_attr(&mut attrs, "type", kind.clone());
        }
        if let Some(role) = &component.role {
            push_xml_attr(&mut attrs, "role", role.clone());
        }
        push_xml_attr(&mut attrs, "offset", component.offset.to_string());
        push_xml_attr(&mut attrs, "depth", depth.to_string());
        push_xml_attr(&mut attrs, "priority", component.priority.to_string());
        push_xml_attr(
            &mut attrs,
            "transaction_index",
            component.transaction_index.to_string(),
        );
        push_xml_attr(
            &mut attrs,
            "created_at_ns",
            component.created_at_ns.to_string(),
        );
        push_xml_attr(
            &mut attrs,
            "creation_index",
            component.creation_index.to_string(),
        );

        if let Some(xml_attrs) = component.xml_attrs() {
            for (key, value) in xml_attrs {
                push_xml_attr(&mut attrs, key, attr_value_to_string(value));
            }
        }

        let mut children = component.children.iter().collect::<Vec<_>>();
        children.sort_by(|left, right| {
            (
                left.offset,
                left.priority,
                left.created_at_ns,
                left.creation_index,
                left.id.as_str(),
            )
                .cmp(&(
                    right.offset,
                    right.priority,
                    right.created_at_ns,
                    right.creation_index,
                    right.id.as_str(),
                ))
        });

        Self {
            element_name: component_type.to_string(),
            attrs,
            content: component.content(),
            children: children
                .into_iter()
                .map(|child| XmlComponent::from_component(child, depth + 1))
                .collect(),
        }
    }
}

fn push_xml_attr(attrs: &mut Vec<(String, String)>, key: &str, value: String) {
    attrs.push((key.to_string(), value));
}

fn render_xml_component(component: &XmlComponent<'_>, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    out.push_str(&pad);
    out.push('<');
    out.push_str(&xml_name(&component.element_name));
    for (key, value) in &component.attrs {
        push_attr(out, key, value);
    }

    let content = component.content.unwrap_or("");
    if content.is_empty() && component.children.is_empty() {
        out.push_str(" />\n");
        return;
    }

    out.push('>');
    if !content.is_empty() {
        out.push_str(&escape_text(content));
    }
    if !component.children.is_empty() {
        out.push('\n');
        for child in &component.children {
            render_xml_component(child, indent + 1, out);
        }
        out.push_str(&pad);
    }
    out.push_str("</");
    out.push_str(&xml_name(&component.element_name));
    out.push_str(">\n");
}

fn render_node(node: &NormalizedNode, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    out.push_str(&pad);
    out.push('<');
    out.push_str(&xml_name(&node.node_type));
    push_attr(out, "id", &node.id);
    push_attr(out, "componentType", &node.node_type);
    if let Some(kind) = &node.kind {
        push_attr(out, "type", kind);
    }
    if let Some(role) = &node.role {
        push_attr(out, "role", role);
    }
    push_attr(out, "offset", &node.offset.to_string());
    push_attr(out, "depth", &node.depth.to_string());
    push_attr(out, "priority", &node.priority.to_string());
    push_attr(
        out,
        "transaction_index",
        &node.transaction_index.to_string(),
    );
    push_attr(out, "created_at_ns", &node.created_at_ns.to_string());
    push_attr(out, "creation_index", &node.creation_index.to_string());
    for (key, value) in &node.attrs {
        push_attr(out, key, &attr_value_to_string(value));
    }

    if node.content.is_empty() && node.children.is_empty() {
        out.push_str(" />\n");
        return;
    }

    out.push('>');
    if !node.content.is_empty() {
        out.push_str(&escape_text(&node.content));
    }
    if !node.children.is_empty() {
        out.push('\n');
        for child in &node.children {
            render_node(child, indent + 1, out);
        }
        out.push_str(&pad);
    }
    out.push_str("</");
    out.push_str(&xml_name(&node.node_type));
    out.push_str(">\n");
}

fn push_attr(out: &mut String, key: &str, value: &str) {
    out.push(' ');
    out.push_str(&xml_name(key));
    out.push_str("=\"");
    out.push_str(&escape_attr(value));
    out.push('"');
}

fn attr_value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(items) => items
            .iter()
            .map(attr_value_to_string)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(_) => value.to_string(),
    }
}

fn xml_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, ch) in value.chars().enumerate() {
        let valid = ch == '_' || ch == '-' || ch == ':' || ch.is_ascii_alphanumeric();
        if valid && !(index == 0 && ch.is_ascii_digit()) {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "Node".to_string()
    } else {
        out
    }
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests;
