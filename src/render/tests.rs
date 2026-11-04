use super::*;
use crate::components::Component;
use crate::normalize::normalize_tree;
use crate::tree::ComponentTree;
use std::collections::BTreeMap;

fn node(id: &str, offset: i32, created_at_ns: u64) -> Component {
    Component {
        id: id.to_string(),
        subtype: Some("text_content".to_string()),
        parent_id: None,
        role: None,
        offset,
        pact_depth: None,
        priority: 0,
        transaction_index: 0,
        created_at_ns,
        created_at_iso: None,
        creation_index: 0,
        lifecycle: crate::components::ComponentLifecycle::default(),
        external_refs: Vec::new(),
        key: None,
        tags: Vec::new(),
        aux: BTreeMap::new(),
        kind: crate::components::ComponentKind::Text(crate::components::TextPayload {
            content: Some(id.to_string()),
            ..Default::default()
        }),
        provenance: None,
        children: vec![],
    }
}

#[test]
fn renders_xml_safely() {
    let mut root = node("a&b", 0, 0);
    root.kind = crate::components::ComponentKind::Text(crate::components::TextPayload {
        content: Some("<hello & goodbye>".to_string()),
        ..Default::default()
    });
    let xml = render_xml(&normalize_tree(root));
    assert!(xml.contains("id=\"a&amp;b\""));
    assert!(xml.contains("&lt;hello &amp; goodbye&gt;"));
}

#[test]
fn native_component_xml_matches_normalized_adapter() {
    let mut root = node("root", 0, 0);
    root.kind = crate::components::ComponentKind::Root;

    let mut child_b = node("b", 1, 20);
    let mut b_attrs = BTreeMap::new();
    b_attrs.insert("data-id".to_string(), Value::String("b&b".to_string()));
    child_b.kind = crate::components::ComponentKind::Unknown(crate::components::UnknownPayload {
        component_type: "b".to_string(),
        payload: BTreeMap::new(),
        attrs: b_attrs,
    });
    let child_a = node("a", 0, 10);
    root.children = vec![child_b, child_a];

    let native = render_component_xml(&root);
    let adapter = render_xml(&normalize_tree(root));

    assert_eq!(native, adapter);
    assert!(native.contains("data-id=\"b&amp;b\""));
}

#[test]
fn render_tree_xml_uses_component_tree_root() {
    let root = node("root", 0, 0);
    let tree = ComponentTree::from_root(root).expect("tree");

    let xml = render_tree_xml(&tree);

    assert!(xml.starts_with("<text_content"));
    assert!(xml.contains("id=\"root\""));
}
