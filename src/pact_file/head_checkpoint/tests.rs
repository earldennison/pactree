#[test]
fn head_checkpoint_contains_components_once_without_derived_maps() {
    let ledger = crate::pact_file::tests::fixture_ledger(300);
    let checkpoint = super::HeadCheckpoint::from_ledger(&ledger).expect("checkpoint");
    let bytes = super::encode(&checkpoint).expect("encode");

    assert!(
        !bytes
            .windows(b"by_id".len())
            .any(|window| window == b"by_id")
    );
    assert!(
        !bytes
            .windows(b"child_ids".len())
            .any(|window| window == b"child_ids")
    );
    assert!(
        !bytes
            .windows(b"tool_pairs".len())
            .any(|window| window == b"tool_pairs")
    );
    assert_eq!(super::decode(&bytes).expect("decode"), checkpoint);
}

#[test]
fn typed_compact_ledger_matches_the_existing_compact_shape_exactly() {
    let ledger = crate::pact_file::tests::fixture_ledger(40);
    let verbose = serde_json::to_value(&ledger).expect("verbose ledger");
    let expected = super::strip_derived_head(&verbose).expect("existing compact ledger");

    assert_eq!(
        super::compact_ledger_value(&ledger).expect("typed compact ledger"),
        expected
    );
}

#[test]
fn owned_and_borrowed_typed_hydration_produce_the_same_ledger() {
    let fixture = crate::pact_file::tests::saved_fixture(12);
    let bytes = std::fs::read(&fixture.path).expect("read fixture");

    let borrowed = crate::pact_file::open_ledger_bytes(&bytes).expect("open bytes");

    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        &fixture.path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )
    .expect("snapshot");
    let owned = snapshot.replay_ledger().expect("replay");
    assert_eq!(
        serde_json::to_value(&borrowed).expect("borrowed json"),
        serde_json::to_value(&owned).expect("owned json"),
        "owned hydration must not change the materialized ledger"
    );
}
