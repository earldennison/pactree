use super::{
    SymbolTable, decode_standalone_core, decode_transaction, encode_standalone_core,
    encode_transaction,
};

#[test]
fn compact_transaction_round_trips_all_public_semantics() {
    let transaction = crate::pact_file::tests::transaction_with_every_op();
    let mut symbols = SymbolTable::default();
    let encoded = encode_transaction(&transaction, &mut symbols).expect("encode");
    let decoded = decode_transaction(&encoded, &symbols).expect("decode");

    assert_eq!(decoded, transaction);
    assert!(encoded.len() < serde_json::to_vec(&transaction.trace).unwrap().len());
}

#[test]
fn compact_transaction_rejects_unknown_version_and_overrun() {
    let symbols = SymbolTable::default();
    assert!(decode_transaction(&[0xff], &symbols).is_err());
    assert!(decode_transaction(b"PCTX\x01\x80", &symbols).is_err());
}

#[test]
fn compact_transaction_round_trips_genesis_metadata_and_every_boundary() {
    let ledger = crate::pact_file::tests::fixture_ledger(4);
    let value = serde_json::to_value(&ledger).expect("ledger value");
    let transaction = crate::pact_file::transaction::PactTransaction::genesis(
        "genesis_compact".to_string(),
        ledger.root_id.clone(),
        ledger.current_branch_id.clone(),
        ledger.branches[&ledger.current_branch_id].head_revision,
        value,
    );
    let boundaries = [
        crate::trace::BoundaryKind::Authored,
        crate::trace::BoundaryKind::PreRequest,
        crate::trace::BoundaryKind::PostRequestAlteration,
        crate::trace::BoundaryKind::Request,
        crate::trace::BoundaryKind::Response,
        crate::trace::BoundaryKind::PostResponseAlteration,
        crate::trace::BoundaryKind::PostResponse,
        crate::trace::BoundaryKind::Adhoc,
    ];
    let mut symbols = SymbolTable::default();
    for boundary in boundaries {
        let mut expected = transaction.clone();
        expected.boundary = boundary;
        let encoded = encode_transaction(&expected, &mut symbols).expect("encode boundary");
        assert_eq!(
            decode_transaction(&encoded, &symbols).expect("decode boundary"),
            expected
        );
    }
}

#[test]
fn compact_transaction_rejects_unknown_symbol_and_section_bomb() {
    let transaction = crate::pact_file::tests::transaction_with_every_op();
    let mut symbols = SymbolTable::default();
    let encoded = encode_transaction(&transaction, &mut symbols).expect("encode");
    assert!(decode_transaction(&encoded, &SymbolTable::default()).is_err());

    let mut forged = b"PCTX\x01\x00\x00\x07\x00\x00".to_vec();
    forged.extend_from_slice(&[0xff; 10]);
    assert!(decode_transaction(&forged, &symbols).is_err());
}

#[test]
fn standalone_core_round_trips_without_duplicating_content() {
    let transaction = crate::pact_file::tests::transaction_with_every_op();
    assert!(!transaction.new_content.is_empty());

    let encoded = encode_standalone_core(&transaction).expect("encode standalone core");
    let decoded = decode_standalone_core(&encoded).expect("decode standalone core");

    let mut expected = transaction;
    expected.new_content.clear();
    expected.head_metadata = None;
    assert_eq!(decoded, expected);
    assert!(
        encoded
            .windows(b"PCTX".len())
            .any(|window| window == b"PCTX")
    );
}
