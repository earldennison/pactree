use std::collections::BTreeMap;
use std::sync::Arc;

use super::{GENERATION_PAGE_ENTRIES, TransactionStream};
use crate::pact_file::backend::{CandidateBackend, IoCounterSet, PactFileBackend};
use crate::pact_file::index::IndexKey;
use crate::pact_file::snapshot::Snapshot;
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::tests::instrumentation::PACT_TRANSACTION_DECODE_COUNT;
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;

fn transaction(id: &str, from: u64, to: u64) -> PactTransaction {
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(to),
        transaction_id: Some(id.to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransaction::new(
        id.to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: from,
            to_rev: to,
            ops: Vec::new(),
        },
        PactTransactionTrace {
            schema: TRANSACTION_TRACE_SCHEMA.to_string(),
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            transaction_id: id.to_string(),
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

fn file_with_transactions(dir: &std::path::Path, count: u64) -> std::path::PathBuf {
    let path = dir.join(format!("stream-{count}.pact"));
    let backend = CandidateBackend::default();
    backend
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");
    for n in 2..=count {
        backend
            .commit(&path, &transaction(&format!("tx_{n}"), n - 1, n))
            .expect("commit");
    }
    path
}

fn decodes() -> usize {
    PACT_TRANSACTION_DECODE_COUNT.with(std::cell::Cell::get)
}

#[test]
fn construction_decodes_nothing_and_each_next_decodes_exactly_one() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = file_with_transactions(temp.path(), 3);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("open");
    let before = decodes();
    let mut stream = TransactionStream::new(&snapshot);
    assert_eq!(decodes(), before, "construction must not decode");
    let (first, _) = stream.next().expect("first").expect("decoded");
    assert_eq!(first.transaction_id, "tx_1");
    assert_eq!(decodes(), before + 1);
    let (second, _) = stream.next().expect("second").expect("decoded");
    assert_eq!(second.transaction_id, "tx_2");
    assert_eq!(decodes(), before + 2);

    drop(stream);
    assert_eq!(decodes(), before + 2);
}

#[test]
fn keyset_pages_cover_more_generations_than_one_page_in_order() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let count = GENERATION_PAGE_ENTRIES as u64 + 5;
    let path = file_with_transactions(temp.path(), count);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("open");
    let ids = TransactionStream::new(&snapshot)
        .map(|item| item.expect("decoded").0.transaction_id)
        .collect::<Vec<_>>();
    assert_eq!(ids.len() as u64, count);
    assert_eq!(ids.first().map(String::as_str), Some("tx_1"));
    assert_eq!(ids.last().map(String::as_str), Some("tx_261"));
    assert!(ids.windows(2).all(|pair| pair[0] != pair[1]));
}

#[test]
fn a_corrupt_later_transaction_errors_only_when_reached() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = file_with_transactions(temp.path(), 3);

    let address = {
        let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("open");
        let entries = snapshot
            .index_range(
                &IndexKey::Generation { generation: 2 },
                &IndexKey::Generation { generation: 3 },
                1,
                false,
            )
            .expect("generation 2 entry");
        let raw: [u8; 8] = entries[0].1.as_slice().try_into().expect("address width");
        u64::from_le_bytes(raw)
    };
    let mut bytes = std::fs::read(&path).expect("read file");
    let target = address as usize + 32 + 4;
    bytes[target] ^= 0xff;
    std::fs::write(&path, &bytes).expect("write damaged file");

    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("open");
    let mut stream = TransactionStream::new(&snapshot);
    let (first, _) = stream
        .next()
        .expect("first")
        .expect("intact first transaction");
    assert_eq!(first.transaction_id, "tx_1");
    assert!(stream.next().expect("second").is_err());
    assert!(stream.next().is_none(), "a failed stream stops");
}
