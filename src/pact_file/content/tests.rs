use super::ContentSpine;
use crate::PactErrorKind;
use crate::store::{ContentRecord, ContentStorageRef};

fn record(id: &str, kind: &str, body: &str) -> ContentRecord {
    ContentRecord {
        id: id.to_string(),
        kind: kind.to_string(),
        hash: crate::store::stable_content_hash(kind, body.as_bytes()),
        bytes: body.len(),
        chars: Some(body.chars().count()),
        preview: body.to_string(),
        storage: ContentStorageRef {
            kind: "inline".to_string(),
            inline: Some(body.to_string()),
            inline_bytes: None,
            path: None,
            external_ref: None,
        },
    }
}

#[test]
fn exact_payload_dedup_writes_one_body_and_preserves_public_ids() {
    let mut spine = ContentSpine::default();
    let first = spine
        .insert(record("cnt_first", "node.content", "same body"))
        .expect("insert first");
    let second = spine
        .insert(record("cnt_second", "other.content", "same body"))
        .expect("insert second ref");

    assert!(first.payload_written);
    assert!(!second.payload_written);
    assert_eq!(first.full_hash, second.full_hash);
    assert_eq!(spine.payload_count(), 1);
    assert_eq!(spine.record("cnt_first").expect("first").id, "cnt_first");
    assert_eq!(spine.record("cnt_second").expect("second").id, "cnt_second");
}

#[test]
fn reused_content_id_with_different_body_is_rejected() {
    let mut spine = ContentSpine::default();
    spine
        .insert(record("cnt_same", "node.content", "first"))
        .expect("insert first");
    let error = spine
        .insert(record("cnt_same", "node.content", "second"))
        .expect_err("conflicting id");
    assert_eq!(error.kind(), PactErrorKind::Invalid);
}
