use super::*;
use crate::store::new_ledger_json;

fn fresh_root_store() -> ContextTreeLedger {
    let tree = r#"{"id":"root","componentType":"root","children":[]}"#;
    let json = new_ledger_json(tree, Some("main"), 1024).expect("root store");
    serde_json::from_str(&json).expect("decode")
}

#[test]
fn read_only_live_follows_source_head() {
    let mut store = fresh_root_store();

    if let Some(main) = store.branches.get_mut("main") {
        main.head_revision = 5;
    }
    let view = open_read_only_live(&store, "main").expect("open");
    assert_eq!(view.kind, ViewKind::ReadOnlyLive);
    assert_eq!(view.source_revision_at_creation, 5);

    if let Some(main) = store.branches.get_mut("main") {
        main.head_revision = 8;
    }
    let resolved = resolve_view(&store, &view).expect("resolve");
    assert_eq!(resolved.revision, Some(8));
}

#[test]
fn read_only_pinned_stays_at_creation_revision() {
    let mut store = fresh_root_store();
    if let Some(main) = store.branches.get_mut("main") {
        main.head_revision = 3;
    }
    let view = open_read_only_pinned(&store, "main", Some(3)).expect("open");
    assert_eq!(view.kind, ViewKind::ReadOnlyPinned);

    if let Some(main) = store.branches.get_mut("main") {
        main.head_revision = 9;
    }
    let resolved = resolve_view(&store, &view).expect("resolve");
    assert_eq!(resolved.revision, Some(3));
}

#[test]
fn read_only_pinned_rejects_future_revision() {
    let store = fresh_root_store();

    let err = open_read_only_pinned(&store, "main", Some(99)).unwrap_err();
    assert!(matches!(err, ViewError::InvalidPinnedRevision { .. }));
}

#[test]
fn mutable_consumer_creates_branch_with_lineage_aux() {
    let mut store = fresh_root_store();
    if let Some(main) = store.branches.get_mut("main") {
        main.head_revision = 2;
    }
    let view = open_mutable_consumer(&mut store, "main", "worker_view").expect("open");
    assert_eq!(view.kind, ViewKind::MutableConsumer);
    assert_eq!(view.consumer_branch_id.as_deref(), Some("worker_view"));

    let consumer = store
        .branches
        .get("worker_view")
        .expect("consumer branch exists");
    assert_eq!(consumer.parent_branch_id.as_deref(), Some("main"));
    assert!(consumer.aux.contains_key("view"));
    let view_aux = consumer.aux.get("view").unwrap();
    assert_eq!(
        view_aux.get("source_branch_id").and_then(Value::as_str),
        Some("main")
    );
    assert_eq!(
        view_aux.get("source_revision").and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        view_aux.get("kind").and_then(Value::as_str),
        Some("mutable_consumer")
    );
}

#[test]
fn mutable_consumer_rejects_duplicate_branch_id() {
    let mut store = fresh_root_store();
    open_mutable_consumer(&mut store, "main", "dup").expect("first");
    let err = open_mutable_consumer(&mut store, "main", "dup").unwrap_err();
    assert!(matches!(err, ViewError::ConsumerBranchAlreadyExists(_)));
}

#[test]
fn resolve_consumer_view_returns_consumer_head() {
    let mut store = fresh_root_store();
    let view = open_mutable_consumer(&mut store, "main", "wv").expect("open");

    if let Some(wv) = store.branches.get_mut("wv") {
        wv.head_revision = 7;
    }
    let resolved = resolve_view(&store, &view).expect("resolve");
    assert_eq!(resolved.branch_id, "wv");
    assert_eq!(resolved.revision, Some(7));
}

#[test]
fn view_ref_round_trips_json() {
    let view = ViewRef {
        schema: default_view_ref_schema(),
        source_root_id: "root_1".to_string(),
        source_branch_id: "main".to_string(),
        kind: ViewKind::ReadOnlyLive,
        source_revision_at_creation: 4,
        consumer_branch_id: None,
    };
    let s = serde_json::to_string(&view).unwrap();
    let back: ViewRef = serde_json::from_str(&s).unwrap();
    assert_eq!(back, view);
}
