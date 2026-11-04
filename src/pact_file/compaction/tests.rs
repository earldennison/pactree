use crate::{Component, ContextTree, ProviderThreadOptions};

#[test]
fn repeated_appends_compact_automatically_without_explicit_repack() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("automatic-compaction.pact");
    let mut tree = ContextTree::with_id("automatic_compaction_root").expect("tree");
    tree.save_file(&path).expect("save");

    for revision in 1..=96 {
        tree.insert(
            Component::from_component_value(&serde_json::json!({
                "id": format!("text_{revision}"),
                "componentType": "text_content",
                "content": format!("turn {revision}: {}", "bounded compact payload ".repeat(8)),
            }))
            .expect("component"),
            "root",
        )
        .expect("insert");
        tree.send(&ProviderThreadOptions::default())
            .expect("commit");
    }

    let bytes = std::fs::read(&path).expect("read compacted file");
    let selected = crate::pact_file::root_slot::select_root_slot(&bytes).expect("selected root");
    let metrics =
        crate::pact_file::layout_metrics::measure_committed_layout(&bytes).expect("layout metrics");
    assert!(selected.slot.generation <= 16);
    assert!(metrics.derived_amplification <= 1.25);
    assert_eq!(metrics.compaction_whole_ledger_hydrations, 0);
    assert_eq!(
        ContextTree::open_file(&path, None)
            .expect("reopen")
            .head()
            .expect("head")
            .head_revision,
        96
    );
}

#[test]
fn automatic_compaction_preserves_externalized_content_exactly() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("automatic-content-compaction.pact");
    let mut tree = ContextTree::with_id("automatic_content_root").expect("tree");
    tree.save_file(&path).expect("save");
    let expected = "large tool payload line\n".repeat(4096);
    let options = tree.next_authoring_component_options();
    let trace = tree
        .insert(Component::text_authoring(expected.clone(), options), "root")
        .expect("insert large content");
    let component_id = trace.produced_component_ids[0].clone();
    tree.send(&ProviderThreadOptions::default())
        .expect("commit large content");

    for revision in 1..=48 {
        let options = tree.next_authoring_component_options();
        tree.insert(
            Component::text_authoring(format!("turn {revision}"), options),
            "root",
        )
        .expect("insert");
        tree.send(&ProviderThreadOptions::default())
            .expect("commit");
    }

    crate::tests::instrumentation::reset_ledger_hydration_count();
    let actual = crate::PactFileReadHandle::open(&path, None)
        .expect("read handle")
        .component(&component_id)
        .expect("component");
    assert_eq!(actual["content"], expected);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}
