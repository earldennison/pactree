use super::{
    FaultPoint, FilePageReader, commit_checkpointed, commit_with_fault, create,
    create_checkpointed, open_snapshot,
};
use crate::PactErrorKind;
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::transaction::PactTransaction;
use crate::store::{
    ContentRecord, ContentStorageRef, PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA,
};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;
use crate::transaction_trace::{TraceOp, TraceOpKind};
use std::collections::BTreeMap;

fn sample_transaction(transaction_id: &str, from_rev: u64, to_rev: u64) -> PactTransaction {
    let trace = TransactionTrace {
        from_rev,
        to_rev,
        ops: Vec::new(),
    };
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
        trace,
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
fn create_and_commit_publish_exact_generations() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("candidate.pact");
    let counters = IoCounterSet::default();
    let first = sample_transaction("tx_1", 0, 1);
    let second = sample_transaction("tx_2", 1, 2);

    let first_receipt = create(&path, &first, &counters, None).expect("create");
    let first_snapshot = open_snapshot(&path, &counters).expect("open first");
    let second_receipt = commit_with_fault(&path, &second, &counters, None).expect("commit second");
    let second_snapshot = open_snapshot(&path, &counters).expect("open second");

    assert_eq!(first_receipt.generation, 1);
    assert_eq!(first_snapshot.generation, 1);
    assert_eq!(first_snapshot.transaction, first);
    assert_eq!(second_receipt.generation, 2);
    assert_eq!(second_snapshot.generation, 2);
    assert_eq!(second_snapshot.transaction, second);
    assert_eq!(first_snapshot.transaction.transaction_id, "tx_1");
}

#[test]
fn create_and_open_preserve_a_revision_zero_genesis_snapshot() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("genesis.pact");
    let counters = IoCounterSet::default();
    let genesis = PactTransaction::genesis(
        "genesis_root".to_string(),
        "root".to_string(),
        "main".to_string(),
        0,
        serde_json::json!({"root_id": "root", "current_branch_id": "main"}),
    );

    create(&path, &genesis, &counters, None).expect("create genesis");
    let snapshot = open_snapshot(&path, &counters).expect("open genesis");

    assert_eq!(snapshot.transaction, genesis);
    assert_eq!(snapshot.transaction.trace.to_rev, 0);
}

#[test]
fn every_commit_fault_reopens_to_exact_previous_or_next_generation() {
    for fault in [
        FaultPoint::DirectoryWritten,
        FaultPoint::AllocatorWritten,
        FaultPoint::RecordsWritten,
        FaultPoint::DataSynced,
        FaultPoint::RootSlotWritten,
        FaultPoint::RootSynced,
    ] {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join(format!("fault-{fault:?}.pact"));
        let counters = IoCounterSet::default();
        let first = sample_transaction("tx_1", 0, 1);
        let second = sample_transaction("tx_2", 1, 2);
        create(&path, &first, &counters, None).expect("create first");

        let error =
            commit_with_fault(&path, &second, &counters, Some(fault)).expect_err("injected fault");
        assert_eq!(error.kind(), PactErrorKind::Io);
        let reopened = open_snapshot(&path, &counters).expect("reopen after fault");
        assert!(reopened.generation == 1 || reopened.generation == 2);
        if reopened.generation == 1 {
            assert_eq!(reopened.transaction, first);
        } else {
            assert_eq!(reopened.transaction, second);
        }
    }
}

#[test]
fn commit_reads_header_latest_transaction_and_index_paths_not_full_history() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("bounded-commit.pact");
    create(
        &path,
        &sample_transaction("tx_1", 0, 1),
        &IoCounterSet::default(),
        None,
    )
    .expect("create");
    for revision in 2..=100 {
        commit_with_fault(
            &path,
            &sample_transaction(&format!("tx_{revision}"), revision - 1, revision),
            &IoCounterSet::default(),
            None,
        )
        .expect("grow history");
    }
    let file_bytes = std::fs::metadata(&path).expect("metadata").len();
    let counters = IoCounterSet::default();
    let receipt = commit_with_fault(
        &path,
        &sample_transaction("tx_101", 100, 101),
        &counters,
        None,
    )
    .expect("bounded commit");

    assert!(receipt.counters.bytes_read < file_bytes / 4);
    assert!(
        receipt.counters.pages_read <= 32,
        "bounded commit read {} index pages from a {} byte file",
        receipt.counters.pages_read,
        file_bytes
    );
}

#[test]
fn identical_content_payload_is_stored_once_across_transactions() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("deduplicated-content.pact");
    let payload = "pact-dedup-body-8c2f49d1";
    let mut first = sample_transaction("tx_1", 0, 1);
    first
        .new_content
        .push(content("cnt_first", "first.kind", payload));
    let mut second = sample_transaction("tx_2", 1, 2);
    second
        .new_content
        .push(content("cnt_second", "second.kind", payload));
    create(&path, &first, &IoCounterSet::default(), None).expect("create");
    commit_with_fault(&path, &second, &IoCounterSet::default(), None).expect("commit");

    let file = std::fs::read(path).expect("read file");
    assert_eq!(
        file.windows(payload.len())
            .filter(|window| *window == payload.as_bytes())
            .count(),
        1
    );
    let reopened = open_snapshot(
        &temp.path().join("deduplicated-content.pact"),
        &IoCounterSet::default(),
    )
    .expect("reopen deduplicated content");
    assert_eq!(reopened.transaction, second);
    let validation =
        crate::pact_file::validate::full_validate(&temp.path().join("deduplicated-content.pact"))
            .expect("validate deduplicated content");
    assert!(validation.ok, "{:?}", validation.errors);
}

#[test]
fn semantic_checkpoint_is_published_inside_its_transaction() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("checkpointed.pact");
    let checkpoint = crate::pact_file::checkpoint::SemanticCheckpoint {
        branch_id: "main".to_string(),
        revision: 1,
        tree: serde_json::json!({
            "id": "root",
            "componentType": "root",
            "children": []
        }),
        first_seen: BTreeMap::new(),
        lifecycle_seen: BTreeMap::new(),
    };
    create_checkpointed(
        &path,
        &sample_transaction("tx_1", 0, 1),
        &IoCounterSet::default(),
        None,
        Some(&checkpoint),
    )
    .expect("create checkpointed transaction");

    let snapshot = open_snapshot(&path, &IoCounterSet::default()).expect("open checkpoint");
    assert_eq!(snapshot.checkpoint, Some(checkpoint));
    assert!(
        crate::pact_file::validate::full_validate(&path)
            .expect("validate checkpoint")
            .ok
    );
}

#[test]
fn historical_replay_starts_at_the_scheduled_checkpoint() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("checkpoint-replay.pact");
    let counters = IoCounterSet::default();
    create(&path, &sample_transaction("tx_1", 0, 1), &counters, None).expect("create");
    let checkpoint = crate::pact_file::checkpoint::SemanticCheckpoint {
        branch_id: "main".to_string(),
        revision: 2,
        tree: serde_json::json!({
            "id": "root",
            "componentType": "root",
            "children": []
        }),
        first_seen: BTreeMap::new(),
        lifecycle_seen: BTreeMap::new(),
    };
    commit_checkpointed(
        &path,
        &sample_transaction("tx_2", 1, 2),
        &counters,
        None,
        Some(&checkpoint),
    )
    .expect("checkpoint commit");
    let mut third = sample_transaction("tx_3", 2, 3);
    third.trace.ops.push(TraceOp::new(
        "root",
        TraceOpKind::SetField {
            key: "status".to_string(),
            value: serde_json::json!("ready"),
        },
    ));
    commit_with_fault(&path, &third, &counters, None).expect("third commit");

    let file = std::fs::read(&path).expect("read checkpoint history");
    let selected = crate::pact_file::root_slot::select_root_slot(&file).expect("select root");
    let replay = crate::pact_file::checkpoint::replay_revision(
        &file,
        selected.slot.index_root_address,
        "main",
        3,
        2,
    )
    .expect("checkpoint replay");
    assert_eq!(replay.operations_replayed, 1);
    assert_eq!(replay.tree["status"], "ready");
}

fn content(id: &str, kind: &str, payload: &str) -> ContentRecord {
    ContentRecord {
        id: id.to_string(),
        kind: kind.to_string(),
        hash: crate::store::stable_content_hash(kind, payload.as_bytes()),
        bytes: payload.len(),
        chars: Some(payload.chars().count()),
        preview: String::new(),
        storage: ContentStorageRef {
            kind: "inline".to_string(),
            inline: Some(payload.to_string()),
            inline_bytes: None,
            path: None,
            external_ref: None,
        },
    }
}

#[test]
fn file_page_reader_decompresses_each_index_batch_once() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("index-batch.pact");
    let batch_address = crate::pact_file::layout::HEADER_BYTES as u64;
    let pages = vec![
        [b'a'; crate::pact_file::btree::PAGE_BYTES],
        [b'b'; crate::pact_file::btree::PAGE_BYTES],
    ];
    let record = crate::pact_file::codec::encode_index_page_batch_record(1, &pages)
        .expect("encode index batch");
    let mut file = vec![0_u8; crate::pact_file::layout::HEADER_BYTES];
    file.extend_from_slice(&record);
    std::fs::write(&path, file).expect("write index batch");
    let reader = FilePageReader::open(&path).expect("open page reader");

    for ordinal in 0..pages.len() {
        let address = crate::pact_file::codec::pack_index_page_address(batch_address, ordinal)
            .expect("pack page address");
        crate::pact_file::btree::PageReader::read_page(&reader, address)
            .expect("read batched page");
    }

    assert_eq!(reader.bytes_read.get(), record.len() as u64);
    assert_eq!(reader.pages_read.get(), 1);
}

#[test]
fn streamed_slot_selection_agrees_with_decoding_and_skips_damaged_slots() {
    use super::{select_bounded, select_committed_slot_streamed};
    use crate::pact_file::layout::{ROOT_SLOT_BYTES, ROOT_SLOT_START};
    use std::fs::File;

    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("streamed.pact");
    let counters = IoCounterSet::default();
    create(&path, &sample_transaction("tx_1", 0, 1), &counters, None).expect("create");
    let streamed = |path: &std::path::Path| {
        let mut file = File::open(path).expect("open");
        select_committed_slot_streamed(&mut file, 4096).expect("streamed selection")
    };
    let decoded = |path: &std::path::Path| {
        let mut file = File::open(path).expect("open");
        select_bounded(&mut file).expect("decoded selection")
    };
    let (selection, io) = streamed(&path);
    assert_eq!(
        selection.slot.generation, 1,
        "a genesis-only file selects its genesis"
    );
    assert!(io.bytes_read > 0);

    commit_with_fault(&path, &sample_transaction("tx_2", 1, 2), &counters, None).expect("gen 2");
    commit_with_fault(&path, &sample_transaction("tx_3", 2, 3), &counters, None).expect("gen 3");
    let (selection, _) = streamed(&path);
    let (expected, _, _) = decoded(&path);
    assert_eq!(selection.slot.generation, 3);
    assert_eq!(selection.slot_index, expected.slot_index);
    assert_eq!(selection.marker, expected.marker);

    let damaged_slot = path.with_extension("slot.pact");
    let mut bytes = std::fs::read(&path).expect("read");
    let slot_offset = ROOT_SLOT_START + selection.slot_index * ROOT_SLOT_BYTES;
    bytes[slot_offset] ^= 0xff;
    std::fs::write(&damaged_slot, &bytes).expect("write");
    let (fallback, _) = streamed(&damaged_slot);
    let (expected, _, _) = decoded(&damaged_slot);
    assert_eq!(fallback.slot.generation, 2);
    assert_eq!(fallback.slot_index, expected.slot_index);

    let damaged_body = path.with_extension("body.pact");
    let mut bytes = std::fs::read(&path).expect("read");
    let inside = selection.marker.transaction_address as usize + 40;
    bytes[inside] ^= 0x01;
    std::fs::write(&damaged_body, &bytes).expect("write");
    let (fallback, _) = streamed(&damaged_body);
    assert_eq!(
        fallback.slot.generation, 2,
        "the damaged newest region is rejected"
    );
    assert_eq!(decoded(&damaged_body).0.slot.generation, 2);
}
