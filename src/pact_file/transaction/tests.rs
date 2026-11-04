use std::collections::BTreeMap;

use super::PactTransaction;
use crate::PactErrorKind;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;

fn public_trace(replay_trace: TransactionTrace) -> PactTransactionTrace {
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(1),
        transaction_id: Some("tx_1".to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        transaction_id: "tx_1".to_string(),
        authored_ref: pact_ref.clone(),
        pre_request_ref: pact_ref.clone(),
        post_request_alteration_ref: pact_ref,
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: None,
        alteration_ids: Vec::new(),
        commit: None,
        replay_trace: Some(replay_trace),
        aux: BTreeMap::new(),
    }
}

#[test]
fn canonical_transaction_keeps_replay_ops_on_one_spine() {
    let trace = TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: Vec::new(),
    };
    let transaction = PactTransaction::new(
        "tx_1".to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        trace.clone(),
        public_trace(trace.clone()),
        Vec::new(),
    );

    assert_eq!(transaction.trace, trace);
    assert!(transaction.public_trace.replay_trace.is_none());
}

fn sample_transaction_for_storage(
    transaction_id: &str,
    from_rev: u64,
    to_rev: u64,
) -> PactTransaction {
    let trace = TransactionTrace {
        from_rev,
        to_rev,
        ops: Vec::new(),
    };
    let mut public_trace = public_trace(trace.clone());
    public_trace.transaction_id = transaction_id.to_string();
    for pact_ref in [
        &mut public_trace.authored_ref,
        &mut public_trace.pre_request_ref,
        &mut public_trace.post_request_alteration_ref,
    ] {
        pact_ref.revision = Some(to_rev);
        pact_ref.transaction_id = Some(transaction_id.to_string());
    }
    PactTransaction::new(
        transaction_id.to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        trace,
        public_trace,
        Vec::new(),
    )
}

fn sample_transaction() -> PactTransaction {
    sample_transaction_for_storage("tx_1", 0, 1)
}

#[test]
fn canonical_transaction_rejects_identity_and_revision_mismatches() {
    let mut transaction = sample_transaction();
    transaction.transaction_id.clear();
    assert_eq!(
        transaction
            .validate()
            .expect_err("empty transaction id")
            .kind(),
        PactErrorKind::Invalid
    );

    let mut transaction = sample_transaction();
    transaction.trace.to_rev = 2;
    assert_eq!(
        transaction
            .validate()
            .expect_err("non-contiguous revision")
            .kind(),
        PactErrorKind::Invalid
    );

    let mut transaction = sample_transaction();
    transaction.public_trace.branch_id = "other".to_string();
    assert_eq!(
        transaction
            .validate()
            .expect_err("public trace identity mismatch")
            .kind(),
        PactErrorKind::Invalid
    );
}

#[test]
fn canonical_transaction_rejects_conflicting_duplicate_content_ids() {
    let mut transaction = sample_transaction();
    transaction.new_content = vec![
        crate::store::ContentRecord {
            id: "cnt_same".to_string(),
            kind: "text".to_string(),
            hash: "hash_a".to_string(),
            bytes: 1,
            chars: Some(1),
            preview: "a".to_string(),
            storage: crate::store::ContentStorageRef {
                kind: "inline".to_string(),
                inline: Some("a".to_string()),
                inline_bytes: None,
                path: None,
                external_ref: None,
            },
        },
        crate::store::ContentRecord {
            id: "cnt_same".to_string(),
            kind: "text".to_string(),
            hash: "hash_b".to_string(),
            bytes: 1,
            chars: Some(1),
            preview: "b".to_string(),
            storage: crate::store::ContentStorageRef {
                kind: "inline".to_string(),
                inline: Some("b".to_string()),
                inline_bytes: None,
                path: None,
                external_ref: None,
            },
        },
    ];

    assert_eq!(
        transaction
            .validate()
            .expect_err("conflicting content id")
            .kind(),
        PactErrorKind::Invalid
    );
}

#[test]
fn canonical_transaction_validates_replay_against_component_trees() {
    let before = crate::tree::ComponentTree::from_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": []
    }))
    .expect("before tree");
    let after = before.clone();
    let transaction = sample_transaction();

    transaction
        .validate_against(&before, &after)
        .expect("empty trace preserves tree");

    let different = crate::tree::ComponentTree::from_value(&serde_json::json!({
        "id": "different",
        "componentType": "root",
        "children": []
    }))
    .expect("different tree");
    assert_eq!(
        transaction
            .validate_against(&before, &different)
            .expect_err("replay mismatch")
            .kind(),
        PactErrorKind::Invalid
    );
}

#[test]
fn genesis_snapshot_is_revision_zero_without_a_public_commit() {
    let ledger = serde_json::json!({
        "schema": "pact.ledger.v1",
        "root_id": "root",
        "current_branch_id": "main"
    });

    let transaction = PactTransaction::genesis(
        "genesis_root".to_string(),
        "root".to_string(),
        "main".to_string(),
        0,
        ledger.clone(),
    );

    transaction.validate().expect("valid genesis snapshot");
    assert!(transaction.is_genesis());
    assert_eq!(transaction.trace.from_rev, 0);
    assert_eq!(transaction.trace.to_rev, 0);
    assert_eq!(transaction.head_checkpoint, Some(ledger));
}
