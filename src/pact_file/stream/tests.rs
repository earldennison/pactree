use std::collections::BTreeMap;
use std::sync::Arc;

use super::{StreamDirection, activity_window, current_window};
use crate::pact_file::backend::{CandidateBackend, IoCounterSet, PactFileBackend};
use crate::pact_file::snapshot::Snapshot;
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

fn transaction(revision: u64) -> PactTransaction {
    let id = format!("tx_{revision}");
    let component_id = format!("message_{revision:04}");
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(revision),
        transaction_id: Some(id.clone()),
        component_id: None,
        content_ref: None,
    };
    PactTransaction::new(
        id.clone(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: revision - 1,
            to_rev: revision,
            ops: vec![TraceOp::new(
                component_id.clone(),
                TraceOpKind::InsertComponent {
                    parent_id: "root".to_string(),
                    index: (revision - 1) as usize,
                    component: serde_json::json!({
                        "id": component_id,
                        "componentType": "message_turn",
                        "role": if revision.is_multiple_of(2) { "assistant" } else { "user" }
                    }),
                },
            )],
        },
        PactTransactionTrace {
            schema: TRANSACTION_TRACE_SCHEMA.to_string(),
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            transaction_id: id,
            authored_ref: pact_ref.clone(),
            pre_request_ref: pact_ref.clone(),
            post_request_alteration_ref: pact_ref,
            request_ref: None,
            response_ref: None,
            post_response_alteration_ref: None,
            post_response_ref: None,
            alteration_ids: Vec::new(),
            commit: None,
            replay_trace: None,
            aux: BTreeMap::new(),
        },
        Vec::new(),
    )
}

#[test]
fn current_windows_materialize_only_limit_plus_lookahead() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("stream.pact");
    let backend = CandidateBackend::default();
    backend.create(&path, &transaction(1)).expect("create");
    for revision in 2..=100 {
        backend
            .commit(&path, &transaction(revision))
            .expect("commit");
    }
    let counters = Arc::new(IoCounterSet::default());
    let snapshot = Snapshot::open(&path, Arc::clone(&counters)).expect("snapshot");
    let window = current_window(&snapshot, "main", None, StreamDirection::Before, 10)
        .expect("latest window");

    assert_eq!(window.rows.len(), 10);
    assert!(window.has_before);
    assert!(!window.has_after);
    assert_eq!(window.rows[0].component_id, "message_0091");
    assert!(counters.snapshot().rows_materialized <= 11);
}

#[test]
fn public_current_stream_window_does_not_hydrate_the_full_ledger() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("public-stream.pact");
    let mut tree = crate::ContextTree::with_id("public_stream").expect("tree");
    tree.save_file(&path).expect("save");
    for revision in 1..=100 {
        let component = crate::Component::text_authoring(
            format!("message {revision}"),
            tree.next_authoring_component_options(),
        );
        tree.add_message("user", vec![component], crate::PactAttrs::new())
            .expect("message");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("send");
    }

    let expected: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_stream_json(
            &path,
            "current_surface",
            Some("main"),
            Some(100),
            None,
            None,
            "before",
            10,
            1024,
        )
        .expect("hydrated compatibility oracle"),
    )
    .expect("oracle JSON");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let stream: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_stream_json(
            &path,
            "current_surface",
            Some("main"),
            None,
            None,
            None,
            "before",
            10,
            1024,
        )
        .expect("public stream"),
    )
    .expect("stream JSON");

    assert_eq!(stream["rows"].as_array().map(Vec::len), Some(10));
    assert_eq!(
        stream, expected,
        "indexed stream changed the public envelope"
    );
    assert_eq!(
        crate::tests::instrumentation::ledger_hydration_count(),
        0,
        "a bounded current-surface stream must stay on the snapshot indexes"
    );
}

#[test]
fn cold_baseline_stream_matches_hydrated_projection_without_full_hydration() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let hot_path = temp.path().join("hot.pact");
    let cold_path = temp.path().join("cold.pact");
    let mut tree = crate::ContextTree::with_id("cold_stream").expect("tree");
    tree.save_file(&hot_path).expect("save");
    for revision in 1..=30 {
        let component = crate::Component::text_authoring(
            format!("message {revision}"),
            tree.next_authoring_component_options(),
        );
        tree.add_message("user", vec![component], crate::PactAttrs::new())
            .expect("message");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("send");
    }
    let ledger = crate::pact_file::open_ledger(&hot_path).expect("open hot ledger");
    std::fs::write(
        &cold_path,
        crate::pact_file::encode_file_baseline(&ledger).expect("cold baseline"),
    )
    .expect("write cold baseline");
    let expected: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_stream_json(
            &cold_path,
            "current_surface",
            Some("main"),
            Some(30),
            None,
            None,
            "before",
            10,
            1024,
        )
        .expect("hydrated compatibility oracle"),
    )
    .expect("oracle JSON");

    crate::tests::instrumentation::reset_ledger_hydration_count();
    let actual: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_stream_json(
            &cold_path,
            "current_surface",
            Some("main"),
            None,
            None,
            None,
            "before",
            10,
            1024,
        )
        .expect("indexed cold stream"),
    )
    .expect("indexed JSON");

    assert_eq!(actual, expected, "cold index changed the public envelope");
    assert_eq!(
        crate::tests::instrumentation::ledger_hydration_count(),
        0,
        "cold stream must stay on snapshot indexes"
    );
}

#[test]
fn public_current_stream_all_cursor_shapes_match_without_full_hydration() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("public-stream-cursors.pact");
    let mut tree = crate::ContextTree::with_id("public_stream_cursors").expect("tree");
    tree.save_file(&path).expect("save");
    for revision in 1..=24 {
        let component = crate::Component::text_authoring(
            format!("message {revision}"),
            tree.next_authoring_component_options(),
        );
        tree.add_message("user", vec![component], crate::PactAttrs::new())
            .expect("message");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("send");
    }
    let full_oracle: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_stream_json(
            &path,
            "current_surface",
            Some("main"),
            Some(24),
            None,
            None,
            "after",
            500,
            1024,
        )
        .expect("full oracle"),
    )
    .expect("oracle JSON");
    let anchor_row_id = full_oracle["rows"][9]["row_id"]
        .as_str()
        .expect("row id")
        .to_string();
    let cases = [
        (None, None, "after", 7),
        (None, Some(9), "before", 5),
        (None, Some(9), "after", 5),
        (None, Some(9), "around", 5),
        (Some(anchor_row_id.as_str()), None, "before", 5),
        (Some(anchor_row_id.as_str()), None, "after", 5),
    ];

    for (anchor_row_id, anchor_seq, direction, limit) in cases {
        let mut expected: serde_json::Value = serde_json::from_str(
            &crate::file_format::file_stream_json(
                &path,
                "current_surface",
                Some("main"),
                Some(24),
                anchor_row_id,
                anchor_seq,
                direction,
                limit,
                1024,
            )
            .expect("hydrated oracle"),
        )
        .expect("oracle JSON");
        let indexed_path = temp.path().join(format!(
            "indexed-{direction}-{}-{}.pact",
            anchor_seq.unwrap_or_default(),
            anchor_row_id.is_some()
        ));
        std::fs::copy(&path, &indexed_path).expect("copy indexed candidate");
        expected["path"] = serde_json::json!(indexed_path.display().to_string());
        expected["head"]["file_path"] = serde_json::json!(indexed_path.display().to_string());
        crate::tests::instrumentation::reset_ledger_hydration_count();
        let actual: serde_json::Value = serde_json::from_str(
            &crate::file_format::file_stream_json(
                &indexed_path,
                "current_surface",
                Some("main"),
                None,
                anchor_row_id,
                anchor_seq,
                direction,
                limit,
                1024,
            )
            .expect("indexed stream"),
        )
        .expect("indexed JSON");

        assert_eq!(actual, expected, "cursor case changed: {direction}");
        assert_eq!(
            crate::tests::instrumentation::ledger_hydration_count(),
            0,
            "cursor case hydrated the full ledger: {direction}"
        );
    }
}

#[test]
fn anchored_forward_window_starts_after_anchor() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("anchored.pact");
    let backend = CandidateBackend::default();
    backend.create(&path, &transaction(1)).expect("create");
    for revision in 2..=8 {
        backend
            .commit(&path, &transaction(revision))
            .expect("commit");
    }
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let window = current_window(&snapshot, "main", Some(4), StreamDirection::After, 2)
        .expect("anchored window");
    assert_eq!(
        window
            .rows
            .iter()
            .map(|row| row.component_id.as_str())
            .collect::<Vec<_>>(),
        vec!["message_0005", "message_0006"]
    );
    assert!(window.has_after);
    assert!(window.has_before);
}

#[test]
fn current_window_reflects_component_updates_and_deletes() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("mutated.pact");
    let backend = CandidateBackend::default();
    backend.create(&path, &transaction(1)).expect("create");
    backend.commit(&path, &transaction(2)).expect("second");
    let mut update = transaction(3);
    update.trace.ops = vec![TraceOp::new(
        "message_0001",
        TraceOpKind::SetField {
            key: "role".to_string(),
            value: serde_json::json!("system"),
        },
    )];
    backend.commit(&path, &update).expect("update");
    let mut delete = transaction(4);
    delete.trace.ops = vec![TraceOp::new("message_0002", TraceOpKind::DeleteComponent)];
    backend.commit(&path, &delete).expect("delete");

    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let window = current_window(&snapshot, "main", None, StreamDirection::After, 10)
        .expect("current window");
    assert_eq!(window.rows.len(), 1);
    assert_eq!(window.rows[0].component_id, "message_0001");
    assert_eq!(window.rows[0].payload["role"], "system");
}

#[test]
fn activity_window_decodes_only_relevant_window_traces() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("activity.pact");
    let backend = CandidateBackend::default();
    backend.create(&path, &transaction(1)).expect("create");
    for revision in 2..=100 {
        backend
            .commit(&path, &transaction(revision))
            .expect("commit");
    }
    let counters = Arc::new(IoCounterSet::default());
    let snapshot = Snapshot::open(&path, Arc::clone(&counters)).expect("snapshot");
    let window = activity_window(&snapshot, "main", None, StreamDirection::Before, 10)
        .expect("activity window");

    assert_eq!(window.rows.len(), 10);
    assert!(window.has_before);
    assert!(counters.snapshot().records_decoded <= 11);
    assert!(counters.snapshot().rows_materialized <= 11);
}
