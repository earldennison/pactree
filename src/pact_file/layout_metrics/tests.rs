#[test]
fn metrics_partition_every_committed_byte_once() {
    let ledger = crate::pact_file::tests::fixture_ledger(12);
    let bytes = crate::pact_file::encode_file_baseline(&ledger).expect("baseline");
    let metrics = super::measure_committed_layout(&bytes).expect("metrics");
    assert_eq!(metrics.file_bytes, bytes.len() as u64);
    assert_eq!(metrics.accounted_bytes(), metrics.file_bytes);
    assert!(metrics.canonical_bytes > 0);
    assert!(metrics.facet_bytes > 0);
}

#[test]
fn v2_storage_audit_includes_exact_physical_layout() {
    let fixture = crate::pact_file::tests::saved_fixture(3);
    let audit: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_storage_audit_json(&fixture.path).expect("storage audit"),
    )
    .expect("audit JSON");
    let physical = audit
        .get("physical_layout")
        .expect("PACTBIN2 physical layout");
    assert_eq!(physical["file_bytes"], audit["file_bytes"]);
    let accounted = [
        "envelope_bytes",
        "canonical_bytes",
        "checkpoint_bytes",
        "content_bytes",
        "facet_bytes",
        "directory_bytes",
        "allocator_bytes",
        "commit_bytes",
    ]
    .into_iter()
    .map(|field| physical[field].as_u64().expect("physical byte count"))
    .sum::<u64>();
    assert_eq!(accounted, physical["file_bytes"]);
}

#[test]
fn legacy_storage_audit_does_not_claim_v2_physical_layout() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/unsupported/current-pre-v0.1-minimal.pact");
    let audit: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_storage_audit_json(&path).expect("pre-v0.1 storage audit"),
    )
    .expect("audit JSON");

    assert_eq!(audit["schema"], "pact.file.v0.storage_audit.v1");
    assert!(audit.get("physical_layout").is_none());
}

#[test]
fn saved_fixture_exposes_real_externalized_content() {
    let fixture = crate::pact_file::tests::saved_fixture(1);
    let handle = crate::PactFileReadHandle::open(&fixture.path, None).expect("read handle");

    let payload = handle
        .content_payload(&fixture.content_id)
        .expect("fixture content payload");

    assert_eq!(payload["content_id"], fixture.content_id);
    assert!(
        payload["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
}

#[test]
#[ignore = "requires PACTREE_LENSKEEPER_ARTIFACT evidence path"]
fn lenskeeper_artifact_has_no_dangling_cold_component_pages() {
    let path = std::path::Path::new(
        &std::env::var("PACTREE_LENSKEEPER_ARTIFACT").expect("Lenskeeper artifact path"),
    )
    .to_path_buf();
    let ledger = crate::pact_file::open_ledger(&path).expect("canonical Lenskeeper ledger");
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        &path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )
    .expect("cold snapshot");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let branch = &ledger.branches["main"];
    let mut found = 0;
    for component_id in branch.current.tree.index.by_id.keys() {
        if snapshot
            .component("main", component_id)
            .unwrap_or_else(|error| panic!("cold component {component_id}: {error}"))
            .is_some()
        {
            found += 1;
        }
    }
    assert!(found > 0);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
fn every_op_fixture_is_a_valid_real_transaction() {
    let transaction = crate::pact_file::tests::transaction_with_every_op();

    transaction.validate().expect("valid every-op transaction");
    assert_eq!(transaction.trace.ops.len(), 10);
    assert_eq!(transaction.new_content.len(), 1);
}

#[test]
fn mutation_fixture_commits_insert_update_and_delete() {
    let fixture = crate::pact_file::tests::fixture_with_text_updates_and_delete();
    let ledger = crate::pact_file::open_ledger(&fixture.path).expect("mutation fixture ledger");

    assert_eq!(ledger.branches["main"].head_revision, 3);
    assert!(
        !ledger.branches["main"]
            .current
            .tree
            .index
            .by_id
            .contains_key("fixture_mutation_text")
    );
    assert!(ledger.content.records.contains_key(&fixture.content_id));
}

#[test]
fn baseline_encoder_keeps_component_overflow_pages_reachable() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("overflow-component.pact");
    let tags = (0..256)
        .map(|index| {
            blake3::hash(format!("overflow-{index}").as_bytes())
                .to_hex()
                .to_string()
        })
        .collect::<Vec<_>>();
    let mut tree = crate::ContextTree::with_id("overflow_fixture").expect("tree");
    tree.save_file(&path).expect("save");
    let trace = tree
        .insert(
            crate::Component::from_component_value(&serde_json::json!({
                "id": "overflow_component",
                "componentType": "text_content",
                "content": "payload",
                "tags": tags,
            }))
            .expect("overflow component"),
            "root",
        )
        .expect("insert overflow component");
    let component_id = trace.produced_component_ids[0].clone();
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit overflow component");

    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        &path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )
    .expect("cold snapshot");
    let indexed = snapshot
        .component("main", &component_id)
        .expect("cold component lookup")
        .expect("indexed component");

    assert_eq!(indexed.value["id"], component_id);
}
