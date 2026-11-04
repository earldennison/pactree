use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

fn op(component_id: &str, kind: TraceOpKind) -> TraceOp {
    TraceOp::new(component_id, kind)
}

#[test]
fn every_search_affecting_op_has_an_explicit_dependency_event() {
    let trace = TransactionTrace {
        from_rev: 7,
        to_rev: 8,
        ops: vec![
            op(
                "ordinary",
                TraceOpKind::SetField {
                    key: "title".to_string(),
                    value: serde_json::json!("updated"),
                },
            ),
            op(
                "ordinary",
                TraceOpKind::UnsetField {
                    key: "title".to_string(),
                },
            ),
            op(
                "moved",
                TraceOpKind::MoveComponent {
                    parent_id: "turn-2".to_string(),
                    index: 1,
                },
            ),
            op(
                "turn",
                TraceOpKind::SetField {
                    key: "role".to_string(),
                    value: serde_json::json!("assistant"),
                },
            ),
            op(
                "call",
                TraceOpKind::SetField {
                    key: "tool_name".to_string(),
                    value: serde_json::json!("search"),
                },
            ),
            op(
                "result",
                TraceOpKind::SetField {
                    key: "success".to_string(),
                    value: serde_json::json!(true),
                },
            ),
            op(
                "result",
                TraceOpKind::SetField {
                    key: "tool_call_id".to_string(),
                    value: serde_json::json!("call-2"),
                },
            ),
            op(
                "hidden",
                TraceOpKind::SetLifecycle {
                    value: serde_json::json!({"state": "rendered", "revision": 8}),
                },
            ),
            op(
                "hidden",
                TraceOpKind::SetAttr {
                    key: "visibility".to_string(),
                    value: serde_json::json!("hidden"),
                },
            ),
            op(
                "turn-3",
                TraceOpKind::InsertComponent {
                    parent_id: "root".to_string(),
                    index: 2,
                    component: serde_json::json!({
                        "id": "turn-3",
                        "componentType": "message_turn",
                        "role": "user"
                    }),
                },
            ),
            op("turn-1", TraceOpKind::DeleteComponent),
        ],
    };

    let changes =
        super::indexed_search_changes_from_trace("main", &trace).expect("derive search changes");
    assert_eq!(changes.len(), trace.ops.len());
    assert_eq!(
        changes[0].change.kind,
        super::PactSearchChangeKind::FieldUpsert
    );
    assert_eq!(
        changes[1].change.kind,
        super::PactSearchChangeKind::FieldDelete
    );
    assert_eq!(
        changes[2].change.kind,
        super::PactSearchChangeKind::RelationshipChange
    );
    assert_eq!(
        changes[3].change.kind,
        super::PactSearchChangeKind::ContextInvalidate
    );
    assert_eq!(
        changes[4].change.kind,
        super::PactSearchChangeKind::ContextInvalidate
    );
    assert_eq!(
        changes[5].change.kind,
        super::PactSearchChangeKind::ContextInvalidate
    );
    assert_eq!(
        changes[6].change.kind,
        super::PactSearchChangeKind::RelationshipChange
    );
    assert_eq!(
        changes[7].change.kind,
        super::PactSearchChangeKind::VisibilityChange
    );
    assert_eq!(
        changes[8].change.kind,
        super::PactSearchChangeKind::VisibilityChange
    );
    assert_eq!(
        changes[9].change.kind,
        super::PactSearchChangeKind::ContextInvalidate
    );
    assert_eq!(
        changes[10].change.kind,
        super::PactSearchChangeKind::FieldDelete
    );
    assert!(changes[0].change.source_hash.is_some());
    assert_eq!(changes[3].change.field.as_deref(), Some("role"));
    assert_eq!(
        changes[6].change.dependency_scope,
        super::PactSearchDependencyScope::BranchContext
    );
    assert_eq!(
        changes[9].change.dependency_scope,
        super::PactSearchDependencyScope::BranchContext
    );
}

#[test]
fn compact_index_value_round_trips_without_component_identity_duplication() {
    let indexed = super::IndexedSearchChange {
        revision: 42,
        ordinal: 3,
        change: super::PactSearchChange {
            kind: super::PactSearchChangeKind::RelationshipChange,
            component_id: "result-1".to_string(),
            parent_turn_id: Some("turn-1".to_string()),
            field: Some("tool_call_id".to_string()),
            content_ref: None,
            source_hash: Some(blake3::hash(b"call-1").to_hex().to_string()),
            dependency_scope: super::PactSearchDependencyScope::BranchContext,
        },
    };
    let encoded = super::encode_index_value(&indexed).expect("encode");
    assert!(!encoded.windows("result-1".len()).any(|v| v == b"result-1"));
    assert_eq!(
        super::decode_index_value(42, 3, "result-1".to_string(), &encoded).expect("decode"),
        indexed
    );
}

#[test]
fn file_feed_is_byte_bounded_cursor_qualified_and_nonhydrating() {
    let fixture = crate::pact_file::tests::fixture_with_text_updates_and_delete();
    crate::tests::instrumentation::reset_ledger_hydration_count();

    let first =
        super::file_search_changes(&fixture.path, Some("main"), None, 2, 4096).expect("first page");
    assert_eq!(first.changes.len(), 2);
    let cursor = first.next_cursor.clone().expect("next cursor");
    assert_eq!(cursor.branch_id, "main");
    assert_ne!(cursor.source_file_identity, cursor.root_id);

    let moved = fixture.path.with_extension("search-moved.pact");
    std::fs::rename(&fixture.path, &moved).expect("move");
    let second = super::file_search_changes(&moved, Some("main"), Some(&cursor), 100, 16 * 1024)
        .expect("second page");
    assert!(!second.changes.is_empty());
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);

    let mut foreign_root = cursor.clone();
    foreign_root.root_id.push_str("-foreign");
    assert_eq!(
        super::file_search_changes(&moved, Some("main"), Some(&foreign_root), 2, 4096)
            .expect_err("foreign root")
            .kind(),
        crate::PactErrorKind::Invalid
    );
    let mut foreign_source = cursor.clone();
    foreign_source.source_file_identity.push_str("-foreign");
    assert_eq!(
        super::file_search_changes(&moved, Some("main"), Some(&foreign_source), 2, 4096)
            .expect_err("foreign source")
            .kind(),
        crate::PactErrorKind::Invalid
    );
    assert_eq!(
        super::file_search_changes(&moved, Some("main"), None, 2, 1)
            .expect_err("insufficient byte budget")
            .kind(),
        crate::PactErrorKind::Invalid
    );
}

#[test]
fn context_only_mutations_produce_distinct_reindex_inputs() {
    let role = |value| TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: vec![op(
            "turn",
            TraceOpKind::SetField {
                key: "role".to_string(),
                value: serde_json::json!(value),
            },
        )],
    };
    let user = super::indexed_search_changes_from_trace("main", &role("user")).expect("user role");
    let assistant = super::indexed_search_changes_from_trace("main", &role("assistant"))
        .expect("assistant role");
    assert_eq!(
        user[0].change.kind,
        super::PactSearchChangeKind::ContextInvalidate
    );
    assert_ne!(user[0].change.source_hash, assistant[0].change.source_hash);
}

#[test]
fn semantic_cursor_survives_automatic_physical_compaction() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("search-compaction.pact");
    let mut tree = crate::ContextTree::with_id("search_compaction_root").expect("tree");
    tree.save_file(&path).expect("save");
    for revision in 1..=15 {
        tree.insert(
            crate::Component::from_component_value(&serde_json::json!({
                "id": format!("text_{revision}"),
                "componentType": "text_content",
                "content": format!("search change {revision}"),
            }))
            .expect("component"),
            "root",
        )
        .expect("insert");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("commit");
    }
    let first =
        super::file_search_changes(&path, Some("main"), None, 1, 4096).expect("first event");
    let cursor = first.next_cursor.clone().expect("cursor before compaction");

    for revision in 16..=20 {
        tree.insert(
            crate::Component::from_component_value(&serde_json::json!({
                "id": format!("text_{revision}"),
                "componentType": "text_content",
                "content": format!("search change {revision}"),
            }))
            .expect("component"),
            "root",
        )
        .expect("insert");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("commit");
    }
    let snapshot = crate::pact_file::snapshot::Snapshot::open(
        &path,
        std::sync::Arc::new(crate::pact_file::backend::IoCounterSet::default()),
    )
    .expect("snapshot");
    assert!(snapshot.generation() <= 16, "automatic compaction must run");

    let tail = super::file_search_changes(&path, Some("main"), Some(&cursor), 1000, 1024 * 1024)
        .expect("tail after compaction");
    let full = super::file_search_changes(&path, Some("main"), None, 1000, 1024 * 1024)
        .expect("full after compaction");
    let mut combined = first.changes;
    combined.extend(tail.changes);
    assert_eq!(combined, full.changes);
}

#[test]
fn positional_field_ops_do_not_produce_search_change_rows() {
    let trace = TransactionTrace {
        from_rev: 3,
        to_rev: 4,
        ops: vec![
            op(
                "m1",
                TraceOpKind::InsertComponent {
                    parent_id: "root".to_string(),
                    index: 0,
                    component: serde_json::json!({"componentType": "message_turn"}),
                },
            ),
            op(
                "old1",
                TraceOpKind::SetField {
                    key: "pact_depth".to_string(),
                    value: serde_json::json!(2),
                },
            ),
            op(
                "old1",
                TraceOpKind::SetField {
                    key: "offset".to_string(),
                    value: serde_json::json!(3),
                },
            ),
            op(
                "root",
                TraceOpKind::SetField {
                    key: "child_ids".to_string(),
                    value: serde_json::json!(["old1", "m1"]),
                },
            ),
            op(
                "old1",
                TraceOpKind::UnsetField {
                    key: "depth".to_string(),
                },
            ),
            op(
                "m1",
                TraceOpKind::SetField {
                    key: "role".to_string(),
                    value: serde_json::json!("user"),
                },
            ),
        ],
    };
    let rows = super::indexed_search_changes_from_trace("main", &trace).expect("rows");

    assert_eq!(
        rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
        vec![0, 5]
    );
    assert!(rows.iter().all(|row| row.revision == 4));
}
