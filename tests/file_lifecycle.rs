use pactree::{Component, ContextTree, ProviderThreadOptions, file_format};
use serde_json::{Value, json};

fn text_component(id: &str, content: &str) -> Component {
    Component::from_component_value(&json!({
        "id": id,
        "componentType": "text_content",
        "content": content,
    }))
    .expect("text component")
}

#[test]
fn range_slice_heals_and_merges_as_an_independent_pact_file() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = temp.path().join("source.pact");
    let slice = temp.path().join("slice.pact");
    let healed = temp.path().join("slice-healed.pact");
    let merge_target = temp.path().join("merge-target.pact");

    let mut tree = ContextTree::with_id("slice_root").expect("tree");
    tree.save_file(&source).expect("save source");
    for revision in 1..=6 {
        let content = if revision == 1 {
            "externalized lifecycle content ".repeat(300)
        } else {
            format!("revision {revision}")
        };
        tree.insert(
            text_component(&format!("text_{revision}"), &content),
            "root",
        )
        .expect("insert revision");
        tree.send(&ProviderThreadOptions::default())
            .expect("seal revision");
    }

    let slice_validation: Value = serde_json::from_str(
        &file_format::file_slice_json(
            &source,
            &slice,
            r#"{"mode":"transaction-range","branch_id":"main","start_cycle":2,"end_cycle":4}"#,
        )
        .expect("slice range"),
    )
    .expect("slice validation JSON");
    assert_eq!(slice_validation["manifest"]["file_kind"], "slice");
    assert!(
        slice_validation["manifest"]["capabilities"]
            .as_array()
            .expect("slice capabilities")
            .iter()
            .any(|capability| capability == "slice_open")
    );
    assert_eq!(slice_validation["manifest"]["trace_count"], 2);
    assert_eq!(slice_validation["latest_valid_branch_heads"]["main"], 4);

    let sliced = ContextTree::open_file(&slice, None).expect("open slice");
    assert_eq!(sliced.head().expect("slice head").head_revision, 4);
    assert!(sliced.project_at_revision(2).is_ok());
    let slice_head = sliced
        .project_at_revision(4)
        .expect("slice head projection");
    assert!(slice_head.branch.component("slice_root:text_4").is_some());
    assert!(slice_head.branch.component("slice_root:text_5").is_none());

    let heal_report: Value = serde_json::from_str(
        &file_format::file_repair_json(&slice, &healed, true).expect("heal slice"),
    )
    .expect("heal report JSON");
    assert_eq!(
        heal_report["repaired_validation"]["manifest"]["file_kind"],
        "slice"
    );
    assert!(
        heal_report["repaired_validation"]["manifest"]["capabilities"]
            .as_array()
            .expect("healed capabilities")
            .iter()
            .any(|capability| capability == "is_healed")
    );

    std::fs::copy(&source, &merge_target).expect("copy merge target");
    let merge_report: Value = serde_json::from_str(
        &file_format::file_merge_json(&merge_target, &healed).expect("merge healed slice"),
    )
    .expect("merge report JSON");
    let imported_branch = merge_report["imported_branch_ids"][0]
        .as_str()
        .expect("imported branch id");
    let imported_history = ContextTree::open_file(&merge_target, Some(imported_branch))
        .expect("open imported slice branch");
    assert!(
        imported_history.project_at_revision(2).is_ok(),
        "merge must copy the slice base content required for historical projection"
    );
    let imported: Value = serde_json::from_str(
        &file_format::file_project_tree_json(&merge_target, Some(imported_branch), None)
            .expect("project imported slice"),
    )
    .expect("imported projection JSON");
    let expected: Value = serde_json::from_str(
        &file_format::file_project_tree_json(&healed, Some("main"), None)
            .expect("project healed slice"),
    )
    .expect("healed projection JSON");
    assert_eq!(imported, expected);

    let moved_source = source.with_extension("moved");
    std::fs::rename(&source, &moved_source).expect("move source");
    let slice_tree = ContextTree::open_file(&slice, Some("main")).expect("reopen moved slice");
    let merged_tree =
        ContextTree::open_file(&merge_target, Some(imported_branch)).expect("reopen merged branch");
    let slice_revision = slice_tree.project_at_revision(4).expect("slice revision");
    let merged_revision = merged_tree.project_at_revision(4).expect("merged revision");
    assert_eq!(
        serde_json::to_value(slice_revision.branch.tree).expect("slice projection JSON"),
        serde_json::to_value(merged_revision.branch.tree).expect("merged projection JSON"),
    );
    let component = pactree::PactFileReadHandle::open(&merge_target, Some(imported_branch))
        .expect("imported read handle")
        .component("slice_root:text_4")
        .expect("indexed imported component");
    assert_eq!(component["content"], "revision 4");
    let mut selector = file_format::PactFileSelectorQuery::new(".text_content");
    selector.branch_id = Some(imported_branch.to_string());
    selector.limit = 10;
    let selected: Value = serde_json::from_str(
        &file_format::file_select_json(&merge_target, selector).expect("indexed branch selector"),
    )
    .expect("selector JSON");
    assert_eq!(selected["fallback_used"], false);
    assert_eq!(selected["hits"].as_array().map(Vec::len), Some(4));
    assert_content_subset_exact(&slice, &merge_target);
}

fn assert_content_subset_exact(source: &std::path::Path, target: &std::path::Path) {
    let json = file_format::file_query_json(source, "content", None).expect("query content");
    let value: Value = serde_json::from_str(&json).expect("content query JSON");
    for content_id in value["matches"]
        .as_array()
        .expect("content matches")
        .iter()
        .filter_map(|record| record.get("id").and_then(Value::as_str))
    {
        assert_eq!(
            file_format::file_content_payload_json(source, content_id).expect("source content"),
            file_format::file_content_payload_json(target, content_id).expect("target content"),
        );
    }
}
