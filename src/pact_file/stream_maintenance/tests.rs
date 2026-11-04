use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use super::{MaintenancePolicy, RepackStage, assess, remove_stale_candidates, repack};
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::snapshot::Snapshot;
use crate::pact_file::transaction_stream::TransactionStream;
use crate::{
    Component, ContextTree, DeterministicAuthoringOptions, PactAppendBatch, PactAppendMessage,
    PactAppendWriter, PactAttrs, PactMemoryOptions,
};

fn options(prefix: &str) -> DeterministicAuthoringOptions {
    DeterministicAuthoringOptions::new(prefix.to_string(), 1_000_000, 1).expect("options")
}

#[test]
fn message_order_repair_accepts_the_maximum_supported_revision_keys() {
    let mut pages = super::ScratchPages::create(None, 128, 4096).expect("scratch");
    let mut keys = super::CowBTree::from_root(None);
    let orders = [
        0,
        super::message_order_key(u64::MAX >> 16, 0).expect("high revision"),
        super::message_order_key(u64::MAX >> 16, 65_535).expect("maximum key"),
    ];
    for order in orders {
        (keys, _) = keys
            .insert(&mut pages, order.to_be_bytes().to_vec(), Vec::new())
            .expect("key");
    }
    for (rank, order) in orders.into_iter().enumerate() {
        assert_eq!(
            super::repair_key_at_rank(&keys, &pages, rank).expect("rank"),
            order
        );
    }
}

#[test]
fn baseline_repair_seed_includes_the_exact_maximum_order_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = recorded_fixture(dir.path(), 1);
    let id = message_surface_ids(&path)[0].clone();
    let options = PactMemoryOptions::default();
    let mut writer = PactAppendWriter::open(&path, "main", options.clone()).expect("writer");
    writer.set_automatic_maintenance(false);
    let mut high = super::MessageOrderRepairPlan::new(&options).expect("plan");
    high.insert(&id, u64::MAX).expect("maximum key");
    high.rows_to_reindex = 1;
    writer
        .commit_message_order_plan(high)
        .expect("publish maximum key");
    writer.close();
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let address = snapshot
        .index_value(&super::IndexKey::Generation {
            generation: snapshot.generation(),
        })
        .expect("address")
        .expect("transaction");
    let address = super::decode_u64_value(&address, "transaction address").expect("decode");
    let mut seeded = super::MessageOrderRepairPlan::new(&options).expect("seeded plan");
    super::seed_baseline_message_rows(&snapshot, address, "main", &mut seeded)
        .expect("seed immutable index");
    assert_eq!(
        seeded.get(id.as_bytes()).expect("seeded key"),
        Some(u64::MAX)
    );
}

fn part(value: serde_json::Value) -> Component {
    Component::from_component_value(&value).expect("part")
}

fn recorded_fixture(dir: &Path, batches: usize) -> PathBuf {
    let path = dir.join("maintenance.pact");
    let mut writer = PactAppendWriter::open_with_deterministic_authoring(
        &path,
        "main",
        PactMemoryOptions::default(),
        options("mt_"),
    )
    .expect("open writer");
    writer.set_automatic_maintenance(false);
    for batch in 0..batches {
        let mut messages = vec![PactAppendMessage {
            row_id: format!("q{batch}"),
            role: "user".to_string(),
            parts: vec![part(
                json!({"componentType": "text_content", "content": format!("question {batch}")}),
            )],
            aux: PactAttrs::new(),
        }];
        let parts = match batch % 4 {
            0 => vec![part(
                json!({"componentType": "text_content", "content": "b".repeat(2_000 + batch)}),
            )],
            1 => vec![
                part(
                    json!({"componentType": "tool_call", "tool_call_id": format!("call_{batch}"), "tool_name": "search", "parameters": {"q": "x".repeat(1_500)}}),
                ),
                part(
                    json!({"componentType": "tool_result", "tool_call_id": format!("call_{batch}"), "tool_name": "search", "success": true, "result": {"rows": (0..40).map(|i| i.to_string()).collect::<Vec<_>>()}}),
                ),
            ],
            2 => vec![part(
                json!({"componentType": "container", "type": "group", "children": [
                    {"componentType": "text_content", "content": "inner"},
                    {"componentType": "reasoning_content", "content": "why"}
                ]}),
            )],
            _ => vec![part(
                json!({"componentType": "text_content", "content": format!("answer {batch}")}),
            )],
        };
        messages.push(PactAppendMessage {
            row_id: format!("a{batch}"),
            role: "assistant".to_string(),
            parts,
            aux: PactAttrs::new(),
        });
        if batch % 5 == 3 {
            messages.push(PactAppendMessage {
                row_id: format!("s{batch}"),
                role: "system".to_string(),
                parts: vec![part(
                    json!({"componentType": "text_content", "content": format!("rule {batch}")}),
                )],
                aux: PactAttrs::new(),
            });
        }
        writer
            .append(PactAppendBatch {
                source: "lens".to_string(),
                batch_id: format!("b{batch}"),
                expected: writer.version().clone(),
                previous_checkpoint: (batch > 0).then(|| format!("cp{}", batch - 1)),
                checkpoint: Some(format!("cp{batch}")),
                messages,
            })
            .expect("append");
    }
    writer.close();
    path
}

#[derive(Debug, PartialEq)]
struct Observation {
    transactions: Vec<(
        u64,
        String,
        usize,
        serde_json::Value,
        serde_json::Value,
        usize,
    )>,
    head: serde_json::Value,
    metadata: Vec<u8>,
    receipts: Vec<Option<Vec<String>>>,
    checkpoint: Option<String>,
    contents: Vec<(String, serde_json::Value)>,
}

fn observe(path: &Path, batches: usize) -> Observation {
    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let transactions = TransactionStream::new(&snapshot)
        .map(|item| {
            let (transaction, _) = item.expect("transaction");
            (
                transaction.trace.to_rev,
                transaction.transaction_id.clone(),
                transaction.trace.ops.len(),
                serde_json::to_value(&transaction.public_trace).unwrap(),
                serde_json::to_value(&transaction.ledger_delta).unwrap(),
                transaction.new_content.len(),
            )
        })
        .collect::<Vec<_>>();
    let metadata = snapshot
        .index_value(&crate::pact_file::index::IndexKey::Metadata)
        .expect("metadata")
        .expect("present");
    drop(snapshot);
    let tree = ContextTree::open_file(path, None).expect("open");
    let head = tree
        .head()
        .expect("head")
        .tree
        .root
        .project_component_value();
    let reader = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let root_id = head["id"].as_str().unwrap_or_default().to_string();
    let mut contents = Vec::new();
    fn collect(
        value: &serde_json::Value,
        reader: &Snapshot,
        root_id: &str,
        out: &mut Vec<(String, serde_json::Value)>,
    ) {
        let id = value
            .get("id")
            .and_then(|id| id.as_str())
            .unwrap_or_default();
        if id != root_id {
            let component = reader
                .component_subtree("main", id)
                .expect("indexed component")
                .unwrap_or_else(|| panic!("indexed component {id} present"));
            out.push((id.to_string(), component));
        }
        if let Some(children) = value.get("children").and_then(|c| c.as_array()) {
            for child in children {
                collect(child, reader, root_id, out);
            }
        }
    }
    collect(&head, &reader, &root_id, &mut contents);
    drop(reader);
    let writer =
        PactAppendWriter::open(path, "main", PactMemoryOptions::default()).expect("writer");
    let receipts = (0..batches)
        .map(|batch| {
            writer
                .lookup("lens", &format!("b{batch}"))
                .expect("lookup")
                .map(|receipt| receipt.row_ids)
        })
        .collect();
    let checkpoint = writer.checkpoint("lens").expect("checkpoint");
    Observation {
        transactions,
        head,
        metadata,
        receipts,
        checkpoint,
        contents,
    }
}

#[test]
fn assessment_reports_growth_without_hydration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = recorded_fixture(temp.path(), 20);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let assessment = assess(
        &snapshot,
        &PactMemoryOptions::default(),
        &MaintenancePolicy::default(),
    )
    .expect("assess");
    assert!(assessment.generation >= 20, "{assessment:?}");
    assert!(assessment.index_pages > assessment.reachable_index_pages);
    assert!(assessment.dead_page_permille > 0);
    assert!(assessment.needed, "{assessment:?}");
    assert!(assessment.reasons.contains(&"hot segments"));
    let counters = IoCounterSet::default();
    assert_eq!(counters.snapshot().operations_replayed, 0);
}

#[test]
fn repack_preserves_every_observation_and_shrinks_the_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = recorded_fixture(temp.path(), 24);
    let before = observe(&path, 24);
    let bytes_before = std::fs::metadata(&path).expect("len").len();
    let pinned = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("pinned reader");
    let pinned_metadata = pinned.metadata().expect("metadata");

    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let mut stages = Vec::new();
    let report = repack(
        &path,
        &snapshot,
        &PactMemoryOptions::default(),
        &mut |stage| {
            stages.push(stage);
            Ok(())
        },
    )
    .expect("repack");
    assert_eq!(
        stages,
        vec![
            RepackStage::RecordsCopied,
            RepackStage::CandidateComplete,
            RepackStage::BeforeReplace,
            RepackStage::Replaced
        ]
    );
    assert_eq!(report.generation, snapshot.generation());
    assert!(report.bytes_after < bytes_before, "{report:?}");
    assert!(report.transactions >= 24);
    assert!(report.index_entries > 0);

    let after = observe(&path, 24);
    assert_eq!(after, before, "the repacked file reads identically");
    assert_eq!(
        std::fs::metadata(&path).expect("len").len(),
        report.bytes_after
    );
    let reassessed = assess(
        &Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot"),
        &PactMemoryOptions::default(),
        &MaintenancePolicy::default(),
    )
    .expect("assess");
    assert_eq!(reassessed.dead_page_permille, 0);
    assert!(
        reassessed.derived_amplification_permille < 1_250,
        "{reassessed:?}"
    );

    assert_eq!(pinned.metadata().expect("metadata"), pinned_metadata);
    assert!(TransactionStream::new(&pinned).count() >= 24);

    crate::file_format::file_validate_json(&path).expect("validate");
    let mut writer = PactAppendWriter::open_with_deterministic_authoring(
        &path,
        "main",
        PactMemoryOptions::default(),
        options("post_"),
    )
    .expect("reopen writer");
    let outcome = writer
        .append(PactAppendBatch {
            source: "lens".to_string(),
            batch_id: "after-repack".to_string(),
            expected: writer.version().clone(),
            previous_checkpoint: Some("cp23".to_string()),
            checkpoint: Some("cp24".to_string()),
            messages: vec![PactAppendMessage {
                row_id: "post".to_string(),
                role: "user".to_string(),
                parts: vec![part(
                    json!({"componentType": "text_content", "content": "after"}),
                )],
                aux: PactAttrs::new(),
            }],
        })
        .expect("append after repack");
    assert_eq!(
        outcome.receipt().version.revision,
        before.transactions.last().unwrap().0 + 1
    );
    let tree = ContextTree::open_file(&path, None).expect("open");
    assert_eq!(
        tree.head().expect("head").tree.root.children.len(),
        before.head["children"].as_array().unwrap().len() + 1
    );
}

#[test]
fn interrupted_replacements_leave_the_source_intact_and_are_cleaned_up() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = recorded_fixture(temp.path(), 6);
    let before = observe(&path, 6);
    let bytes_before = std::fs::read(&path).expect("read");
    for stage in [
        RepackStage::RecordsCopied,
        RepackStage::CandidateComplete,
        RepackStage::BeforeReplace,
    ] {
        let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
        let error = repack(&path, &snapshot, &PactMemoryOptions::default(), &mut |at| {
            if at == stage {
                Err(crate::PactError::Io(std::io::Error::from_raw_os_error(28)))
            } else {
                Ok(())
            }
        })
        .expect_err("injected failure");
        assert_eq!(error.kind(), crate::PactErrorKind::Io, "{stage:?}");
        assert_eq!(
            std::fs::read(&path).expect("read"),
            bytes_before,
            "{stage:?}: source untouched"
        );
        let leftovers = std::fs::read_dir(temp.path())
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".repack-"))
            .count();
        assert_eq!(leftovers, 0, "{stage:?}: no candidate left behind");
    }
    assert_eq!(observe(&path, 6), before);

    let stale = temp.path().join("maintenance.pact.repack-1-abc.tmp");
    std::fs::write(&stale, b"partial").expect("write");
    assert_eq!(remove_stale_candidates(&path).expect("cleanup"), 1);
    assert!(!stale.exists());
    std::fs::write(&stale, b"partial").expect("write");
    let writer = PactAppendWriter::open(&path, "main", PactMemoryOptions::default()).expect("open");
    assert!(
        !stale.exists(),
        "open removes stale candidates under the lock"
    );
    drop(writer);
}

#[test]
fn source_advancement_during_repack_discards_the_candidate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = recorded_fixture(temp.path(), 5);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let advance_path = path.clone();
    let error = repack(
        &path,
        &snapshot,
        &PactMemoryOptions::default(),
        &mut |stage| {
            if stage == RepackStage::CandidateComplete {
                let mut tree = ContextTree::open_file(&advance_path, None).expect("open");
                tree.add_message(
                    "user",
                    vec![part(
                        json!({"componentType": "text_content", "content": "moved"}),
                    )],
                    PactAttrs::new(),
                )
                .expect("stage");
                tree.seal_staged(crate::trace::BoundaryKind::Adhoc)
                    .expect("seal");
            }
            Ok(())
        },
    )
    .expect_err("advanced source");
    assert!(
        error.to_string().contains("advanced under maintenance"),
        "{error}"
    );
    let reopened = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    assert!(
        reopened.generation() > snapshot.generation()
            || reopened.committed_len() != snapshot.committed_len()
    );
    let leftovers = std::fs::read_dir(temp.path())
        .expect("dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".repack-"))
        .count();
    assert_eq!(leftovers, 0);
}

fn message_order_keys(path: &Path) -> Vec<(u64, String)> {
    use crate::pact_file::index::IndexKey;
    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let key = |order: u64| IndexKey::MessageOrder {
        branch: "main".to_string(),
        order,
    };
    snapshot
        .index_range(&key(0), &key(u64::MAX), 4096, false)
        .expect("range")
        .into_iter()
        .map(|(key, value)| {
            let IndexKey::MessageOrder { order, .. } = IndexKey::decode(&key).expect("key") else {
                panic!("message order namespace");
            };
            (order, String::from_utf8(value).expect("id"))
        })
        .collect()
}

fn canonical_records(path: &Path) -> Vec<(u64, u64, [u8; 32])> {
    use crate::pact_file::index::IndexKey;
    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let mapping = snapshot.mapping();
    let key = |revision: u64| IndexKey::Revision {
        branch: "main".to_string(),
        revision,
    };
    snapshot
        .index_range(&key(0), &key(u64::MAX), 4096, false)
        .expect("range")
        .into_iter()
        .map(|(key, value)| {
            let IndexKey::Revision { revision, .. } = IndexKey::decode(&key).expect("key") else {
                panic!("revision namespace");
            };
            let address = u64::from_le_bytes(value[..8].try_into().expect("address"));
            let mut position = address as usize;
            let record =
                crate::pact_file::codec::read_record(mapping, &mut position).expect("record");
            (revision, address, *blake3::hash(record.payload).as_bytes())
        })
        .collect()
}

fn message_surface_ids(path: &Path) -> Vec<String> {
    let handle = crate::PactFileReadHandle::open(path, None).expect("read handle");
    let page = handle
        .stream("message_surface", None, None, "before", 64)
        .expect("stream");
    page["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            row["component_id"]
                .as_str()
                .expect("component id")
                .to_string()
        })
        .collect()
}

#[test]
fn message_order_repair_reindexes_carried_keys_without_touching_canonical_records() {
    use super::MESSAGE_ORDER_REPAIR_MARKER;
    use crate::pact_file::index::MessageOrderRepairPlan;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = recorded_fixture(dir.path(), 4);
    let order_report =
        |path: &Path| crate::file_format::file_message_order_report(path).expect("order report");
    let before = order_report(&path);
    assert_eq!(before["head_index_matches"]["neither"], 0);
    let keys_before = message_order_keys(&path);
    let canonical_before = canonical_records(&path);
    let ids_before = message_surface_ids(&path);
    assert_eq!(
        ids_before,
        keys_before
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>(),
        "the message surface follows the message order keys"
    );

    let carried = keys_before
        .iter()
        .filter(|(order, _)| order >> 16 == 2)
        .enumerate()
        .map(|(offset, (_, id))| (id.clone(), (2_u64 << 16) + 70_000 + offset as u64))
        .collect::<Vec<_>>();
    assert_eq!(carried.len(), 2);
    let mut writer =
        PactAppendWriter::open(&path, "main", PactMemoryOptions::default()).expect("writer");
    writer.set_automatic_maintenance(false);
    let revision_before = writer.version().revision;
    let mut damage = MessageOrderRepairPlan::new(&PactMemoryOptions::default()).expect("plan");
    for (id, order) in &carried {
        damage.insert(id, *order).expect("row");
    }
    damage.rows_to_reindex = carried.len() as u64;
    let damaged = writer.commit_message_order_plan(damage).expect("damage");
    assert!(damaged.transaction_id.is_some());
    assert_eq!(damaged.revision, revision_before + 1);
    writer.close();
    let report = order_report(&path);
    assert_eq!(report["head_index_matches"]["neither"], 2);
    assert_ne!(message_surface_ids(&path), ids_before);
    assert_eq!(
        &canonical_records(&path)[..canonical_before.len()],
        &canonical_before[..]
    );

    let mut writer =
        PactAppendWriter::open(&path, "main", PactMemoryOptions::default()).expect("writer");
    writer.set_automatic_maintenance(false);
    let repaired = writer.repair_message_order().expect("repair");
    assert!(repaired.transaction_id.is_some());
    assert_eq!(repaired.revision, revision_before + 2);
    assert_eq!(repaired.rows_reindexed, 2);
    assert_eq!(repaired.stale_keys_removed, 2);
    assert_eq!(
        repaired.rows_scanned,
        ids_before.len() as u64,
        "the pruned genesis turn is a baseline row, not a planned one"
    );
    let again = writer.repair_message_order().expect("repair again");
    assert_eq!(again.transaction_id, None);
    assert_eq!(again.revision, repaired.revision);
    assert_eq!(again.rows_reindexed, 0);

    writer
        .append(PactAppendBatch {
            source: "lens".to_string(),
            batch_id: "after-repair".to_string(),
            expected: writer.version().clone(),
            previous_checkpoint: Some("cp3".to_string()),
            checkpoint: Some("cp-after".to_string()),
            messages: vec![PactAppendMessage {
                row_id: "after-1".to_string(),
                role: "user".to_string(),
                parts: vec![part(
                    json!({"componentType": "text_content", "content": "after repair"}),
                )],
                aux: PactAttrs::new(),
            }],
        })
        .expect("append after repair");
    writer.close();

    let after = order_report(&path);
    assert_eq!(after["head_index_matches"]["neither"], 0);
    assert_eq!(after["carried_rows"], before["carried_rows"]);
    let keys_after = message_order_keys(&path);
    assert_eq!(&keys_after[..keys_before.len()], &keys_before[..]);
    assert_eq!(keys_after.len(), keys_before.len() + 1);
    let ids_after = message_surface_ids(&path);
    assert_eq!(&ids_after[..ids_before.len()], &ids_before[..]);
    let canonical_after = canonical_records(&path);
    assert_eq!(
        &canonical_after[..canonical_before.len()],
        &canonical_before[..]
    );
    assert_eq!(canonical_after.len(), canonical_before.len() + 3);

    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let maintenance = TransactionStream::new(&snapshot)
        .map(|item| item.expect("transaction").0)
        .filter(|transaction| {
            transaction.trace.ops.is_empty() && transaction.head_checkpoint.is_none()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        maintenance.len(),
        2,
        "the genesis checkpoint also carries no ops"
    );
    for transaction in &maintenance {
        let public = serde_json::to_value(&transaction.public_trace).expect("public trace");
        assert_eq!(
            public["aux"][MESSAGE_ORDER_REPAIR_MARKER]["schema"], MESSAGE_ORDER_REPAIR_MARKER,
            "{public}"
        );
    }
    drop(snapshot);
    crate::file_format::file_validate_json(&path).expect("validate");
    let tree = ContextTree::open_file(&path, None).expect("eager reopen");
    assert_eq!(
        tree.head().expect("head").tree.root.children.len(),
        1 + ids_before.len() + 1,
        "system header, every recorded turn and the appended turn"
    );
}
