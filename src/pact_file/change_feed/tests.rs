#[test]
fn text_change_feed_is_offset_independent_incremental_and_nonhydrating() {
    let fixture = crate::pact_file::tests::fixture_with_text_updates_and_delete();
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        &fixture.path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )
    .expect("open indexed snapshot");
    assert!(
        snapshot
            .has_text_change_facet()
            .expect("read text-change facet marker")
    );
    assert!(
        !snapshot
            .text_changes("main", None, 1)
            .expect("read bounded indexed text change")
            .is_empty()
    );
    crate::tests::instrumentation::reset_ledger_hydration_count();

    let first = super::stream_text_changes(&fixture.path, None, 2).expect("first page");
    assert_eq!(first.changes.len(), 2);
    assert!(
        first
            .changes
            .iter()
            .filter(|change| !change.tombstone)
            .all(|change| change.text_hash.len() == 64)
    );
    let cursor = first.next_cursor.clone().expect("next cursor");
    assert!(first.changes.iter().all(|change| {
        serde_json::to_value(change)
            .expect("change JSON")
            .get("physical_offset")
            .is_none()
    }));

    let moved = fixture.path.with_extension("moved.pact");
    std::fs::rename(&fixture.path, &moved).expect("move Pact file");
    let second = super::stream_text_changes(&moved, Some(&cursor), 100).expect("second page");
    assert!(second.changes.iter().any(|change| change.tombstone));
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);

    let other = crate::pact_file::tests::saved_fixture(1);
    let error = super::stream_text_changes(&other.path, Some(&cursor), 10)
        .expect_err("foreign cursor must fail");
    assert_eq!(error.kind(), crate::PactErrorKind::Invalid);
}

#[test]
fn indexed_text_change_value_is_compact_and_exact() {
    let indexed = super::IndexedTextChange {
        revision: 42,
        ordinal: 7,
        change: super::PactTextChange {
            node_id: "node-1".to_string(),
            branch_id: "main".to_string(),
            cycle: Some(42),
            field: "content".to_string(),
            text_hash: blake3::hash(b"vector text").to_hex().to_string(),
            content_ref: None,
            tombstone: false,
        },
    };

    let encoded = super::encode_index_value(&indexed).expect("encode compact change");
    assert!(encoded.len() < 16);
    let mut expected = indexed;
    expected.change.text_hash.clear();
    assert_eq!(
        super::decode_index_value(
            expected.revision,
            expected.ordinal,
            &expected.change.branch_id,
            &expected.change.node_id,
            &expected.change.field,
            &encoded,
        )
        .expect("decode compact change"),
        expected
    );
}

#[test]
fn public_text_feed_remains_the_exact_compatibility_wrapper() {
    let fixture = crate::pact_file::tests::fixture_with_text_updates_and_delete();
    let public = super::file_text_changes(&fixture.path, None, 100).expect("public feed");
    let compatibility = super::stream_text_changes(&fixture.path, None, 100).expect("compat feed");
    assert_eq!(
        serde_json::to_vec(&public).expect("public JSON"),
        serde_json::to_vec(&compatibility).expect("compat JSON")
    );
}

#[test]
#[ignore = "requires PACTREE_LENSKEEPER_ARTIFACT evidence path"]
fn lenskeeper_change_feed_stays_incremental_and_exact() {
    let path = std::path::PathBuf::from(
        std::env::var("PACTREE_LENSKEEPER_ARTIFACT").expect("set PACTREE_LENSKEEPER_ARTIFACT"),
    );
    crate::tests::instrumentation::reset_ledger_hydration_count();

    let started = std::time::Instant::now();
    let first = super::stream_text_changes(&path, None, 20).expect("first Lenskeeper page");
    let first_elapsed = started.elapsed();
    assert_eq!(first.changes.len(), 20);
    assert!(
        first
            .changes
            .iter()
            .filter(|change| !change.tombstone)
            .all(|change| change.text_hash.len() == 64)
    );
    let cursor = first.next_cursor.as_ref().expect("Lenskeeper cursor");
    let second =
        super::stream_text_changes(&path, Some(cursor), 20).expect("second Lenskeeper page");
    assert_eq!(second.changes.len(), 20);
    assert_ne!(first.changes, second.changes);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
    eprintln!("Lenskeeper first text-change page: {first_elapsed:?}");
}
