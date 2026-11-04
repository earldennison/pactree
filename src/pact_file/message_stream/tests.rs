use std::sync::Arc;

use super::window;
use crate::error::PactError;
use crate::pact_file::backend::{IoCounterSet, IoCounters};
use crate::pact_file::snapshot::Snapshot;

fn saved_messages(count: usize) -> (tempfile::TempDir, std::path::PathBuf, crate::ContextTree) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("message-stream.pact");
    let mut tree = crate::ContextTree::with_id("message_stream_root").expect("tree");
    tree.save_file(&path).expect("save");
    for index in 0..count {
        append_message(&mut tree, index);
    }
    (temp, path, tree)
}

fn append_message(tree: &mut crate::ContextTree, index: usize) {
    let component = crate::Component::text_authoring(
        format!("message {index} ").repeat(160),
        tree.next_authoring_component_options(),
    );
    tree.add_message("user", vec![component], crate::PactAttrs::new())
        .expect("add message");
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit message");
}

fn counters_after_window(
    path: &std::path::Path,
    direction: &str,
    limit: usize,
) -> (serde_json::Value, IoCounters) {
    let counters = Arc::new(IoCounterSet::default());
    let snapshot = Snapshot::open(path, Arc::clone(&counters)).expect("snapshot");
    let value = window(
        &snapshot,
        &path.display().to_string(),
        "main",
        None,
        None,
        direction,
        limit,
        "test_index",
    )
    .expect("message window");
    (value, counters.snapshot())
}

#[test]
fn message_surface_materializes_only_the_selected_messages_and_children() {
    let (_temp, path, _tree) = saved_messages(40);
    crate::tests::instrumentation::reset_ledger_hydration_count();

    let (stream, counters) = counters_after_window(&path, "before", 3);

    assert_eq!(stream["schema"], "pact.stream.v1");
    assert_eq!(stream["mode"], "message_surface");
    assert_eq!(stream["source"], "test_index");
    assert_eq!(stream["position"]["total_rows"], 40);
    assert_eq!(stream["rows"].as_array().map(Vec::len), Some(3));
    assert_eq!(stream["rows"][0]["seq"], stream["rows"][0]["cursor"]["seq"]);
    assert_eq!(stream["rows"][0]["seq"], 37);
    assert_eq!(
        stream["rows"][0]["payload"]["children"][0]["content"],
        "message 37 ".repeat(160)
    );
    assert!(counters.rows_materialized <= 3);
    assert_eq!(
        crate::tests::instrumentation::ledger_hydration_count(),
        0,
        "message paging must not hydrate the ledger"
    );
}

#[test]
fn message_surface_cursor_fetches_only_rows_appended_after_refresh() {
    let (_temp, path, mut tree) = saved_messages(3);
    let mut handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
    let initial = handle
        .stream("message_surface", None, None, "before", 3)
        .expect("initial window");
    let cursor = initial["cursors"]["after"].clone();

    append_message(&mut tree, 3);
    append_message(&mut tree, 4);
    handle.refresh().expect("refresh");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let delta = handle
        .stream(
            "message_surface",
            cursor["row_id"].as_str(),
            cursor["seq"].as_u64(),
            "after",
            10,
        )
        .expect("delta window");

    let delta_rows = delta["rows"].as_array().expect("rows");
    assert_eq!(delta_rows.len(), 2);
    assert_eq!(
        delta_rows[0]["payload"]["children"][0]["content"],
        "message 3 ".repeat(160)
    );
    assert_eq!(
        delta_rows[1]["payload"]["children"][0]["content"],
        "message 4 ".repeat(160)
    );
    assert_eq!(cursor["seq"], 2);
    assert_eq!(delta_rows[0]["seq"], 3);
    assert_eq!(delta_rows[1]["seq"], 4);
    assert!(!delta["has_after"].as_bool().expect("has_after"));
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
fn message_surface_reports_missing_and_stale_anchors_explicitly() {
    let (_temp, path, _tree) = saved_messages(4);
    let handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
    let first = handle
        .stream("message_surface", None, None, "after", 1)
        .expect("first row");
    let row_id = first["rows"][0]["row_id"].as_str().expect("row id");

    let missing = handle
        .stream(
            "message_surface",
            Some("node:missing-message"),
            None,
            "after",
            1,
        )
        .expect_err("missing anchor");
    assert!(matches!(missing, PactError::NotFound(_)));

    let actual_seq = first["rows"][0]["seq"].as_u64().expect("stable seq");
    let stale = handle
        .stream(
            "message_surface",
            Some(row_id),
            Some(actual_seq.saturating_add(1)),
            "after",
            1,
        )
        .expect_err("stale anchor");
    assert!(matches!(stale, PactError::Invalid(_)));
    assert!(stale.to_string().contains("stale message stream anchor"));

    let seq_only = handle
        .stream("message_surface", None, Some(99), "after", 1)
        .expect_err("seq-only anchor must not alias a positional row");
    assert!(matches!(seq_only, PactError::Unsupported(_)));
    assert!(seq_only.to_string().contains("requires anchor_row_id"));
}

#[test]
fn message_surface_cursor_stales_after_prior_row_deletion() {
    let (_temp, path, mut tree) = saved_messages(5);
    let mut handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
    let initial = handle
        .stream("message_surface", None, None, "before", 5)
        .expect("initial window");
    let cursor = initial["rows"][2]["cursor"].clone();
    let deleted_id = initial["rows"][0]["component_id"]
        .as_str()
        .expect("deleted component id")
        .to_string();

    tree.prune(format!("#{deleted_id}"))
        .expect("prune earlier message by selector");
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit prune");
    handle.refresh().expect("refresh");
    let stale = handle
        .stream(
            "message_surface",
            cursor["row_id"].as_str(),
            cursor["seq"].as_u64(),
            "after",
            10,
        )
        .expect_err("prior deletion must stale the projection token");
    assert!(matches!(stale, PactError::Invalid(_)));
    assert!(stale.to_string().contains("seq 2 is now 1"));

    let rebased = handle
        .stream(
            "message_surface",
            cursor["row_id"].as_str(),
            None,
            "after",
            10,
        )
        .expect("row-id-only bounded rebase");

    assert_eq!(rebased["position"]["start_index"], 2);
    assert_eq!(rebased["position"]["total_rows"], 4);
    assert_eq!(rebased["rows"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        rebased["rows"][0]["payload"]["children"][0]["content"],
        "message 3 ".repeat(160)
    );
    assert_eq!(
        rebased["rows"][1]["payload"]["children"][0]["content"],
        "message 4 ".repeat(160)
    );
}

#[test]
fn message_surface_cursor_stales_after_insertion_before_anchor() {
    let (_temp, path, mut tree) = saved_messages(5);
    let mut handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
    let initial = handle
        .stream("message_surface", None, None, "before", 5)
        .expect("initial window");
    let cursor = initial["rows"][2]["cursor"].clone();

    let text = crate::Component::text_authoring(
        "inserted message".to_string(),
        tree.next_authoring_component_options(),
    );
    let mut inserted = crate::Component::message_turn_authoring(
        Some("user".to_string()),
        None,
        tree.next_authoring_component_options(),
    );
    inserted.children.push(text);
    let root_id = tree.id().to_string();
    tree.insert(inserted, (root_id, 2))
        .expect("insert message before anchor");
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit insertion");
    handle.refresh().expect("refresh");

    let stale = handle
        .stream(
            "message_surface",
            cursor["row_id"].as_str(),
            cursor["seq"].as_u64(),
            "after",
            10,
        )
        .expect_err("prior insertion must stale the projection token");
    assert!(matches!(stale, PactError::Invalid(_)));
    assert!(stale.to_string().contains("seq 2 is now 3"));

    let rebased = handle
        .stream(
            "message_surface",
            cursor["row_id"].as_str(),
            None,
            "around",
            1,
        )
        .expect("row-id-only bounded rebase");
    assert_eq!(rebased["rows"][0]["row_id"], cursor["row_id"]);
    assert_eq!(rebased["rows"][0]["seq"], 3);
    assert_eq!(
        rebased["rows"][0]["payload"]["children"][0]["content"],
        "message 2 ".repeat(160)
    );
}

#[test]
fn file_message_surface_uses_the_same_index_only_contract() {
    let (_temp, path, _tree) = saved_messages(8);
    crate::tests::instrumentation::reset_ledger_hydration_count();

    let stream: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_stream_json(
            &path,
            "chat_surface",
            Some("main"),
            None,
            None,
            None,
            "before",
            2,
            1024,
        )
        .expect("file message stream"),
    )
    .expect("stream JSON");

    assert_eq!(stream["mode"], "message_surface");
    assert_eq!(stream["source"], "pact_file_index");
    assert_eq!(stream["rows"].as_array().map(Vec::len), Some(2));
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
#[ignore = "requires PACTREE_LENSKEEPER_ARTIFACT evidence path"]
fn lenskeeper_message_surface_pages_the_real_converted_fixture_without_hydration() {
    let path = std::path::PathBuf::from(
        std::env::var("PACTREE_LENSKEEPER_ARTIFACT").expect("Lenskeeper artifact path"),
    );
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let handle = crate::PactFileReadHandle::open(&path, None).expect("Lenskeeper read handle");
    let latest = handle
        .stream("message_surface", None, None, "before", 20)
        .expect("latest Lenskeeper messages");
    let cursor = latest["cursors"]["before"].clone();
    let earlier = handle
        .stream(
            "message_surface",
            cursor["row_id"].as_str(),
            cursor["seq"].as_u64(),
            "before",
            20,
        )
        .expect("earlier Lenskeeper messages");

    assert_eq!(latest["rows"].as_array().map(Vec::len), Some(20));
    assert_eq!(earlier["rows"].as_array().map(Vec::len), Some(20));
    assert!(
        latest["position"]["total_rows"]
            .as_u64()
            .unwrap_or_default()
            > 40
    );
    assert_ne!(
        earlier["cursors"]["after"]["row_id"],
        latest["cursors"]["before"]["row_id"]
    );
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}
