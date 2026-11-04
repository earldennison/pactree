use std::collections::BTreeMap;

use super::{CandidateBackend, IoCounterSet, IoCounters, PactFileBackend};
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;

fn sample_transaction(transaction_id: &str, from_rev: u64, to_rev: u64) -> PactTransaction {
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(to_rev),
        transaction_id: Some(transaction_id.to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransaction::new(
        transaction_id.to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev,
            to_rev,
            ops: Vec::new(),
        },
        PactTransactionTrace {
            schema: TRANSACTION_TRACE_SCHEMA.to_string(),
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            transaction_id: transaction_id.to_string(),
            authored_ref: pact_ref.clone(),
            pre_request_ref: pact_ref.clone(),
            post_request_alteration_ref: pact_ref,
            request_ref: None,
            response_ref: None,
            post_response_alteration_ref: None,
            post_response_ref: None,
            alteration_ids: Vec::new(),
            commit: None,
            replay_trace: None,
            aux: BTreeMap::new(),
        },
        Vec::new(),
    )
}

#[test]
fn io_counter_set_accumulates_one_typed_delta() {
    let counters = IoCounterSet::default();
    counters.record(&IoCounters {
        bytes_read: 11,
        bytes_written: 13,
        pages_read: 2,
        pages_written: 3,
        records_decoded: 5,
        operations_replayed: 7,
        rows_materialized: 17,
        mappings_touched: 19,
        components_hydrated: 23,
        content_ranges_hydrated: 29,
    });

    assert_eq!(
        counters.snapshot(),
        IoCounters {
            bytes_read: 11,
            bytes_written: 13,
            pages_read: 2,
            pages_written: 3,
            records_decoded: 5,
            operations_replayed: 7,
            rows_materialized: 17,
            mappings_touched: 19,
            components_hydrated: 23,
            content_ranges_hydrated: 29,
        }
    );
}

#[test]
fn repeated_ledger_opens_share_one_immutable_allocation() {
    let temp = tempfile::NamedTempFile::new().expect("temporary Pact file");
    let ledger = crate::store::bootstrap_ledger("shared_open_root", Some("main"), 1024)
        .expect("bootstrap ledger");
    let bytes = crate::pact_file::encode_file_baseline(&ledger).expect("encode baseline");
    std::fs::write(temp.path(), bytes).expect("write baseline");

    let first = crate::pact_file::open_shared_ledger(temp.path()).expect("first open");
    let second = crate::pact_file::open_shared_ledger(temp.path()).expect("second open");

    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn candidate_backend_publishes_one_generation_per_transaction() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("candidate.pact");
    let backend = CandidateBackend::default();
    let first = sample_transaction("tx_1", 0, 1);
    let second = sample_transaction("tx_2", 1, 2);

    let first_receipt = backend.create(&path, &first).expect("create candidate");
    let second_receipt = backend.commit(&path, &second).expect("commit candidate");
    let snapshot = backend.open_snapshot(&path).expect("open candidate");
    let validation = backend.validate(&path).expect("validate candidate");

    assert_eq!(first_receipt.generation, 1);
    assert_eq!(second_receipt.generation, 2);
    assert_eq!(snapshot.generation(), 2);
    assert_eq!(
        snapshot.latest_transaction().expect("latest transaction"),
        second
    );
    assert!(validation.ok, "{:?}", validation.errors);
}
