#[test]
fn public_range_types_are_json_stable() {
    let range = super::ContentRange {
        content_id: "cnt_1".to_string(),
        offset: 0,
        data: b"bounded".to_vec(),
        next_offset: Some(7),
    };
    let value = serde_json::to_value(range).expect("range JSON");
    assert_eq!(value["content_id"], "cnt_1");
    assert_eq!(value["next_offset"], 7);
}

#[test]
fn cold_large_content_range_touches_only_requested_chunks() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let hot = temp.path().join("large-hot.pact");
    let cold = temp.path().join("large-cold.pact");
    let mut tree = crate::ContextTree::with_id("bounded_range_root").expect("tree");
    tree.save_file(&hot).expect("save");
    let options = tree.next_authoring_component_options();
    let trace = tree
        .insert(
            crate::Component::text_authoring("large bounded payload €\n".repeat(100_000), options),
            "root",
        )
        .expect("insert");
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit");
    let component_id = &trace.produced_component_ids[0];
    let projected = tree.project_component(component_id).expect("project");
    let content_id = projected["content_ref"]["id"]
        .as_str()
        .expect("content id")
        .to_string();
    crate::file_format::file_repack_json(&hot, &cold).expect("cold repack");

    let counters = std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default());
    let snapshot =
        crate::pact_file::snapshot::Snapshot::open(&cold, std::sync::Arc::clone(&counters))
            .expect("snapshot");
    let page = snapshot
        .content_range(&content_id, 0, 64 * 1024)
        .expect("range");
    let io = counters.snapshot();

    assert!(page.data.len() <= 64 * 1024);
    assert!(page.next_offset.is_some());
    assert!(std::str::from_utf8(&page.data).is_ok());
    assert!(
        io.bytes_read <= 80 * 1024,
        "bounded read touched {} bytes",
        io.bytes_read
    );
    assert_eq!(io.mappings_touched, 1);
    assert_eq!(io.content_ranges_hydrated, 1);
    assert_eq!(io.components_hydrated, 0);

    let tail_counters = std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default());
    let tail_snapshot =
        crate::pact_file::snapshot::Snapshot::open(&cold, std::sync::Arc::clone(&tail_counters))
            .expect("tail snapshot");
    let line_bytes = "large bounded payload €\n".len() as u64;
    let tail = tail_snapshot
        .content_range(&content_id, line_bytes * 90_000, 64 * 1024)
        .expect("high-offset range");
    let tail_io = tail_counters.snapshot();
    assert!(!tail.data.is_empty());
    assert!(tail_io.bytes_read <= 80 * 1024);
}

#[test]
fn sparse_five_hundred_mib_range_plan_is_logarithmic_and_tiny() {
    const LOGICAL_BYTES: u64 = 500 * 1024 * 1024;
    const CHUNK_BYTES: u64 = 4096;
    let offsets = (0..LOGICAL_BYTES)
        .step_by(CHUNK_BYTES as usize)
        .collect::<Vec<_>>();
    let target = LOGICAL_BYTES - 64 * 1024;
    let (index, chunk_start) = super::content_chunk_start(&offsets, offsets.len(), target);

    assert_eq!(chunk_start, target);
    assert_eq!(index, offsets.len() - 16);
    assert!(offsets.capacity() * std::mem::size_of::<u64>() < 2 * 1024 * 1024);
}
