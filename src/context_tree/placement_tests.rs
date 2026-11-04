use super::*;
use serde_json::json;

fn text_component(id: &str, content: &str) -> Component {
    Component::from_component_value(&json!({
        "id": id,
        "componentType": "text_content",
        "content": content
    }))
    .expect("component")
}

fn first_produced_id(trace: &TraceRef) -> String {
    trace
        .produced_component_ids
        .first()
        .cloned()
        .expect("trace should expose a produced component id")
}

#[test]
fn placement_accepts_coordinates_component_ids_and_selector_syntax() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");

    let by_depth = tree
        .insert(text_component("by_depth", "depth"), (-1, 0))
        .expect("coordinate placement");
    let by_depth_id = first_produced_id(&by_depth);
    let by_component = tree
        .insert(text_component("by_component", "component"), "root")
        .expect("component id placement");
    let by_component_id = first_produced_id(&by_component);
    let by_selector = tree
        .insert(text_component("by_selector", "selector"), "#test_context")
        .expect("selector placement");
    let by_selector_id = first_produced_id(&by_selector);

    let view = tree.view().expect("view");
    assert!(view.component(&by_depth_id).is_some());
    assert!(view.component(&by_component_id).is_some());
    assert!(view.component(&by_selector_id).is_some());
}
