use std::sync::Arc;

use crate::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions,
    ProviderThreadOptions,
};

pub(crate) struct SavedFixture {
    pub(crate) temp: tempfile::TempDir,
    pub(crate) path: std::path::PathBuf,
    pub(crate) component_id: String,
    pub(crate) content_id: String,
}

pub(crate) fn saved_fixture(turns: usize) -> SavedFixture {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join(format!("fixture-{turns}.pact"));
    let options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new(format!("fixture_{turns}_"), 1_000_000, 1)
                .expect("deterministic options"),
        ),
    };
    let mut tree = ContextTree::with_id_and_options(format!("fixture_{turns}_root"), options)
        .expect("fixture tree");
    tree.save_file(&path).expect("save fixture");
    let mut component_id = String::new();
    let mut content_id = String::new();
    for turn in 0..turns {
        let authoring = tree.next_authoring_component_options();
        let content = if turn == 0 {
            format!(
                "fixture turn {turn}: {}",
                "externalized fixture content ".repeat(48)
            )
        } else {
            format!("fixture turn {turn}")
        };
        let trace = tree
            .insert(Component::text_authoring(content, authoring), "root")
            .expect("insert fixture component");
        if content_id.is_empty() {
            component_id = trace
                .produced_component_ids
                .first()
                .expect("inserted component id")
                .clone();
            content_id = tree
                .project_component(&component_id)
                .expect("project inserted fixture component")["content_ref"]["id"]
                .as_str()
                .expect("externalized fixture content id")
                .to_string();
        }
        tree.send(&ProviderThreadOptions::default())
            .expect("commit fixture turn");
    }
    SavedFixture {
        temp,
        path,
        component_id,
        content_id,
    }
}

pub(crate) fn fixture_ledger(turns: usize) -> crate::store::ContextTreeLedger {
    let fixture = saved_fixture(turns);
    super::open_ledger(&fixture.path).expect("open fixture ledger")
}

pub(crate) fn transaction_with_every_op() -> super::transaction::PactTransaction {
    use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

    let mut content = crate::store::ContentStore::default();
    let content_record =
        content.put_inline("text", &"externalized every-op content ".repeat(64), 64);
    let content_ref = serde_json::json!({
        "id": content_record.id,
        "hash": content_record.hash,
        "kind": content_record.kind,
        "bytes": content_record.bytes,
    });
    let trace = TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: vec![
            TraceOp::new(
                "inserted",
                TraceOpKind::InsertComponent {
                    parent_id: "root".to_string(),
                    index: 0,
                    component: serde_json::json!({
                        "id": "inserted",
                        "componentType": "text_content",
                        "content": "inserted",
                    }),
                },
            ),
            TraceOp::new("deleted", TraceOpKind::DeleteComponent),
            TraceOp::new(
                "moved",
                TraceOpKind::MoveComponent {
                    parent_id: "root".to_string(),
                    index: 1,
                },
            ),
            TraceOp::new(
                "field",
                TraceOpKind::SetField {
                    key: "role".to_string(),
                    value: serde_json::json!("assistant"),
                },
            ),
            TraceOp::new(
                "field",
                TraceOpKind::UnsetField {
                    key: "obsolete".to_string(),
                },
            ),
            TraceOp::new(
                "attribute",
                TraceOpKind::SetAttr {
                    key: "score".to_string(),
                    value: serde_json::json!(7),
                },
            ),
            TraceOp::new(
                "attribute",
                TraceOpKind::UnsetAttr {
                    key: "stale".to_string(),
                },
            ),
            TraceOp::new(
                "content",
                TraceOpKind::SetContent {
                    content: None,
                    content_ref: Some(content_ref.clone()),
                    content_preview: Some("externalized every-op content".to_string()),
                },
            ),
            TraceOp::new(
                "range",
                TraceOpKind::ReplaceContentRange {
                    start: 2,
                    end: 5,
                    replacement: "updated".to_string(),
                },
            ),
            TraceOp::new(
                "lifecycle",
                TraceOpKind::SetLifecycle {
                    value: serde_json::json!({"state": "active", "revision": 1}),
                },
            ),
        ],
    };
    let pact_ref = crate::store::PactRef {
        root_id: "every_op_root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(1),
        transaction_id: Some("tx_every_op".to_string()),
        component_id: Some("inserted".to_string()),
        content_ref: Some(content_record.id.clone()),
    };
    let public_trace = crate::store::PactTransactionTrace {
        schema: crate::store::TRANSACTION_TRACE_SCHEMA.to_string(),
        root_id: "every_op_root".to_string(),
        branch_id: "main".to_string(),
        transaction_id: "tx_every_op".to_string(),
        authored_ref: pact_ref.clone(),
        pre_request_ref: pact_ref.clone(),
        post_request_alteration_ref: pact_ref.clone(),
        request_ref: Some(pact_ref.clone()),
        response_ref: Some(pact_ref.clone()),
        post_response_alteration_ref: Some(pact_ref.clone()),
        post_response_ref: Some(pact_ref),
        alteration_ids: vec!["alteration_every_op".to_string()],
        commit: None,
        replay_trace: None,
        aux: std::collections::BTreeMap::from([
            ("fixture".to_string(), serde_json::json!("every_op")),
            ("score".to_string(), serde_json::json!(7)),
        ]),
    };

    super::transaction::PactTransaction::new(
        "tx_every_op".to_string(),
        "main".to_string(),
        crate::trace::BoundaryKind::Adhoc,
        trace,
        public_trace,
        vec![content_record],
    )
}

pub(crate) fn fixture_with_text_updates_and_delete() -> SavedFixture {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("fixture-text-mutations.pact");
    let options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new("fixture_mutation_", 2_000_000, 1)
                .expect("deterministic options"),
        ),
    };
    let mut tree =
        ContextTree::with_id_and_options("fixture_mutation_root", options).expect("fixture tree");
    tree.save_file(&path).expect("save fixture");
    tree.insert(
        Component::from_component_value(&serde_json::json!({
            "id": "fixture_mutation_text",
            "componentType": "text_content",
            "content": "initial mutation content ".repeat(64),
        }))
        .expect("mutation component"),
        "root",
    )
    .expect("insert mutation component");
    let content_id = tree
        .project_component("fixture_mutation_text")
        .expect("project mutation component")["content_ref"]["id"]
        .as_str()
        .expect("externalized mutation content id")
        .to_string();
    tree.send(&ProviderThreadOptions::default())
        .expect("commit mutation insert");

    tree.update(
        "fixture_mutation_text",
        crate::ComponentChanges {
            content: Some("updated mutation content ".repeat(64)),
            ..crate::ComponentChanges::default()
        },
    )
    .expect("update mutation content");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit mutation update");
    tree.prune("fixture_mutation_text")
        .expect("delete mutation component");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit mutation delete");

    SavedFixture {
        temp,
        path,
        component_id: "fixture_mutation_text".to_string(),
        content_id,
    }
}

#[test]
fn read_handle_narrow_reads_never_hydrate_the_ledger() {
    let fixture = saved_fixture(20);
    let hydrated = crate::ContextTree::open_file(&fixture.path, None).expect("hydrated oracle");
    let expected_traces = hydrated.traces().expect("oracle traces");
    let expected_rewind_targets = hydrated.rewind_targets().expect("oracle rewind targets");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let handle = crate::PactFileReadHandle::open(&fixture.path, None).expect("open read handle");

    handle.info().expect("info");
    handle
        .component(&fixture.component_id)
        .expect("component by id");
    handle
        .content_payload(&fixture.content_id)
        .expect("content by id");
    handle
        .stream("current_surface", None, None, "before", 10)
        .expect("current stream");
    assert_eq!(handle.traces().expect("traces"), expected_traces);
    assert_eq!(
        handle.rewind_targets().expect("rewind targets"),
        expected_rewind_targets
    );

    assert_eq!(
        crate::tests::instrumentation::ledger_hydration_count(),
        0,
        "narrow read-handle operations must stay on the immutable snapshot"
    );
}

#[test]
fn read_handle_rewind_targets_match_hydrated_message_provenance() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("rewind-targets.pact");
    let mut tree = ContextTree::with_id("rewind_targets_root").expect("tree");
    tree.save_file(&path).expect("save");
    for (role, content) in [("user", "question"), ("assistant", "answer")] {
        let component =
            Component::text_authoring(content.to_string(), tree.next_authoring_component_options());
        tree.add_message(role, vec![component], crate::PactAttrs::new())
            .expect("add message");
        tree.send(&ProviderThreadOptions::default())
            .expect("commit message");
    }
    let hydrated = ContextTree::open_file(&path, None).expect("hydrated oracle");
    let expected = hydrated.rewind_targets().expect("oracle rewind targets");
    crate::tests::instrumentation::reset_ledger_hydration_count();

    let actual = crate::PactFileReadHandle::open(&path, None)
        .expect("read handle")
        .rewind_targets()
        .expect("indexed rewind targets");

    assert_eq!(actual, expected);
    assert_eq!(actual["targets"].as_array().map(Vec::len), Some(2));
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
#[ignore = "requires PACTREE_LENSKEEPER_ARTIFACT evidence path"]
fn lenskeeper_read_handle_traces_and_rewind_targets_match_without_hydration() {
    let path = std::path::PathBuf::from(
        std::env::var("PACTREE_LENSKEEPER_ARTIFACT").expect("artifact path"),
    );
    let hydrated = ContextTree::open_file(&path, None).expect("hydrated Lenskeeper oracle");
    let expected_traces = hydrated.traces().expect("oracle traces");
    let expected_rewind_targets = hydrated.rewind_targets().expect("oracle rewind targets");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let handle = crate::PactFileReadHandle::open(&path, None).expect("Lenskeeper read handle");

    assert_eq!(handle.traces().expect("indexed traces"), expected_traces);
    assert_eq!(
        handle.rewind_targets().expect("indexed rewind targets"),
        expected_rewind_targets
    );
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
fn replay_materializes_each_touched_branch_once() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("linear-replay.pact");

    let root = serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": []
    });
    let ledger: crate::store::ContextTreeLedger = serde_json::from_str(
        &crate::new_ledger_json(&root.to_string(), Some("main"), 1024).expect("ledger"),
    )
    .expect("decode ledger");
    let (ledger, side_branch_id) = ledger
        .fork_reference_at(
            "main",
            Some(0),
            Some("side".to_string()),
            std::collections::BTreeMap::new(),
        )
        .expect("fork side branch at revision 0");
    let bytes = super::encode_file_baseline(&ledger).expect("encode two-branch baseline");
    std::fs::write(&path, bytes).expect("write baseline");

    let mut tree = ContextTree::open_file(&path, Some("main")).expect("reopen main");
    for turn in 0..12 {
        tree.insert(
            Component::from_component_value(&serde_json::json!({
                "id": format!("turn_{turn}"),
                "componentType": "text_content",
                "content": format!("turn {turn}"),
            }))
            .expect("component"),
            "root",
        )
        .expect("insert");
        tree.send(&ProviderThreadOptions::default()).expect("seal");
    }
    let snapshot =
        super::snapshot::Snapshot::open(&path, Arc::new(super::backend::IoCounterSet::default()))
            .expect("snapshot");
    let transactions = snapshot
        .transactions_in_generation_order()
        .expect("transactions");
    assert_eq!(
        transactions.len(),
        13,
        "test file must retain the baseline plus 12 main deltas; if background \
         compaction folded them into the baseline this test proves nothing"
    );

    let (replayed, metrics) =
        super::replay_transactions_with_metrics(transactions).expect("replay");

    assert_eq!(replayed.branches["main"].head_revision, 12);
    assert!(
        replayed.branches.contains_key(&side_branch_id),
        "replay must carry the baseline-only side branch {side_branch_id}"
    );
    assert_eq!(
        metrics.projections_built, 1,
        "replay must build exactly one projection for the single delta-touched \
         branch, no matter how many deltas touch it"
    );
}

#[test]
fn baseline_encoding_reuses_ledger_for_head_metadata() {
    let root = serde_json::json!({
        "id": "metadata_reuse",
        "componentType": "root",
        "children": []
    });
    let ledger: crate::store::ContextTreeLedger = serde_json::from_str(
        &crate::new_ledger_json(&root.to_string(), Some("main"), 1024).expect("ledger"),
    )
    .expect("decode ledger");

    crate::tests::instrumentation::reset_head_metadata_snapshot_decode_count();
    super::encode_file_baseline(&ledger).expect("encode baseline");

    assert_eq!(
        crate::tests::instrumentation::head_metadata_snapshot_decode_count(),
        0,
        "baseline encoding must not clone and deserialize its ledger JSON for metadata"
    );
}

#[test]
fn baseline_encoding_relocates_oversized_head_metadata() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("oversized-head-metadata.pact");
    let root = serde_json::json!({
        "id": "oversized_head_metadata",
        "componentType": "root",
        "children": []
    });
    let mut ledger: crate::store::ContextTreeLedger = serde_json::from_str(
        &crate::new_ledger_json(&root.to_string(), Some("main"), 1024).expect("ledger"),
    )
    .expect("decode ledger");
    let template = ledger.branches["main"].clone();
    for index in 0..32 {
        let branch_id = format!("imported_branch_with_a_long_identity_{index:04}");
        let mut branch = template.clone();
        branch.branch_id = branch_id.clone();
        branch.current.branch_id = branch_id.clone();
        branch.base_ref.branch_id = branch_id.clone();
        ledger.branches.insert(branch_id, branch);
    }
    let bytes = super::encode_file_baseline(&ledger).expect("encode baseline");
    std::fs::write(&path, bytes).expect("write baseline");

    let validation = super::validate::fast_validate(&path).expect("validate baseline");

    assert!(validation.ok, "{:?}", validation.errors);
}

#[test]
fn snapshot_type_facet_returns_dense_bounded_windows() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("type-facet.pact");
    let mut tree = ContextTree::with_id("type_facet").expect("tree");
    for index in 0..40 {
        let options = tree.next_authoring_component_options();
        tree.insert(
            Component::text_authoring(format!("text {index}"), options),
            "root",
        )
        .expect("insert text");
    }
    tree.send(&ProviderThreadOptions::default())
        .expect("seal text components");
    tree.save_file(&path).expect("save baseline");
    let snapshot =
        super::snapshot::Snapshot::open(&path, Arc::new(super::backend::IoCounterSet::default()))
            .expect("snapshot");

    let (total, components) = snapshot
        .components_by_type("main", "text_content", 10, 5)
        .expect("type window");

    assert_eq!(total, 40);
    assert_eq!(components.len(), 5);
    assert!(components.iter().all(|component| {
        component.value["componentType"] == "text_content"
            || component.value["node_type"] == "text_content"
    }));
}

fn message_turn_fixture(
    dir: &std::path::Path,
    turns: usize,
    derived_depth: bool,
    bounded_rows: usize,
) -> std::path::PathBuf {
    use crate::append_writer::{
        PactAppendBatch, PactAppendMessage, PactAppendOptions, PactAppendOutcome, PactAppendWriter,
    };
    let path = dir.join(format!("turns-{turns}-{derived_depth}-{bounded_rows}.pact"));
    let options = ContextTreeOptions {
        derived_depth,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new("dt_".to_string(), 1_000_000, 1).expect("options"),
        ),
    };
    let mut tree = ContextTree::with_id_and_options("dt_root", options).expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..turns {
        let role = if turn % 2 == 0 { "user" } else { "assistant" };
        let parts = (0..2)
            .map(|part| {
                let options = tree.next_authoring_component_options();
                Component::text_authoring(format!("turn {turn} part {part}"), options)
            })
            .collect();
        tree.add_message(role, parts, crate::PactAttrs::new())
            .expect("stage");
        tree.seal_staged(crate::trace::BoundaryKind::Adhoc)
            .expect("seal");
    }
    drop(tree);
    if bounded_rows > 0 {
        let mut writer = PactAppendWriter::open_with_options(
            &path,
            "main",
            crate::PactMemoryOptions::default(),
            PactAppendOptions {
                derived_depth: true,
            },
        )
        .expect("open writer");
        for row in 0..bounded_rows {
            let outcome = writer
                .append(PactAppendBatch {
                    source: "lens".to_string(),
                    batch_id: format!("b{row}"),
                    expected: writer.version().clone(),
                    previous_checkpoint: None,
                    checkpoint: None,
                    messages: vec![PactAppendMessage {
                        row_id: format!("r{row}"),
                        role: "user".to_string(),
                        parts: vec![
                            Component::from_component_value(&serde_json::json!({
                                "componentType": "text_content",
                                "content": format!("bounded row {row}"),
                            }))
                            .expect("part"),
                        ],
                        aux: crate::PactAttrs::new(),
                    }],
                })
                .expect("append");
            assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
        }
        writer.close();
    }
    path
}

#[test]
fn depth_anchor_file_selectors_resolve_through_the_index() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let cases = [
        (
            "unmarked eager",
            message_turn_fixture(temp.path(), 4, false, 0),
            false,
            4,
        ),
        (
            "marked eager",
            message_turn_fixture(temp.path(), 4, true, 0),
            true,
            4,
        ),
        (
            "marked eager plus bounded",
            message_turn_fixture(temp.path(), 2, true, 3),
            true,
            5,
        ),
    ];
    let comparable = |value: &serde_json::Value| {
        serde_json::json!({
            "schema": value["schema"],
            "temporal": value["temporal"],
            "source": value["source"],
            "hits": value["hits"],
        })
    };
    for (label, path, marked, turns) in &cases {
        let turn_count = ContextTree::open_file(path, None)
            .expect("open")
            .head()
            .expect("head")
            .tree
            .root
            .children
            .iter()
            .filter(|child| child.component_type() == "message_turn")
            .count();
        assert_eq!(
            turn_count, *turns,
            "{label}: every row is a root message turn"
        );
        for (selector, indexed_on_unmarked) in [
            ("d0", true),
            ("(d0)", true),
            ("@t0 d0", true),
            ("d0 @limit=1", true),
            ("d1", false),
            ("(d1)", false),
            ("d4", false),
            ("d5", false),
            ("d99", false),
            ("d-1", false),
        ] {
            let query = crate::file_format::PactFileSelectorQuery::new(selector);
            let expected: serde_json::Value = serde_json::from_str(
                &crate::file_format::corpus::file_select_json_full_scan(path, query.clone())
                    .expect("oracle select"),
            )
            .expect("oracle JSON");
            crate::tests::instrumentation::reset_file_selector_full_scan_count();
            let actual: serde_json::Value = serde_json::from_str(
                &crate::file_format::file_select_json(path, query).expect("select"),
            )
            .expect("JSON");
            assert_eq!(
                comparable(&actual),
                comparable(&expected),
                "{label}: {selector} differs from the full scan"
            );
            let scans = crate::tests::instrumentation::file_selector_full_scan_count();
            if selector == "d-1" {
                continue;
            }
            if *marked || indexed_on_unmarked {
                assert_eq!(scans, 0, "{label}: {selector} scanned the corpus");
            }
        }

        let hydrated = ContextTree::open_file(path, None).expect("open");
        for depth in 0..5 {
            let selector = format!("d{depth}");
            let expected = hydrated.select(&selector, false).expect("hydrated select");
            let actual: serde_json::Value = serde_json::from_str(
                &crate::file_format::file_select_json(
                    path,
                    crate::file_format::PactFileSelectorQuery::new(selector.clone()),
                )
                .expect("select"),
            )
            .expect("JSON");
            let ids = actual["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .map(|hit| hit["component_id"].as_str().expect("id").to_string())
                .collect::<Vec<_>>();
            assert_eq!(
                ids,
                expected.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                "{label}: {selector}"
            );
        }
    }
}

#[test]
fn current_type_file_selector_matches_corpus_without_ledger_hydration() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let oracle_path = temp.path().join("selector-oracle.pact");
    let indexed_path = temp.path().join("selector-indexed.pact");
    let mut tree = ContextTree::with_id("selector_type_facet").expect("tree");
    for index in 0..40 {
        let mut options = tree.next_authoring_component_options();
        if index % 4 == 0 {
            options.key = Some("indexed-group".to_string());
        }
        tree.insert(
            Component::text_authoring(format!("text {index}"), options),
            "root",
        )
        .expect("insert text");
    }
    tree.send(&ProviderThreadOptions::default())
        .expect("seal text components");
    tree.save_file(&oracle_path).expect("save baseline");
    std::fs::copy(&oracle_path, &indexed_path).expect("copy candidate");
    let mut query = crate::file_format::PactFileSelectorQuery::new("@t0 .text_content");
    query.limit = 5;
    let expected: serde_json::Value = serde_json::from_str(
        &crate::file_format::corpus::file_select_json_full_scan(&oracle_path, query.clone())
            .expect("oracle select"),
    )
    .expect("oracle JSON");

    crate::tests::instrumentation::reset_ledger_hydration_count();
    crate::tests::instrumentation::reset_file_selector_full_scan_count();
    let actual: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_select_json(&indexed_path, query).expect("indexed select"),
    )
    .expect("indexed JSON");

    assert_eq!(actual, expected);
    assert_eq!(
        crate::tests::instrumentation::file_selector_full_scan_count(),
        0,
        "current type selection must stay on the file index"
    );

    let selected_id = actual["hits"][0]["component_id"]
        .as_str()
        .expect("selected id");
    for (selector, limit, order) in [
        (format!("@t0 #{selected_id}"), 4, "newest"),
        ("@t0 [key=indexed-group]".to_string(), 4, "newest"),
        ("@t0 [role=user]".to_string(), 4, "newest"),
        (
            "@t0 .text_content @offset=3 @limit=4".to_string(),
            9,
            "newest",
        ),
        ("@t0 [key=indexed-group]".to_string(), 4, "oldest"),
    ] {
        let mut query = crate::file_format::PactFileSelectorQuery::new(selector.clone());
        query.limit = limit;
        query.order = order.to_string();
        let expected: serde_json::Value = serde_json::from_str(
            &crate::file_format::corpus::file_select_json_full_scan(&oracle_path, query.clone())
                .expect("oracle indexed facet"),
        )
        .expect("oracle JSON");
        crate::tests::instrumentation::reset_file_selector_full_scan_count();
        let actual: serde_json::Value = serde_json::from_str(
            &crate::file_format::file_select_json(&indexed_path, query).expect("indexed facet"),
        )
        .expect("indexed JSON");
        assert_eq!(actual, expected, "indexed selector changed: {selector}");
        assert_eq!(
            crate::tests::instrumentation::file_selector_full_scan_count(),
            0,
            "indexed selector scanned the corpus: {selector}"
        );
    }
}

#[test]
fn secondary_selector_facets_follow_updates_and_deletes() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("selector-facet-mutations.pact");
    let mut tree = ContextTree::with_id("selector_facet_mutations").expect("tree");
    tree.save_file(&path).expect("save");
    let mut options = tree.next_authoring_component_options();
    options.key = Some("before".to_string());
    let trace = tree
        .insert(
            Component::text_authoring("indexed".to_string(), options),
            "root",
        )
        .expect("insert");
    let component_id = trace.produced_component_ids[0].clone();
    tree.send(&ProviderThreadOptions::default())
        .expect("seal insert");
    let mut changes = crate::ComponentChanges::default();
    changes
        .fields
        .insert("key".to_string(), serde_json::json!("after"));
    tree.update(&component_id, changes).expect("update key");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal update");

    let snapshot =
        super::snapshot::Snapshot::open(&path, Arc::new(super::backend::IoCounterSet::default()))
            .expect("updated snapshot");
    assert_eq!(
        snapshot
            .components_by_aux("main", "key", &serde_json::json!("before"), 0, 10)
            .expect("old key")
            .0,
        0
    );
    assert_eq!(
        snapshot
            .components_by_aux("main", "key", &serde_json::json!("after"), 0, 10)
            .expect("new key")
            .0,
        1
    );

    tree.prune(&component_id).expect("delete component");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal delete");
    let snapshot = snapshot.refresh().expect("deleted snapshot");
    assert_eq!(
        snapshot
            .components_by_type("main", "text_content", 0, 10)
            .expect("deleted type")
            .0,
        0
    );
    assert_eq!(
        snapshot
            .components_by_aux("main", "key", &serde_json::json!("after"), 0, 10)
            .expect("deleted key")
            .0,
        0
    );
}

#[test]
fn public_read_handle_opens_and_streams_without_hydrating_then_refreshes() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("read-handle.pact");
    let mut tree = ContextTree::with_id("indexed_read_handle").expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..12 {
        let component = Component::text_authoring(
            format!("turn {turn}"),
            tree.next_authoring_component_options(),
        );
        tree.add_message("user", vec![component], crate::PactAttrs::new())
            .expect("message");
        tree.send(&ProviderThreadOptions::default()).expect("send");
    }

    crate::tests::instrumentation::reset_ledger_hydration_count();
    let mut handle = crate::PactFileReadHandle::open(&path, None).expect("open handle");
    let info = handle.info().expect("info");
    let before = handle
        .stream("current_surface", None, Some(5), "around", 5)
        .expect("stream");

    assert_eq!(info["branch_id"], "main");
    assert_eq!(info["revision"], 12);
    assert_eq!(before["source"], "held_tree");
    assert_eq!(before["rows"].as_array().map(Vec::len), Some(5));
    assert_eq!(
        crate::tests::instrumentation::ledger_hydration_count(),
        0,
        "opening, info, and bounded current streaming must remain index-only"
    );

    let component = Component::text_authoring(
        "after open".to_string(),
        tree.next_authoring_component_options(),
    );
    tree.add_message("user", vec![component], crate::PactAttrs::new())
        .expect("message");
    tree.send(&ProviderThreadOptions::default()).expect("send");
    assert_eq!(handle.info().expect("stable info")["revision"], 12);
    handle.refresh().expect("refresh");
    assert_eq!(handle.info().expect("refreshed info")["revision"], 13);
}

#[test]
fn indexed_read_handle_stream_matches_held_tree_contract_exactly() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("read-handle-differential.pact");
    let mut tree = ContextTree::with_id("read_handle_differential").expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..8 {
        let component = Component::text_authoring(
            format!("turn {turn}"),
            tree.next_authoring_component_options(),
        );
        tree.add_message("user", vec![component], crate::PactAttrs::new())
            .expect("message");
        tree.send(&ProviderThreadOptions::default()).expect("send");
    }
    let hydrated = ContextTree::open_file(&path, None).expect("hydrated handle");
    let expected = hydrated
        .stream(
            "__pactree_read_handle_current_surface",
            None,
            Some(4),
            "around",
            5,
            Some(&path.display().to_string()),
        )
        .expect("held-tree oracle");
    let indexed = crate::PactFileReadHandle::open(&path, None).expect("indexed handle");
    let actual = indexed
        .stream("current_surface", None, Some(4), "around", 5)
        .expect("indexed stream");

    assert_eq!(actual, expected);
}

#[test]
fn cold_read_handle_component_matches_hydrated_projection_without_ledger_hydration() {
    let fixture = saved_fixture(3);
    let hydrated = ContextTree::open_file(&fixture.path, None).expect("hydrated oracle");
    let expected = hydrated
        .project_component(&fixture.component_id)
        .expect("hydrated component");

    crate::tests::instrumentation::reset_ledger_hydration_count();
    crate::tests::instrumentation::reset_projection_build_count();
    let actual = crate::PactFileReadHandle::open(&fixture.path, None)
        .expect("cold component handle")
        .component(&fixture.component_id)
        .expect("indexed component");

    assert_eq!(actual, expected);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
    assert_eq!(crate::tests::instrumentation::projection_build_count(), 0);
}

#[test]
fn read_handle_overlay_component_lazily_composes_its_parent() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let base_path = temp.path().join("component-overlay-base.pact");
    let child_path = temp.path().join("component-overlay-child.pact");
    let mut tree = ContextTree::with_id("component_overlay_root").expect("tree");
    tree.save_file(&base_path).expect("save base");
    let authoring = tree.next_authoring_component_options();
    let component_id = tree
        .insert(
            Component::text_authoring("composed parent component".to_string(), authoring),
            "root",
        )
        .expect("insert component")
        .produced_component_ids[0]
        .clone();
    tree.send(&ProviderThreadOptions::default())
        .expect("commit component");
    crate::file_format::file_fork_json(
        &base_path,
        Some(&child_path),
        crate::file_format::PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        1024,
    )
    .expect("create overlay child");

    crate::tests::instrumentation::reset_ledger_hydration_count();
    let component = crate::PactFileReadHandle::open(&child_path, None)
        .expect("open overlay handle")
        .component(&component_id)
        .expect("compose parent component");

    assert_eq!(component["content"], "composed parent component");
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 2);
}

#[test]
fn indexed_component_preserves_recursive_subtree_and_local_id_contract() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("recursive-component.pact");
    let mut tree = ContextTree::with_id("recursive_component_root").expect("tree");
    tree.save_file(&path).expect("save");

    let parent = tree
        .insert(
            Component::from_component_value(&serde_json::json!({
                "id": "parent",
                "componentType": "container",
                "children": [],
            }))
            .expect("parent component"),
            "root",
        )
        .expect("insert parent")
        .produced_component_ids[0]
        .clone();
    let child = tree
        .insert(
            Component::from_component_value(&serde_json::json!({
                "id": "child",
                "componentType": "container",
                "children": [],
            }))
            .expect("child component"),
            &parent,
        )
        .expect("insert child")
        .produced_component_ids[0]
        .clone();
    tree.insert(
        Component::from_component_value(&serde_json::json!({
            "id": "grandchild",
            "componentType": "text_content",
            "content": "recursive payload ".repeat(128),
        }))
        .expect("grandchild component"),
        &child,
    )
    .expect("insert grandchild");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit recursive fixture");

    let hydrated = ContextTree::open_file(&path, None).expect("hydrated oracle");
    let expected = hydrated
        .project_component("parent")
        .expect("oracle subtree");

    crate::tests::instrumentation::reset_ledger_hydration_count();
    crate::tests::instrumentation::reset_pact_transaction_decode_count();
    let handle = crate::PactFileReadHandle::open(&path, None).expect("indexed handle");
    let local = handle.component("parent").expect("local-id subtree");
    let qualified = handle.component(&parent).expect("qualified-id subtree");

    assert_eq!(local, expected);
    assert_eq!(qualified, expected);
    assert_eq!(
        local["children"][0]["children"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        local["children"][0]["children"][0]["content"],
        "recursive payload ".repeat(128)
    );
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
    assert_eq!(
        crate::tests::instrumentation::pact_transaction_decode_count(),
        0
    );
}

#[test]
fn indexed_component_tracks_moves_and_subtree_deletes_exactly() {
    fn node(id: &str, content: &str) -> Component {
        Component::from_component_value(&serde_json::json!({
            "id": id,
            "componentType": "text_content",
            "content": content,
        }))
        .expect("component")
    }

    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("component-mutations.pact");
    let mut tree = ContextTree::with_id("component_mutations_root").expect("tree");
    tree.save_file(&path).expect("save");
    let parent_a = tree
        .insert(node("parent_a", "a"), "root")
        .expect("insert a")
        .produced_component_ids[0]
        .clone();
    let parent_b = tree
        .insert(node("parent_b", "b"), "root")
        .expect("insert b")
        .produced_component_ids[0]
        .clone();
    let child = tree
        .insert(node("child", "child"), &parent_a)
        .expect("insert child")
        .produced_component_ids[0]
        .clone();
    let grandchild = tree
        .insert(node("grandchild", "grandchild"), &child)
        .expect("insert grandchild")
        .produced_component_ids[0]
        .clone();
    tree.send(&ProviderThreadOptions::default())
        .expect("commit inserts");

    tree.move_component(
        format!("#{child}").as_str(),
        format!("#{parent_b}").as_str(),
        None,
        None,
    )
    .expect("move subtree");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit move");

    let oracle = ContextTree::open_file(&path, None).expect("move oracle");
    let expected_b = oracle
        .project_component("parent_b")
        .expect("oracle moved subtree");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let handle = crate::PactFileReadHandle::open(&path, None).expect("move handle");
    assert_eq!(
        handle.component("parent_b").expect("indexed moved subtree"),
        expected_b
    );
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);

    tree.prune(format!("#{child}")).expect("delete subtree");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit delete");
    let oracle = ContextTree::open_file(&path, None).expect("delete oracle");
    let expected_b = oracle
        .project_component("parent_b")
        .expect("oracle after delete");
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let handle = crate::PactFileReadHandle::open(&path, None).expect("delete handle");
    assert_eq!(
        handle.component("parent_b").expect("indexed after delete"),
        expected_b
    );
    assert!(handle.component(&child).is_err());
    assert!(handle.component(&grandchild).is_err());
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
fn cold_read_handle_externalized_content_matches_hydrated_payload_without_ledger_hydration() {
    let fixture = saved_fixture(3);
    let hydrated = ContextTree::open_file(&fixture.path, None).expect("hydrated oracle");
    let expected = hydrated
        .content_payload(&fixture.content_id)
        .expect("hydrated content payload");

    crate::tests::instrumentation::reset_ledger_hydration_count();
    crate::tests::instrumentation::reset_projection_build_count();
    crate::tests::instrumentation::reset_pact_transaction_decode_count();
    let actual = crate::PactFileReadHandle::open(&fixture.path, None)
        .expect("cold content handle")
        .content_payload(&fixture.content_id)
        .expect("indexed content payload");

    assert_eq!(actual, expected);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
    assert_eq!(crate::tests::instrumentation::projection_build_count(), 0);
    assert_eq!(
        crate::tests::instrumentation::pact_transaction_decode_count(),
        0
    );
}

#[test]
fn read_handle_lazy_full_surface_survives_source_rename() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("rename-source.pact");
    let renamed = temp.path().join("renamed.pact");
    let mut tree = ContextTree::with_id("read_handle_rename").expect("tree");
    tree.save_file(&path).expect("save");
    let options = tree.next_authoring_component_options();
    let trace = tree
        .insert(
            Component::text_authoring("payload ".repeat(300), options),
            "root",
        )
        .expect("insert");
    tree.send(&ProviderThreadOptions::default()).expect("send");
    let component_id = trace.produced_component_ids[0].clone();
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let handle = crate::PactFileReadHandle::open(&path, None).expect("open handle");
    std::fs::rename(&path, &renamed).expect("rename source");

    let component = handle.component(&component_id).expect("component");
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
    let projected = handle.project(None).expect("project");
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 1);
    let content_id = component["content_ref"]["id"].as_str().expect("content id");
    let payload = handle.content_payload(content_id).expect("content payload");
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 1);

    assert_eq!(component["content"], "payload ".repeat(300));
    assert_eq!(projected.branch.head_revision, 1);
    assert_eq!(payload["text"], "payload ".repeat(300));
    assert!(
        !handle
            .traces()
            .expect("traces")
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        handle.rewind_targets().expect("rewind targets")["schema"],
        "pact.rewind_targets.v1"
    );
}

#[test]
fn read_handle_full_projection_reuses_the_canonical_head_checkpoint() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("shared-ledger.pact");
    let mut tree = ContextTree::with_id("shared_head_checkpoint").expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..8 {
        let options = tree.next_authoring_component_options();
        tree.insert(
            Component::text_authoring(format!("turn {turn}"), options),
            "root",
        )
        .expect("insert");
        tree.send(&ProviderThreadOptions::default()).expect("send");
    }

    crate::tests::instrumentation::reset_ledger_hydration_count();
    let opened = ContextTree::open_file(&path, None).expect("canonical open");
    assert_eq!(opened.head().expect("head").head_revision, 8);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 1);

    let handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
    assert_eq!(handle.project(Some(4)).expect("historical").revision, 4);
    assert_eq!(
        crate::tests::instrumentation::ledger_hydration_count(),
        1,
        "one immutable file snapshot has one hydrated ledger spine while it is owned"
    );
    drop(handle);
    drop(opened);

    let reopened = crate::PactFileReadHandle::open(&path, None).expect("read handle again");
    assert_eq!(reopened.project(Some(4)).expect("historical").revision, 4);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 2);
}
