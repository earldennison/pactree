use pactree::{Component, ContextTree, PactAttrs, PactFileReadHandle, PactMemoryOptions};

#[test]
fn cache_residency_is_reported_while_a_handle_is_open_and_released_on_drop() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("gauge.pact");
    let mut tree = ContextTree::with_id("gauge_root").expect("tree");
    for turn in 0..6 {
        let options = tree.next_authoring_component_options();
        tree.add_message(
            "user",
            vec![Component::text_authoring(format!("turn {turn}"), options)],
            PactAttrs::new(),
        )
        .expect("stage");
        tree.seal_staged(pactree::trace::BoundaryKind::Adhoc)
            .expect("seal");
    }
    tree.save_file(&path).expect("save");
    drop(tree);

    assert_eq!(pactree::storage_memory().cache_resident_bytes, 0);
    let handle = PactFileReadHandle::open(&path, None).expect("open");
    handle
        .stream("current_surface", None, None, "before", 20)
        .expect("window");
    let while_open = pactree::storage_memory().cache_resident_bytes;
    assert!(
        while_open > 0,
        "decoded index records are resident while the handle lives"
    );
    assert!(
        while_open
            <= PactMemoryOptions::default().page_cache_bytes
                + PactMemoryOptions::default().component_cache_bytes
                + 2 * 1024 * 1024,
        "residency stays within the configured budgets (plus one oversized slot): {while_open}"
    );
    drop(handle);
    assert_eq!(pactree::storage_memory().cache_resident_bytes, 0);

    let zero = PactFileReadHandle::open_with_memory_options(
        &path,
        None,
        PactMemoryOptions {
            page_cache_bytes: 0,
            component_cache_bytes: 0,
            ..PactMemoryOptions::default()
        },
    )
    .expect("open with zero budgets");
    zero.info().expect("info");
    let resident = pactree::storage_memory().cache_resident_bytes;
    assert!(
        resident > 0,
        "the oversized slot holds the record being read"
    );
    drop(zero);
    assert_eq!(pactree::storage_memory().cache_resident_bytes, 0);
}
