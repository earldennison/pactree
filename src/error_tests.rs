use crate::error::{PactError, PactErrorKind};

fn sample_tree_json() -> String {
    r#"{"id":"root","componentType":"root","children":[
        {"id":"n1","componentType":"message","text_content":"hello"}
    ]}"#
    .to_string()
}

#[test]
fn malformed_selector_returns_invalid_not_panic() {
    let err = crate::selector::select_json(&sample_tree_json(), "$$??>>")
        .expect_err("malformed selector must error");
    assert!(
        matches!(err, PactError::Invalid(_)),
        "expected Invalid, got {err:?}"
    );
    assert_eq!(err.kind(), PactErrorKind::Invalid);
}

#[test]
fn selector_against_non_object_root_is_invalid() {
    let err = crate::selector::select_json("[1,2,3]", ".message")
        .expect_err("non-object root must error");
    assert_eq!(err.kind(), PactErrorKind::Invalid);
}

#[test]
fn malformed_mutation_request_json_is_invalid() {
    let err = crate::mutation::mutation_request_from_json("{ this is not json")
        .expect_err("bad mutation JSON must error");
    assert_eq!(err.kind(), PactErrorKind::Invalid);

    assert!(
        err.to_string()
            .contains("invalid PACT mutation request JSON")
    );
}

#[test]
fn normalize_rejects_bad_json_as_invalid() {
    let err = crate::normalize::normalize_pact_json("not json at all", 4096)
        .expect_err("bad JSON must error");
    assert_eq!(err.kind(), PactErrorKind::Invalid);
}

#[test]
fn project_missing_branch_is_not_found() {
    let root_store_json = crate::store::new_ledger_json(&sample_tree_json(), Some("main"), 4096)
        .expect("root store should build");
    let err =
        crate::store::ledger_project_branch_tree_json(&root_store_json, "does-not-exist", None)
            .expect_err("missing branch must error");
    assert!(
        matches!(err, PactError::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
    assert_eq!(err.kind(), PactErrorKind::NotFound);
}

#[test]
fn removed_packed_bytes_return_typed_unsupported_not_panic() {
    let garbage = [0xFFu8; 24];

    let err = crate::pact_pack::PackedV1::read(&garbage)
        .err()
        .expect("garbage bytes must not decode");
    assert_eq!(err.kind(), PactErrorKind::Unsupported);
}

#[test]
fn typed_leaf_errors_fold_into_pact_error_and_keep_message() {
    let bad_node = serde_json::json!({ "id": "bad" });
    let err = crate::components::Component::from_component_value(&bad_node)
        .map_err(PactError::from)
        .expect_err("node missing component type must error");
    assert!(matches!(err, PactError::NodeValidation(_)));
    assert!(
        err.to_string()
            .contains("missing string componentType/type")
    );
}
