use std::collections::BTreeMap;

use super::{CheckpointPolicy, decode_checkpoint};
use crate::tree::ComponentTree;

#[test]
fn checkpoint_cadence_changes_replay_cost_not_semantic_identity() {
    let tree = ComponentTree::from_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text",
            "componentType": "text_content",
            "content_ref": {"id": "cnt_1"},
            "children": []
        }]
    }))
    .expect("component tree");
    let policy = CheckpointPolicy::new(2).expect("policy");
    assert!(
        policy
            .capture("main", 1, &tree, &BTreeMap::new(), &BTreeMap::new())
            .expect("revision one")
            .is_none()
    );
    let encoded = policy
        .capture("main", 2, &tree, &BTreeMap::new(), &BTreeMap::new())
        .expect("revision two")
        .expect("checkpoint");
    let checkpoint = decode_checkpoint(&encoded).expect("decode checkpoint");

    assert_eq!(checkpoint.branch_id, "main");
    assert_eq!(checkpoint.revision, 2);
    assert_eq!(checkpoint.tree, tree.project_value());
    assert!(checkpoint.tree.to_string().contains("content_ref"));
    assert!(!checkpoint.tree.to_string().contains("\"content\":"));

    let mut corrupt = encoded;
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0xff;
    assert!(decode_checkpoint(&corrupt).is_err());
}
