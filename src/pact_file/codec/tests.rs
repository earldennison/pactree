use std::collections::BTreeMap;

use super::{
    RECORD_HEADER_BYTES, decode_any_record_generation_at, decode_content_record_at,
    decode_index_page_record, decode_transaction, encode_index_page_batch_record,
    encode_transaction, encode_transaction_checkpointed_inner, index_page_batch_address,
    index_page_ordinal, pack_index_page_address,
};
use crate::PactErrorKind;
use crate::pact_file::index::{IndexKey, read_index_value};
use crate::pact_file::layout::HEADER_BYTES;
use crate::pact_file::transaction::PactTransaction;
use crate::store::{
    ContentRecord, ContentStorageRef, PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA,
};
use crate::trace::BoundaryKind;
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

fn sample_transaction() -> PactTransaction {
    let trace = TransactionTrace {
        from_rev: 4,
        to_rev: 5,
        ops: vec![TraceOp::new(
            "root",
            TraceOpKind::SetField {
                key: "status".to_string(),
                value: serde_json::json!("committed"),
            },
        )],
    };
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(5),
        transaction_id: Some("tx_5".to_string()),
        component_id: None,
        content_ref: None,
    };
    let public_trace = PactTransactionTrace {
        schema: TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        transaction_id: "tx_5".to_string(),
        authored_ref: pact_ref.clone(),
        pre_request_ref: pact_ref.clone(),
        post_request_alteration_ref: pact_ref,
        request_ref: None,
        response_ref: None,
        post_response_alteration_ref: None,
        post_response_ref: None,
        alteration_ids: vec!["adhoc".to_string()],
        commit: None,
        replay_trace: None,
        aux: BTreeMap::new(),
    };
    PactTransaction::new(
        "tx_5".to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        trace,
        public_trace,
        vec![ContentRecord {
            id: "cnt_1".to_string(),
            kind: "node.content".to_string(),
            hash: "hash_1".to_string(),
            bytes: 5,
            chars: Some(5),
            preview: "hello".to_string(),
            storage: ContentStorageRef {
                kind: "inline".to_string(),
                inline: Some("hello".to_string()),
                inline_bytes: None,
                path: None,
                external_ref: None,
            },
        }],
    )
}

fn encode_transaction_with_legacy_content_index(
    transaction: &PactTransaction,
    generation: u64,
    transaction_address: u64,
) -> crate::PactResult<super::EncodedTransaction> {
    encode_transaction_checkpointed_inner(
        transaction,
        generation,
        transaction_address,
        None,
        None,
        None,
        true,
    )
}

#[test]
fn canonical_transaction_records_round_trip_with_one_commit_marker() {
    let transaction = sample_transaction();
    let encoded = encode_transaction(&transaction, 7, HEADER_BYTES as u64).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);

    let decoded = decode_transaction(&file, encoded.transaction_address, 7).expect("decode");

    assert_eq!(decoded.transaction, transaction);
    assert_eq!(decoded.marker.generation, 7);
    assert_eq!(decoded.marker.transaction_address, HEADER_BYTES as u64);
    assert_eq!(
        decoded.marker.index_root_address,
        encoded.index_root_address
    );
    assert_eq!(decoded.marker.file_length_watermark, encoded.committed_len);
    assert_eq!(decoded.marker_address, encoded.commit_marker_address);
    assert_eq!(
        read_index_value(
            &file,
            encoded.index_root_address,
            &IndexKey::BranchHead {
                branch: "main".to_string()
            }
        )
        .expect("read branch head"),
        Some(5_u64.to_le_bytes().to_vec())
    );
    assert_eq!(
        read_index_value(
            &file,
            encoded.index_root_address,
            &IndexKey::Revision {
                branch: "main".to_string(),
                revision: 5
            }
        )
        .expect("read revision address"),
        Some((HEADER_BYTES as u64).to_le_bytes().to_vec())
    );
}

#[test]
fn legacy_content_id_index_resolves_body_without_decoding_transaction() {
    let transaction = sample_transaction();
    let encoded =
        encode_transaction_with_legacy_content_index(&transaction, 7, HEADER_BYTES as u64)
            .expect("pre-v0.1 encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);

    let indexed = read_index_value(
        &file,
        encoded.index_root_address,
        &IndexKey::ContentId {
            id: "cnt_1".to_string(),
        },
    )
    .expect("pre-v0.1 content id")
    .expect("indexed content");
    assert_eq!(indexed.len(), 32);
    let hash: [u8; 32] = indexed.try_into().expect("content hash");
    let body_address = read_index_value(
        &file,
        encoded.index_root_address,
        &IndexKey::ContentHash { hash },
    )
    .expect("content hash index")
    .expect("content body address");
    let body_address = u64::from_le_bytes(body_address.try_into().expect("body address width"));
    assert_eq!(
        decode_any_record_generation_at(&file, body_address).expect("body generation"),
        7
    );
    let transaction_address = read_index_value(
        &file,
        encoded.index_root_address,
        &IndexKey::Generation { generation: 7 },
    )
    .expect("generation index")
    .expect("transaction address");
    let transaction_address = u64::from_le_bytes(
        transaction_address
            .try_into()
            .expect("transaction address width"),
    );

    crate::tests::instrumentation::reset_pact_transaction_decode_count();
    let content = decode_content_record_at(&file, transaction_address, "cnt_1", &hash)
        .expect("pre-v0.1 content");

    assert_eq!(content, transaction.new_content[0]);
    assert_eq!(
        crate::tests::instrumentation::pact_transaction_decode_count(),
        0
    );
}

#[test]
fn record_payload_corruption_is_a_typed_error() {
    let transaction = sample_transaction();
    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);
    file[HEADER_BYTES + RECORD_HEADER_BYTES] ^= 0xff;

    let error = decode_transaction(&file, HEADER_BYTES as u64, 1).expect_err("corrupt payload");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn forged_record_length_is_rejected_before_allocation() {
    let transaction = sample_transaction();
    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);
    let payload_len_offset = HEADER_BYTES + 16;
    file[payload_len_offset..payload_len_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());

    let error = decode_transaction(&file, HEADER_BYTES as u64, 1).expect_err("forged length");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn genesis_snapshot_round_trips_its_revision_zero_ledger() {
    let transaction = PactTransaction::genesis(
        "genesis_root".to_string(),
        "root".to_string(),
        "main".to_string(),
        0,
        serde_json::json!({"root_id": "root", "current_branch_id": "main"}),
    );
    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);

    let decoded = decode_transaction(&file, encoded.transaction_address, 1).expect("decode");

    assert_eq!(decoded.transaction, transaction);
    assert!(decoded.transaction.is_genesis());
}

#[test]
fn genesis_snapshot_is_compressed_as_one_semantic_baseline() {
    let repeated = "cold baseline payload ".repeat(32 * 1024);
    let transaction = PactTransaction::genesis(
        "genesis_root".to_string(),
        "root".to_string(),
        "main".to_string(),
        604,
        serde_json::json!({
            "root_id": "root",
            "current_branch_id": "main",
            "history": repeated,
        }),
    );

    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);
    let decoded = decode_transaction(&file, encoded.transaction_address, 1).expect("decode");

    assert_eq!(decoded.transaction, transaction);
    assert!(encoded.bytes.len() < 32 * 1024);
}

#[test]
fn genesis_snapshot_builds_the_live_component_index_once() {
    let tree = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "role": "assistant",
            "children": []
        }]
    });
    let ledger: serde_json::Value = serde_json::from_str(
        &crate::store::new_ledger_json(&tree.to_string(), Some("main"), 8).expect("new ledger"),
    )
    .expect("ledger value");
    let transaction = PactTransaction::genesis(
        "genesis_root".to_string(),
        "root".to_string(),
        "main".to_string(),
        0,
        ledger,
    );

    let encoded = encode_transaction(&transaction, 1, HEADER_BYTES as u64).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);

    let indexed = read_index_value(
        &file,
        encoded.index_root_address,
        &IndexKey::Component {
            branch: "main".to_string(),
            component: "turn".to_string(),
        },
    )
    .expect("read component");
    assert!(indexed.is_some());
}

#[test]
fn one_compressed_index_batch_round_trips_multiple_addressable_pages() {
    let batch_address = HEADER_BYTES as u64;
    let pages = vec![
        [b'a'; crate::pact_file::btree::PAGE_BYTES],
        [b'b'; crate::pact_file::btree::PAGE_BYTES],
        [b'c'; crate::pact_file::btree::PAGE_BYTES],
    ];
    let record = encode_index_page_batch_record(7, &pages).expect("encode page batch");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&record);

    assert!(record.len() < crate::pact_file::btree::PAGE_BYTES);
    for (ordinal, expected) in pages.iter().enumerate() {
        let address = pack_index_page_address(batch_address, ordinal).expect("pack page address");
        assert_eq!(index_page_batch_address(address), batch_address);
        assert_eq!(
            decode_index_page_record(&file, address).expect("decode addressed page"),
            *expected
        );
    }
}

#[test]
fn index_page_addresses_support_bounded_large_transaction_batches() {
    let batch_address = HEADER_BYTES as u64;
    let address = pack_index_page_address(batch_address, 32_767).expect("pack page 32767");

    assert_eq!(index_page_batch_address(address), batch_address);
    assert_eq!(index_page_ordinal(address), 32_767);
    assert!(pack_index_page_address(batch_address, 32_768).is_err());
}

#[test]
fn record_header_uses_a_compact_64_bit_payload_checksum() {
    assert_eq!(RECORD_HEADER_BYTES, 32);
}

#[test]
fn semantic_records_use_fast_zstd_and_round_trip_with_a_fixed_decode_bound() {
    let payload = b"repeated semantic trace payload ".repeat(256);
    let stored = super::compressed_payload(&payload).expect("compress payload");

    assert!(stored.starts_with(b"PZS2"));
    assert!(stored.len() < payload.len() / 4);
    assert_eq!(
        super::decompressed_payload(&stored).expect("decompress payload"),
        payload
    );
}

#[test]
fn semantic_record_rejects_extreme_decompression_ratio_before_large_allocation() {
    let payload = vec![b'x'; 8 * 1024 * 1024];
    let stored = super::compressed_payload(&payload).expect("compress payload");

    let error = match super::decompressed_payload(&stored) {
        Ok(value) => panic!("accepted {} decompressed bytes", value.len()),
        Err(error) => error,
    };
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn content_body_payloads_compress_independently_and_hash_decoded_bytes() {
    let payload = b"repeated tool output line\n".repeat(512);
    let (compressed, stored) =
        super::encoded_content_body_payload(&payload).expect("compress content body");

    assert!(compressed);
    assert!(stored.len() < payload.len() / 4);
    assert_eq!(
        super::decoded_content_body_payload(&stored, compressed).expect("decode content body"),
        payload
    );
}

#[test]
fn component_payload_batch_round_trips_one_addressed_payload() {
    let batch_address = HEADER_BYTES as u64;
    let entries = vec![
        (3, b"first compact component".repeat(64)),
        (17, b"second compact component".repeat(64)),
    ];
    let record = super::encode_component_payload_batch_record(7, &entries)
        .expect("encode component payload batch");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&record);

    assert!(record.len() < entries.iter().map(|(_, value)| value.len()).sum());
    assert_eq!(
        super::decode_component_payload_batch_record(&file, batch_address, 17)
            .expect("decode addressed component"),
        entries[1].1
    );
    assert!(
        super::decode_component_payload_batch_record(&file, batch_address, 9).is_err(),
        "an absent ordinal must not alias another component"
    );
}

#[test]
fn content_batch_round_trips_one_indexed_record_with_global_compression() {
    let batch_address = HEADER_BYTES as u64;
    let mut transaction = sample_transaction();
    let mut first = transaction.new_content.remove(0);
    first.storage.inline = Some("shared content vocabulary ".repeat(256));
    first.bytes = first.storage.inline.as_ref().unwrap().len();
    let mut second = first.clone();
    second.id = "cnt_2".to_string();
    let records = vec![first, second.clone()];

    let encoded = super::encode_content_batch_record(7, &records).expect("encode content batch");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded);

    assert!(encoded.len() < records.iter().map(|record| record.bytes).sum());
    assert_eq!(
        super::decode_content_batch_record_at(&file, batch_address, "cnt_2")
            .expect("decode content record"),
        second
    );
}

#[test]
fn cold_genesis_keeps_component_payloads_out_of_facet_pages() {
    let tree = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "role": "assistant",
            "children": []
        }]
    });
    let ledger: serde_json::Value = serde_json::from_str(
        &crate::store::new_ledger_json(&tree.to_string(), Some("main"), 8).expect("new ledger"),
    )
    .expect("ledger value");
    let transaction = PactTransaction::genesis(
        "genesis_root".to_string(),
        "root".to_string(),
        "main".to_string(),
        0,
        ledger,
    );
    let encoded = super::encode_transaction_checkpointed_cold(transaction, 1, HEADER_BYTES as u64)
        .expect("encode cold genesis");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&encoded.bytes);
    let locator = read_index_value(
        &file,
        encoded.index_root_address,
        &IndexKey::Component {
            branch: "main".to_string(),
            component: "turn".to_string(),
        },
    )
    .expect("read component locator")
    .expect("component locator");
    let locator = crate::pact_file::segment::RecordLocator::decode(&locator).expect("locator");

    assert!(
        read_index_value(
            &file,
            encoded.index_root_address,
            &IndexKey::ComponentPayload {
                segment_id: locator.segment_id,
                ordinal: locator.ordinal,
            },
        )
        .expect("read pre-v0.1 payload facet")
        .is_none()
    );
    let payload =
        crate::pact_file::index::read_component_payload(&file, encoded.index_root_address, locator)
            .expect("read canonical component payload");
    let component = crate::pact_file::index::IndexedComponent::decode(&payload)
        .expect("decode indexed component");
    assert_eq!(component.value["id"], "turn");
}
