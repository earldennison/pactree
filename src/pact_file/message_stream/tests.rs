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
    tree.set_automatic_compaction(false);
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
    let tail = handle
        .stream("message_surface", None, None, "before", 1)
        .expect("tail after insertion")["rows"][0]["cursor"]
        .clone();
    append_message(&mut tree, 5);
    handle.refresh().expect("refresh append");
    let appended = handle
        .stream(
            "message_surface",
            tail["row_id"].as_str(),
            tail["seq"].as_u64(),
            "after",
            10,
        )
        .expect("tail cursor survives append");
    assert_eq!(appended["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        appended["rows"][0]["payload"]["children"][0]["content"],
        "message 5 ".repeat(160)
    );
}

#[test]
fn message_surface_reorder_matches_fresh_baseline_without_compaction() {
    for derived_depth in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("live.pact");
        let mut tree = crate::ContextTree::with_id_and_options(
            "order-root",
            crate::ContextTreeOptions {
                derived_depth,
                ..crate::ContextTreeOptions::default()
            },
        )
        .expect("tree");
        tree.save_file(&path).expect("save");
        for index in 0..5 {
            append_message(&mut tree, index);
        }
        tree.set_automatic_compaction(false);
        let mut handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
        let before = handle
            .stream("message_surface", None, None, "after", 5)
            .expect("before");
        let mut inserted = crate::Component::message_turn_authoring(
            Some("user".to_string()),
            None,
            tree.next_authoring_component_options(),
        );
        inserted.children.push(crate::Component::text_authoring(
            "inserted".to_string(),
            tree.next_authoring_component_options(),
        ));
        tree.insert(inserted, (tree.id().to_string(), 2))
            .expect("insert before anchor");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("commit insertion");
        handle.refresh().expect("refresh");
        let after = handle
            .stream("message_surface", None, None, "after", 10)
            .expect("after");
        let actual: Vec<_> = after["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["component_id"].as_str().unwrap().to_string())
            .collect();
        let baseline_path = temp.path().join("baseline.pact");
        tree.save_file(&baseline_path).expect("fresh baseline");
        let baseline = crate::PactFileReadHandle::open(&baseline_path, None)
            .expect("baseline handle")
            .stream("message_surface", None, None, "after", 10)
            .expect("baseline stream");
        let expected: Vec<_> = baseline["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["component_id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(actual, expected, "derived_depth={derived_depth}");
        let cursor = &before["rows"][2]["cursor"];
        assert!(
            handle
                .stream(
                    "message_surface",
                    cursor["row_id"].as_str(),
                    cursor["seq"].as_u64(),
                    "after",
                    10
                )
                .is_err()
        );
    }
}

#[test]
fn message_surface_order_repair_preserves_positional_insertions() {
    for delete_inserted in [false, true] {
        let (_temp, path, mut tree) = saved_messages(5);
        tree.set_automatic_compaction(false);
        let mut inserted = crate::Component::message_turn_authoring(
            Some("user".to_string()),
            None,
            tree.next_authoring_component_options(),
        );
        inserted.children.push(crate::Component::text_authoring(
            "inserted".to_string(),
            tree.next_authoring_component_options(),
        ));
        let inserted = tree
            .insert(inserted, (tree.id().to_string(), 2))
            .expect("middle insertion");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("commit");
        if delete_inserted {
            tree.prune(format!("#{}", inserted.produced_component_ids[0]))
                .expect("delete inserted row");
            tree.send(&crate::ProviderThreadOptions::default())
                .expect("commit deletion");
        }
        let mut handle = crate::PactFileReadHandle::open(&path, None).expect("handle");
        let ids = |value: &serde_json::Value| -> Vec<String> {
            value["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["component_id"].as_str().unwrap().to_string())
                .collect()
        };
        let before = handle
            .stream("message_surface", None, None, "after", 10)
            .expect("before repair");
        let options = crate::PactMemoryOptions {
            scratch_cache_bytes: 4096,
            io_chunk_bytes: 128,
            ..crate::PactMemoryOptions::default()
        };
        let mut writer =
            crate::PactAppendWriter::open(&path, "main", options.clone()).expect("writer");
        writer.set_automatic_maintenance(false);
        let repaired = writer.repair_message_order().expect("repair");
        if !delete_inserted {
            assert_eq!(repaired.transaction_id, None);
        }
        assert_eq!(
            writer
                .repair_message_order()
                .expect("idempotent repair")
                .transaction_id,
            None
        );
        handle.refresh().expect("refresh repaired head");
        let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
        let id = before["rows"][1]["component_id"]
            .as_str()
            .expect("damage row");
        let order = snapshot
            .component_shallow("main", id)
            .expect("component")
            .expect("present")
            .order
            .expect("order");
        let mut damage =
            crate::pact_file::index::MessageOrderRepairPlan::new(&options).expect("damage plan");
        damage.insert(id, order + 70_000).expect("carried key");
        damage.rows_to_reindex = 1;
        writer
            .commit_message_order_plan(damage)
            .expect("damage index");
        handle.refresh().expect("refresh damage");
        let damaged = handle
            .stream("message_surface", None, None, "after", 10)
            .expect("damaged");
        assert_ne!(
            ids(&damaged),
            ids(&before),
            "damage must actually reorder rows"
        );
        assert!(
            writer
                .repair_message_order()
                .expect("repair carried key after insertion")
                .transaction_id
                .is_some()
        );
        assert_eq!(
            writer
                .repair_message_order()
                .expect("idempotent carried repair")
                .transaction_id,
            None
        );
        writer.close();
        handle.refresh().expect("refresh");
        let after = handle
            .stream("message_surface", None, None, "after", 10)
            .expect("after repair");
        assert_eq!(
            ids(&after),
            ids(&before),
            "repair must preserve committed positional order"
        );
        let cursor = &before["rows"][2]["cursor"];
        handle
            .stream(
                "message_surface",
                cursor["row_id"].as_str(),
                cursor["seq"].as_u64(),
                "around",
                1,
            )
            .expect("repair retains projection cursor");
        let mut resumed = crate::ContextTree::open_file(&path, None).expect("reopen");
        resumed.set_automatic_compaction(false);
        append_message(&mut resumed, 99);
        let mut writer =
            crate::PactAppendWriter::open(&path, "main", options).expect("reopen writer");
        writer.set_automatic_maintenance(false);
        assert_eq!(
            writer
                .repair_message_order()
                .expect("repair after append")
                .transaction_id,
            None
        );
        writer.close();
        handle.refresh().expect("refresh append");
        let appended = handle
            .stream("message_surface", None, None, "after", 10)
            .expect("appended");
        assert_eq!(&ids(&appended)[..ids(&before).len()], ids(&before));
        assert_eq!(
            appended["rows"].as_array().unwrap().len(),
            before["rows"].as_array().unwrap().len() + 1
        );
    }
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
