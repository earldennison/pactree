use pactree::{
    Component, ContextTree, PactFileReadHandle, ProviderThreadOptions, SearchFieldLocator,
    SearchHitLocator,
};

#[test]
fn one_handle_pins_snapshot_reads_and_hydration_until_refresh() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("pinned-search.pact");
    let mut tree = ContextTree::with_id("pinned_search_root").expect("tree");
    tree.save_file(&path).expect("save");
    let first_options = tree.next_authoring_component_options();
    let trace = tree
        .insert(
            Component::text_authoring("bounded UTF-8 payload €\n".repeat(100_000), first_options),
            "root",
        )
        .expect("insert first");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit first");
    let component_id = trace.produced_component_ids[0].clone();

    let mut handle = PactFileReadHandle::open(&path, None).expect("open pinned handle");
    let pinned = handle.search_snapshot_info().expect("snapshot info");
    let component = handle.component(&component_id).expect("component locator");
    let content_id = component["content_ref"]["id"]
        .as_str()
        .expect("externalized content id")
        .to_string();

    let content = handle
        .stream_content_range(&content_id, 0, 64 * 1024)
        .expect("bounded content range");
    assert!(content.data.len() <= 64 * 1024);
    assert!(content.next_offset.is_some());
    assert!(std::str::from_utf8(&content.data).is_ok());

    let field = handle
        .read_search_field(
            &SearchFieldLocator {
                component_id: component_id.clone(),
                field: "content".to_string(),
                offset: 0,
            },
            64 * 1024,
        )
        .expect("bounded field");
    assert_eq!(field.data, content.data);

    let newer_options = tree.next_authoring_component_options();
    tree.insert(
        Component::text_authoring("newer".to_string(), newer_options),
        "root",
    )
    .expect("insert newer");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit newer");

    assert_eq!(handle.search_snapshot_info().expect("still pinned"), pinned);
    let hydrated = handle
        .hydrate_search_hits(
            &[SearchHitLocator {
                component_id: component_id.clone(),
                field: "content".to_string(),
                offset: 0,
                max_bytes: 4096,
            }],
            4096,
        )
        .expect("pinned hydration");
    assert_eq!(hydrated.len(), 1);
    assert_eq!(hydrated[0].component_id, component_id);
    assert!(hydrated[0].field.data.len() <= 4096);

    handle.refresh().expect("refresh");
    let refreshed = handle.search_snapshot_info().expect("refreshed info");
    assert_eq!(refreshed.head_revision, pinned.head_revision + 1);
    assert_eq!(refreshed.source_file_identity, pinned.source_file_identity);
}
