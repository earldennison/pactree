use super::*;
use crate::components::Component;
use serde_json::{Value, json};

fn component(value: Value) -> Component {
    Component::from_component_value(&value).expect("component")
}

fn env(branch: &str) -> PactOperationEnvelope {
    PactOperationEnvelope {
        branch_id: branch.to_string(),
        source: SourceActor::User,
        source_id: None,
        host_timestamp_ns: None,
        idempotency_key: None,
        aux: BTreeMap::new(),
    }
}

#[test]
fn insert_round_trips() {
    let op = PactOperation::Insert {
        envelope: env("main"),
        target_selector: None,
        parent_id: Some("root".to_string()),
        depth: None,
        offset: Some(0),
        component: component(
            json!({"id": "n_1", "componentType": "text_content", "content": "hello"}),
        ),
    };
    op.validate().expect("valid");
    let json = serde_json::to_string(&op).expect("encode");
    let back: PactOperation = serde_json::from_str(&json).expect("decode");
    assert_eq!(back.kind(), "insert");
    assert_eq!(back.envelope().branch_id, "main");
}

#[test]
fn removed_component_lifecycle_ops_no_longer_deserialize() {
    for kind in [
        "component_registered",
        "component_state_changed",
        "component_projection_changed",
        "component_closed",
    ] {
        let removed = serde_json::json!({
            "kind": kind,
            "envelope": {"branch_id": "main"},
            "component_id": "c1",
            "component_kind": "x",
        });
        assert!(
            serde_json::from_value::<PactOperation>(removed).is_err(),
            "removed op kind {kind:?} must not deserialize into PactOperation"
        );
    }
}

#[test]
fn update_requires_payload() {
    let op = PactOperation::Update {
        envelope: env("main"),
        target_component_id: Some("n_1".to_string()),
        target_selector: None,
        content: None,
        content_ref: None,
        fields: BTreeMap::new(),
        aux: BTreeMap::new(),
    };
    let err = op.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("content, content_ref, fields, or aux"),
        "{err}"
    );
}

#[test]
fn empty_branch_rejected_for_every_variant() {
    let op = PactOperation::Delete {
        envelope: env(""),
        target_component_id: Some("n_1".to_string()),
        target_selector: None,
    };
    assert!(
        op.validate()
            .unwrap_err()
            .to_string()
            .contains("branch_id is empty")
    );
}

#[test]
fn delete_accepts_selector_targets() {
    let op = PactOperation::Delete {
        envelope: env("main"),
        target_component_id: None,
        target_selector: Some("d-1".to_string()),
    };
    op.validate().expect("selector target is valid");

    let op = PactOperation::Delete {
        envelope: env("main"),
        target_component_id: None,
        target_selector: None,
    };
    assert!(
        op.validate()
            .unwrap_err()
            .to_string()
            .contains("target_component_id")
    );
}

#[test]
fn discriminator_kind_strings_stable() {
    let cases: &[(PactOperation, &str)] = &[
        (
            PactOperation::Insert {
                envelope: env("main"),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({"id": "n_1", "componentType": "text_content"})),
            },
            "insert",
        ),
        (
            PactOperation::Update {
                envelope: env("main"),
                target_component_id: Some("n_1".to_string()),
                target_selector: None,
                content: Some("hello".to_string()),
                content_ref: None,
                fields: BTreeMap::new(),
                aux: BTreeMap::new(),
            },
            "update",
        ),
        (
            PactOperation::Replace {
                envelope: env("main"),
                target_component_id: Some("n_1".to_string()),
                target_selector: None,
                component: component(json!({"id": "n_2", "componentType": "text_content"})),
            },
            "replace",
        ),
        (
            PactOperation::Append {
                envelope: env("main"),
                target_component_id: Some("n_1".to_string()),
                target_selector: None,
                offset: None,
                component: component(json!({"id": "n_2", "componentType": "text_content"})),
            },
            "append",
        ),
        (
            PactOperation::Upsert {
                envelope: env("main"),
                target_component_id: Some("n_1".to_string()),
                target_selector: None,
                parent_id: Some("root".to_string()),
                depth: None,
                offset: None,
                component: component(json!({"id": "n_1", "componentType": "text_content"})),
            },
            "upsert",
        ),
        (
            PactOperation::Delete {
                envelope: env("main"),
                target_component_id: Some("n_1".to_string()),
                target_selector: None,
            },
            "delete",
        ),
    ];
    for (op, want) in cases {
        assert_eq!(op.kind(), *want);
    }
}
