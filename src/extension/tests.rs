use serde_json::json;

use crate::trace::BoundaryKind;
use crate::{Component, ContextTree, PactExtensionRecord, PactExtensionWriter, PactFileReadHandle};

fn record(id: &str, path: &str) -> PactExtensionRecord {
    PactExtensionRecord::new(
        "pactree.fs.change",
        "pactree.fs.change.v1",
        id,
        json!({"path": path, "kind": "modified"}),
    )
}

#[test]
fn extension_only_commits_are_durable_and_bounded() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("extensions.pact");
    let mut tree = ContextTree::with_id("extension_test").expect("tree");
    tree.save_file(&path).expect("save baseline");

    tree.stage_extensions(vec![record("change-1", "/one.txt")])
        .expect("stage extension");
    assert!(tree.has_staged());
    let commit = tree
        .seal_staged(BoundaryKind::Adhoc)
        .expect("seal extension")
        .expect("commit");

    let handle = PactFileReadHandle::open(&path, None).expect("open read handle");
    let page = handle
        .extensions(Some("pactree.fs.change"), None, None, "before", 10)
        .expect("read extensions");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].revision, commit.revision);
    assert_eq!(page.rows[0].record.record_id, "change-1");
    assert_eq!(page.rows[0].record.payload["path"], "/one.txt");
    assert!(!page.has_before);
    assert!(!page.has_after);
}

#[test]
fn standalone_extension_writer_appends_without_hydrating_context_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("standalone-extensions.pact");
    let mut tree = ContextTree::with_id("standalone_extension_test").expect("tree");
    tree.insert(
        Component::from_component_value(&json!({
            "id": "standalone-visible",
            "componentType": "text_content",
            "content": "provider context stays intact"
        }))
        .expect("component"),
        "root",
    )
    .expect("stage context");
    tree.seal_staged(BoundaryKind::Adhoc)
        .expect("seal context")
        .expect("context commit");
    tree.save_file(&path).expect("save baseline");
    drop(tree);

    crate::tests::instrumentation::reset_ledger_hydration_count();
    let writer = PactExtensionWriter::open(&path, None).expect("open writer");
    let commit = writer
        .append(vec![record("standalone-change", "/standalone.txt")])
        .expect("append extension");

    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
    let handle = PactFileReadHandle::open(&path, None).expect("read handle");
    assert_eq!(
        handle
            .extension("standalone-change")
            .expect("extension")
            .revision,
        commit.revision
    );
    let reopened = ContextTree::open_file(&path, None).expect("reopen context");
    assert!(
        serde_json::to_string(&reopened.view().expect("view"))
            .expect("encode")
            .contains("provider context stays intact")
    );
}

#[test]
fn standalone_extension_writer_survives_automatic_compaction() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("standalone-extension-compaction.pact");
    let mut tree = ContextTree::with_id("standalone_extension_compaction").expect("tree");
    tree.save_file(&path).expect("save baseline");
    drop(tree);
    let writer = PactExtensionWriter::open(&path, None).expect("open writer");

    for index in 0..40 {
        writer
            .append(vec![record(
                &format!("standalone-{index}"),
                &format!("/{index}.txt"),
            )])
            .expect("append extension");
    }

    let handle = PactFileReadHandle::open(&path, None).expect("read handle");
    let page = handle
        .extensions(Some("pactree.fs.change"), None, None, "before", 80)
        .expect("extension page");
    assert_eq!(page.rows.len(), 40);
    assert_eq!(
        handle
            .extension("standalone-0")
            .expect("oldest direct record")
            .revision,
        1
    );
    assert_eq!(
        ContextTree::open_file(&path, None)
            .expect("reopen context")
            .view()
            .expect("view")
            .head_revision,
        40
    );
}

#[test]
fn extension_pages_use_stable_logical_cursors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("extension-pages.pact");
    let mut tree = ContextTree::with_id("extension_page_test").expect("tree");
    tree.save_file(&path).expect("save baseline");
    for (id, name) in [("c1", "one"), ("c2", "two"), ("c3", "three")] {
        tree.stage_extensions(vec![record(id, &format!("/{name}.txt"))])
            .expect("stage extension");
        tree.seal_staged(BoundaryKind::Adhoc)
            .expect("seal extension")
            .expect("commit");
    }

    let handle = PactFileReadHandle::open(&path, None).expect("open read handle");
    let latest = handle
        .extensions(Some("pactree.fs.change"), None, None, "before", 2)
        .expect("latest page");
    assert_eq!(
        latest
            .rows
            .iter()
            .map(|row| row.record.record_id.as_str())
            .collect::<Vec<_>>(),
        ["c2", "c3"]
    );
    assert!(latest.has_before);
    let older = handle
        .extensions(
            Some("pactree.fs.change"),
            None,
            latest.before_cursor.as_ref(),
            "before",
            2,
        )
        .expect("older page");
    assert_eq!(older.rows[0].record.record_id, "c1");
    assert!(!older.has_before);
    assert!(older.has_after);
}

#[test]
fn extension_windows_can_be_pinned_to_an_earlier_branch_revision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("extension-revision.pact");
    let mut tree = ContextTree::with_id("extension_revision_test").expect("tree");
    tree.save_file(&path).expect("save baseline");
    for (id, name) in [("old", "old"), ("new", "new")] {
        tree.stage_extensions(vec![record(id, &format!("/{name}.txt"))])
            .expect("stage extension");
        tree.seal_staged(BoundaryKind::Adhoc)
            .expect("seal extension")
            .expect("commit");
    }

    let handle = PactFileReadHandle::open(&path, None).expect("open read handle");
    let page = handle
        .extensions_at(Some("pactree.fs.change"), None, None, "before", 10, Some(1))
        .expect("revision-pinned page");

    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.record.record_id.as_str())
            .collect::<Vec<_>>(),
        ["old"]
    );
    assert!(handle.extension_at("old", Some(1)).is_ok());
    assert!(handle.extension_at("new", Some(1)).is_err());
}

#[test]
fn extension_secondary_keys_page_exactly_and_record_ids_resolve_directly() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("extension-secondary.pact");
    let mut tree = ContextTree::with_id("extension_secondary_test").expect("tree");
    tree.save_file(&path).expect("save baseline");

    for (id, path_key) in [("a1", "/a"), ("b1", "/b"), ("a2", "/a")] {
        let mut item = record(id, path_key);
        item.index_keys = vec![format!("path:{path_key}")];
        tree.stage_extensions(vec![item]).expect("stage extension");
        tree.seal_staged(BoundaryKind::Adhoc)
            .expect("seal extension")
            .expect("commit");
    }

    let handle = PactFileReadHandle::open(&path, None).expect("open read handle");
    let latest = handle
        .extensions(
            Some("pactree.fs.change"),
            Some("path:/a"),
            None,
            "before",
            1,
        )
        .expect("latest keyed page");
    assert_eq!(latest.index_key.as_deref(), Some("path:/a"));
    assert_eq!(latest.rows[0].record.record_id, "a2");
    assert!(latest.has_before);

    let older = handle
        .extensions(
            Some("pactree.fs.change"),
            Some("path:/a"),
            latest.before_cursor.as_ref(),
            "before",
            1,
        )
        .expect("older keyed page");
    assert_eq!(older.rows[0].record.record_id, "a1");
    assert!(!older.has_before);
    assert_eq!(
        handle
            .extension("b1")
            .expect("direct record")
            .record
            .payload["path"],
        "/b"
    );
}

#[test]
fn extension_cursor_record_identity_cannot_alias_another_stream() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("extension-cursor-identity.pact");
    let mut tree = ContextTree::with_id("extension_cursor_identity_test").expect("tree");
    tree.save_file(&path).expect("save baseline");
    tree.stage_extensions(vec![record("actual", "/a")])
        .expect("stage extension");
    tree.seal_staged(BoundaryKind::Adhoc)
        .expect("seal extension")
        .expect("commit");

    let handle = PactFileReadHandle::open(&path, None).expect("open read handle");
    let page = handle
        .extensions(Some("pactree.fs.change"), None, None, "before", 1)
        .expect("page");
    let mut cursor = page.before_cursor.expect("cursor");
    cursor.record_id = "different".to_string();
    let error = handle
        .extensions(Some("pactree.fs.change"), None, Some(&cursor), "before", 1)
        .expect_err("mismatched cursor must fail");
    assert!(error.to_string().contains("cursor record"));
}

#[test]
fn duplicate_extension_ids_fail_before_commit() {
    let mut tree = ContextTree::with_id("duplicate_extension_test").expect("tree");
    let error = tree
        .stage_extensions(vec![record("same", "/a"), record("same", "/b")])
        .expect_err("duplicates must fail");
    assert!(
        error
            .to_string()
            .contains("duplicate Pact extension record id")
    );
    assert!(!tree.has_staged());
}

#[test]
fn extensions_join_context_mutations_and_survive_baseline_rewrite() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("joined.pact");
    let rewritten = temp.path().join("joined-rewritten.pact");
    let mut tree = ContextTree::with_id("joined_extension_test").expect("tree");
    tree.save_file(&source).expect("save baseline");

    tree.stage_extensions(vec![record("joined", "/joined.txt")])
        .expect("stage extension");
    tree.insert(
        Component::from_component_value(&json!({
            "id": "placeholder",
            "componentType": "text_content",
            "content": "provider-visible"
        }))
        .expect("component"),
        "root",
    )
    .expect("stage context mutation");
    let commit = tree
        .seal_staged(BoundaryKind::Adhoc)
        .expect("seal joined transaction")
        .expect("commit");

    let mut reopened = ContextTree::open_file(&source, Some("main")).expect("reopen");
    let visible = serde_json::to_string(&reopened.view().expect("view")).expect("encode view");
    assert!(visible.contains("provider-visible"));
    assert!(!visible.contains("pactree.fs.change"));
    reopened.save_file(&rewritten).expect("rewrite baseline");

    let handle = PactFileReadHandle::open(&rewritten, None).expect("open rewritten");
    let page = handle
        .extensions(Some("pactree.fs.change"), None, None, "before", 10)
        .expect("read rewritten extension index");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].revision, commit.revision);
    assert_eq!(page.rows[0].record.record_id, "joined");
    assert_eq!(
        handle.extension("joined").expect("direct rewritten record"),
        page.rows[0]
    );
}

#[test]
fn extension_index_stays_pageable_across_many_large_checkpoint_rows() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("many-extensions.pact");
    let mut tree = ContextTree::with_id("many_extension_test").expect("tree");
    tree.save_file(&path).expect("save baseline");

    for index in 0..64_u64 {
        let mut checkpoint = record(&format!("checkpoint-{index}"), "/workspace.txt");
        checkpoint.kind = "pactree.fs.checkpoint".to_string();
        checkpoint.schema = "pactree.fs.checkpoint.v1".to_string();
        checkpoint.payload = json!({
            "manifest_ref": format!("pfs_blake3_{}", "a".repeat(64)),
            "metadata": { "fixture": "x".repeat(1200), "index": index },
        });
        tree.stage_extensions(vec![checkpoint])
            .expect("stage checkpoint");
        tree.seal_staged(BoundaryKind::Adhoc)
            .expect("seal checkpoint")
            .expect("commit checkpoint");
    }

    let handle = PactFileReadHandle::open(&path, None).expect("open handle");
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = handle
            .extensions(
                Some("pactree.fs.checkpoint"),
                None,
                cursor.as_ref(),
                "before",
                7,
            )
            .expect("page extensions");
        ids.extend(page.rows.iter().map(|row| row.record.record_id.clone()));
        if !page.has_before {
            break;
        }
        cursor = page.before_cursor;
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 64);
}
