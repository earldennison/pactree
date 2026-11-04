use super::{encode_empty_header, encode_root_slot, select_root_slot};
use crate::PactErrorKind;
use crate::pact_file::codec::encode_transaction;
use crate::pact_file::envelope::RootSlot;
use crate::pact_file::layout::{HEADER_BYTES, ROOT_SLOT_START};
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;
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
fn highest_fully_valid_root_slot_wins() {
    let first = sample_transaction("tx_1", 0, 1);
    let second = sample_transaction("tx_2", 1, 2);
    let first = encode_transaction(&first, 1, HEADER_BYTES as u64).expect("first");
    let second = encode_transaction(&second, 2, first.committed_len).expect("second");
    let mut file = encode_empty_header(*blake3::hash(b"root-slot-test").as_bytes()).to_vec();
    file.extend_from_slice(&first.bytes);
    file.extend_from_slice(&second.bytes);
    let first_slot = RootSlot::from_encoded(1, &first);
    let second_slot = RootSlot::from_encoded(2, &second);
    file[ROOT_SLOT_START..ROOT_SLOT_START + 128].copy_from_slice(&encode_root_slot(&first_slot));
    file[ROOT_SLOT_START + 128..ROOT_SLOT_START + 256]
        .copy_from_slice(&encode_root_slot(&second_slot));

    let selected = select_root_slot(&file).expect("select latest");
    assert_eq!(selected.slot_index, 1);
    assert_eq!(selected.slot, second_slot);
}

#[test]
fn torn_newer_slot_or_marker_falls_back_to_previous_generation() {
    let transaction = sample_transaction("tx_1", 0, 1);
    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = encode_empty_header(*blake3::hash(b"root-slot-fallback").as_bytes()).to_vec();
    file.extend_from_slice(&encoded.bytes);
    let valid = RootSlot::from_encoded(1, &encoded);
    file[ROOT_SLOT_START..ROOT_SLOT_START + 128].copy_from_slice(&encode_root_slot(&valid));
    let mut torn = encode_root_slot(&RootSlot {
        generation: 2,
        ..valid
    });
    torn[40] ^= 0xff;
    file[ROOT_SLOT_START + 128..ROOT_SLOT_START + 256].copy_from_slice(&torn);

    let selected = select_root_slot(&file).expect("fall back");
    assert_eq!(selected.slot, valid);

    file[encoded.commit_marker_address as usize] ^= 0xff;
    let error = select_root_slot(&file).expect_err("marker corruption invalidates only slot");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn selected_root_binds_directory_and_allocator_records() {
    let transaction = sample_transaction("tx_metadata", 0, 1);
    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = encode_empty_header(*blake3::hash(b"root-slot-metadata").as_bytes()).to_vec();
    file.extend_from_slice(&encoded.bytes);
    let slot = RootSlot::from_encoded(1, &encoded);
    file[ROOT_SLOT_START..ROOT_SLOT_START + 128].copy_from_slice(&encode_root_slot(&slot));

    let selected = select_root_slot(&file).expect("select metadata root");
    assert_eq!(
        selected.slot.segment_directory_address,
        selected.marker.segment_directory_address
    );
    assert_eq!(
        selected.slot.allocator_address,
        selected.marker.allocator_address
    );
    assert!(selected.slot.segment_directory_len > 0);
    assert!(selected.slot.allocator_len > 0);

    file[selected.slot.allocator_address as usize + 32] ^= 0xff;
    assert!(select_root_slot(&file).is_err());
}
