use super::*;
use serde_json::json;

#[test]
fn id_selector_accepts_scoped_context_tree_ids() {
    let tree = json!({
        "id": "session",
        "componentType": "root",
        "children": [
            {
                "id": "session:abc123ef",
                "componentType": "text_content",
                "content": "scoped"
            }
        ]
    });

    let selected = select_components(&tree, "#session:abc123ef", false).expect("select");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "session:abc123ef");
}

#[test]
fn id_selector_keeps_pseudos_after_unscoped_ids() {
    let tree = json!({
        "id": "session",
        "componentType": "root",
        "children": [
            {
                "id": "plain",
                "componentType": "text_content",
                "content": "plain"
            }
        ]
    });

    let selected = select_components(&tree, "#plain:first-child", false).expect("select");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "plain");
}

#[test]
fn id_selector_accepts_non_hex_scoped_local_ids() {
    let tree = json!({
        "id": "session",
        "componentType": "root",
        "children": [
            {
                "id": "session:victim",
                "componentType": "text_content",
                "content": "named"
            }
        ]
    });

    let selected = select_components(&tree, "#session:victim", false).expect("select");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "session:victim");
}

#[test]
fn id_selector_pseudo_keywords_stay_pseudos_on_scoped_lookalikes() {
    let tree = json!({
        "id": "session",
        "componentType": "root",
        "children": [
            {
                "id": "plain",
                "componentType": "text_content",
                "content": "plain"
            }
        ]
    });

    let selected = select_components(&tree, "#plain:first", false).expect("select");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "plain");
}
