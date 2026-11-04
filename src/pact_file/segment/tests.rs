#[test]
fn baseline_is_canonical_segments_not_a_serialized_ledger() {
    let ledger = crate::pact_file::tests::fixture_ledger(40);
    let bytes = crate::pact_file::encode_file_baseline(&ledger).expect("baseline");
    let directory = super::SegmentDirectory::open(&bytes).expect("directory");

    assert_eq!(directory.branch_heads["main"], 40);
    assert_eq!(directory.segments.len(), 1);
    assert!(directory.transactions.is_empty());
    assert!(
        !bytes
            .windows(b"head_checkpoint".len())
            .any(|window| window == b"head_checkpoint")
    );
    let reopened = crate::pact_file::open_ledger_bytes(&bytes).expect("reopen");
    assert_eq!(reopened.root_id, ledger.root_id);
    assert_eq!(reopened.current_branch_id, ledger.current_branch_id);
    assert_eq!(reopened.content.records, ledger.content.records);
    assert_eq!(
        reopened.event_index.transaction_traces,
        ledger.event_index.transaction_traces
    );
    assert_eq!(reopened.branches["main"].head_revision, 40);
    assert_eq!(
        reopened.branches["main"].base_ref,
        ledger.branches["main"].base_ref
    );
}

#[test]
fn ordinary_baselines_are_the_canonical_cold_spine() {
    let ledger = crate::pact_file::tests::fixture_ledger(40);

    let ordinary = crate::pact_file::encode_file_baseline(&ledger).expect("ordinary baseline");
    let canonical =
        crate::pact_file::encode_canonical_file(&ledger).expect("canonical cold baseline");

    let identity_start = crate::pact_file::layout::FILE_IDENTITY_START;
    let identity_end = identity_start + crate::pact_file::layout::FILE_IDENTITY_BYTES;
    assert_eq!(ordinary[..identity_start], canonical[..identity_start]);
    assert_eq!(ordinary[identity_end..], canonical[identity_end..]);
}
