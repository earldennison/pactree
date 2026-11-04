use super::*;

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
fn normalizes_children_by_col_tuple() {
    let root = Component {
        id: "root".to_string(),
        subtype: None,
        parent_id: None,
        role: None,
        offset: 0,
        pact_depth: None,
        priority: 0,
        transaction_index: 0,
        created_at_ns: 0,
        created_at_iso: None,
        creation_index: 0,
        lifecycle: crate::components::ComponentLifecycle::default(),
        external_refs: Vec::new(),
        key: None,
        tags: Vec::new(),
        aux: BTreeMap::new(),
        kind: crate::components::ComponentKind::default(),
        provenance: None,
        children: vec![
            node("later", 0, 20),
            node("pre", -1, 30),
            node("early", 0, 10),
        ],
    };

    let normalized = normalize_tree(root);
    let ids: Vec<&str> = normalized
        .children
        .iter()
        .map(|child| child.id.as_str())
        .collect();
    assert_eq!(ids, vec!["pre", "early", "later"]);
}

#[test]
fn normalizes_to_node_table_schema() {
    let pact = r#"{
          "id": "root",
          "componentType": "root",
          "content": "short",
          "children": [
            {
              "id": "msg_1",
              "componentType": "message_turn",
              "role": "user",
              "transaction_index": 12,
              "content": "abcdefghijklmnopqrstuvwxyz"
            }
          ]
        }"#;

    let out = normalize_pact_json(pact, 8).expect("normalizes pact");
    let value: Value = serde_json::from_str(&out).expect("json output");

    assert_eq!(value["schema"], NORMALIZED_SCHEMA);
    assert_eq!(value["root_id"], "root");
    assert_eq!(value["stats"]["component_count"], 2);
    assert_eq!(value["stats"]["edge_count"], 1);
    assert_eq!(value["stats"]["blob_count"], 1);
    assert_eq!(value["components"]["root"]["content"], "short");
    assert_eq!(value["components"]["root"]["child_ids"][0], "msg_1");
    assert_eq!(value["components"]["msg_1"]["attrs"]["role"], "user");
    assert_eq!(
        value["components"]["msg_1"]["attrs"]["transaction_index"],
        12
    );
    assert!(value["components"]["msg_1"]["attrs"].get("cycle").is_none());
    assert_eq!(value["components"]["msg_1"]["content_ref"], "blob_00000000");
    assert_eq!(value["components"]["msg_1"]["content_preview"], "abcdefgh");
    assert_eq!(value["blobs"]["blob_00000000"]["chars"], 26);
}

#[test]
fn normalize_rejects_removed_cycle_field() {
    let pact = r#"{
          "id": "root",
          "componentType": "root",
          "children": [
            {
              "id": "msg_1",
              "componentType": "message_turn",
              "cycle": 12
            }
          ]
        }"#;

    let err = normalize_pact_json(pact, 8).expect_err("cycle must not be shimmed");
    assert!(err.to_string().contains("removed PACT field"));
    assert!(err.to_string().contains("transaction_index"));
}

#[test]
fn normalize_rejects_removed_snapshot_fields() {
    let pact = r#"{
          "id": "root",
          "componentType": "root",
          "snapshots": []
        }"#;

    let err = normalize_pact_json(pact, 8).expect_err("snapshots must not be shimmed");
    assert!(err.to_string().contains("removed PACT snapshot field"));
    assert!(err.to_string().contains("transaction traces"));
}
