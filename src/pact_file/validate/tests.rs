use super::{fast_validate, full_validate};
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::log::commit_with_fault;
use crate::pact_file::log::create;
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;
use std::collections::BTreeMap;

fn genesis() -> PactTransaction {
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(1),
        transaction_id: Some("tx_1".to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransaction::new(
        "tx_1".to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: 0,
            to_rev: 1,
            ops: Vec::new(),
        },
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
            replay_trace: None,
            aux: BTreeMap::new(),
        },
        Vec::new(),
    )
}

#[test]
fn fast_and_full_validation_accept_complete_candidate_file() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("candidate.pact");
    create(&path, &genesis(), &IoCounterSet::default(), None).expect("create");

    assert!(fast_validate(&path).expect("fast validation").ok);
    assert!(full_validate(&path).expect("full validation").ok);
}

#[test]
fn full_validation_replays_every_indexed_generation() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("candidate.pact");
    let counters = IoCounterSet::default();
    create(&path, &genesis(), &counters, None).expect("create");
    let mut second = genesis();
    second.transaction_id = "tx_2".to_string();
    second.trace.from_rev = 1;
    second.trace.to_rev = 2;
    second.public_trace.transaction_id = "tx_2".to_string();
    second.public_trace.authored_ref.transaction_id = Some("tx_2".to_string());
    second.public_trace.authored_ref.revision = Some(2);
    second.public_trace.pre_request_ref = second.public_trace.authored_ref.clone();
    second.public_trace.post_request_alteration_ref = second.public_trace.authored_ref.clone();
    commit_with_fault(&path, &second, &counters, None).expect("second commit");

    let report = full_validate(&path).expect("full validation");
    assert!(report.ok, "{:?}", report.errors);
    assert_eq!(report.counters.records_decoded, 3);
}

#[test]
fn validation_reports_corruption_without_panicking() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("candidate.pact");
    create(&path, &genesis(), &IoCounterSet::default(), None).expect("create");
    let mut bytes = std::fs::read(&path).expect("read candidate");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&path, bytes).expect("corrupt candidate");

    let fast = fast_validate(&path).expect("fast validation report");
    let full = full_validate(&path).expect("full validation report");
    assert!(!fast.ok);
    assert!(!fast.errors.is_empty());
    assert!(!full.ok);
    assert!(!full.errors.is_empty());
}
