use std::path::{Path, PathBuf};

use serde_json::json;

use super::{
    PactAppendBatch, PactAppendFailure, PactAppendMessage, PactAppendOptions, PactAppendOutcome,
    PactAppendWriter, PactFileVersion,
};
use crate::pact_file::log::FaultPoint;
use crate::trace::BoundaryKind;
use crate::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs,
    PactMemoryOptions,
};

fn options(prefix: &str) -> DeterministicAuthoringOptions {
    DeterministicAuthoringOptions::new(prefix.to_string(), 1_000_000, 1).expect("options")
}

fn fixture(dir: &Path, turns: usize) -> PathBuf {
    let path = dir.join("writer.pact");
    let tree_options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(options("as_")),
    };
    let mut tree = ContextTree::with_id_and_options("as_root", tree_options).expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..turns {
        let role = if turn % 2 == 0 { "user" } else { "assistant" };
        let parts = (0..2)
            .map(|part| {
                let options = tree.next_authoring_component_options();
                Component::text_authoring(format!("turn {turn} part {part}"), options)
            })
            .collect();
        tree.add_message(role, parts, PactAttrs::new())
            .expect("stage");
        tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    }
    path
}

fn open(path: &Path) -> PactAppendWriter {
    PactAppendWriter::open_with_deterministic_authoring(
        path,
        "main",
        PactMemoryOptions::default(),
        options("rs_"),
    )
    .expect("open writer")
}

fn text(content: &str) -> Component {
    Component::from_component_value(&json!({"componentType": "text_content", "content": content}))
        .expect("part")
}

fn message(row_id: &str, role: &str, content: &str) -> PactAppendMessage {
    PactAppendMessage {
        row_id: row_id.to_string(),
        role: role.to_string(),
        parts: vec![text(content)],
        aux: PactAttrs::new(),
    }
}

fn batch(
    expected: &PactFileVersion,
    batch_id: &str,
    previous: Option<&str>,
    checkpoint: Option<&str>,
    messages: Vec<PactAppendMessage>,
) -> PactAppendBatch {
    PactAppendBatch {
        source: "lens".to_string(),
        batch_id: batch_id.to_string(),
        expected: expected.clone(),
        previous_checkpoint: previous.map(str::to_string),
        checkpoint: checkpoint.map(str::to_string),
        messages,
    }
}

fn message_texts(path: &Path) -> Vec<(String, String)> {
    let tree = ContextTree::open_file(path, None).expect("open");
    let head = tree.head().expect("head");
    head.tree
        .root
        .children
        .iter()
        .filter(|child| child.component_type() == "message_turn")
        .map(|turn| {
            (
                turn.role.clone().unwrap_or_default(),
                turn.children
                    .iter()
                    .filter_map(|part| part.content().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join("|"),
            )
        })
        .collect()
}

#[test]
fn appends_commit_rows_and_receipts_the_oracle_reads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 3);
    let mut writer = open(&path);
    let opened = writer.version().clone();
    assert_eq!(opened.revision, 3);
    assert_eq!(opened.branch_id, "main");

    let outcome = writer
        .append(batch(
            &opened,
            "b1",
            None,
            Some("cp1"),
            vec![
                message("r1", "user", "first question"),
                message("r2", "assistant", "first answer"),
            ],
        ))
        .expect("append");
    let PactAppendOutcome::Committed(receipt) = outcome else {
        panic!("a fresh batch commits");
    };
    assert_eq!(receipt.version.revision, 4);
    assert_eq!(receipt.version.generation, opened.generation + 1);
    assert_eq!(receipt.transaction_id, "tx_main_4");
    assert_eq!(receipt.row_ids, vec!["r1", "r2"]);
    assert_eq!(receipt.checkpoint.as_deref(), Some("cp1"));
    assert_eq!(writer.version().revision, 4);
    assert_eq!(
        writer.checkpoint("lens").expect("checkpoint").as_deref(),
        Some("cp1")
    );
    assert_eq!(
        writer.lookup("lens", "b1").expect("lookup"),
        Some(receipt.clone())
    );
    assert_eq!(writer.lookup("lens", "nope").expect("lookup"), None);
    assert_eq!(writer.checkpoint("other").expect("checkpoint"), None);

    let second = writer
        .append(batch(
            writer.version(),
            "b2",
            Some("cp1"),
            Some("cp2"),
            vec![message("r3", "user", "second question")],
        ))
        .expect("append");
    assert_eq!(second.receipt().version.revision, 5);
    drop(writer);

    let texts = message_texts(&path);
    assert_eq!(texts.len(), 6);
    assert_eq!(texts[3], ("user".to_string(), "first question".to_string()));
    assert_eq!(
        texts[4],
        ("assistant".to_string(), "first answer".to_string())
    );
    assert_eq!(
        texts[5],
        ("user".to_string(), "second question".to_string())
    );

    let handle = crate::PactFileReadHandle::open(&path, Some("main")).expect("read handle");
    let row = handle
        .extension(&super::record_id("lens", "b1"))
        .expect("receipt row");
    assert_eq!(row.record.kind, super::PACT_APPEND_RECEIPT_KIND);
    assert_eq!(row.revision, 4);
    assert_eq!(row.record.payload["batch_id"], "b1");
    assert_eq!(row.record.payload["row_ids"], json!(["r1", "r2"]));
    let row = handle
        .extension(&super::record_id("lens", "b2"))
        .expect("receipt row");
    assert_eq!(row.revision, 5);
}

#[test]
fn exact_retries_return_the_original_receipt_and_reused_keys_conflict() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let mut writer = open(&path);
    let opened = writer.version().clone();
    let first = batch(
        &opened,
        "b1",
        None,
        Some("cp1"),
        vec![message("r1", "user", "one")],
    );
    let original = writer
        .append(first.clone())
        .expect("append")
        .receipt()
        .clone();
    writer
        .append(batch(
            writer.version(),
            "b2",
            Some("cp1"),
            Some("cp2"),
            vec![message("r2", "user", "two")],
        ))
        .expect("later batch");

    let retry = writer.append(first.clone()).expect("retry");
    assert_eq!(retry, PactAppendOutcome::AlreadyCommitted(original.clone()));
    assert_eq!(message_texts(&path).len(), 4, "no duplicate rows");

    let mut changed = first.clone();
    changed.messages[0].parts = vec![text("one, edited")];
    assert!(matches!(
        writer.append(changed),
        Err(PactAppendFailure::IdempotencyConflict)
    ));

    let overlap = batch(
        writer.version(),
        "b3",
        Some("cp2"),
        Some("cp3"),
        vec![
            message("r2", "user", "two again"),
            message("r9", "user", "nine"),
        ],
    );
    assert!(matches!(
        writer.append(overlap),
        Err(PactAppendFailure::IdempotencyConflict)
    ));
    assert_eq!(message_texts(&path).len(), 4, "nothing was written");
}

#[test]
fn stale_versions_predecessors_and_invalid_batches_are_refused_without_writing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let mut writer = open(&path);
    let opened = writer.version().clone();
    let mut stale = opened.clone();
    stale.revision = 1;
    match writer.append(batch(
        &stale,
        "b1",
        None,
        None,
        vec![message("r1", "user", "x")],
    )) {
        Err(PactAppendFailure::Conflict { actual }) => assert_eq!(actual, opened),
        other => panic!("stale revision must conflict, got {other:?}"),
    }
    assert!(matches!(
        writer.append(batch(
            &opened,
            "b1",
            Some("ghost"),
            None,
            vec![message("r1", "user", "x")]
        )),
        Err(PactAppendFailure::Conflict { .. })
    ));
    assert!(matches!(
        writer.append(batch(&opened, "b1", None, None, vec![])),
        Err(PactAppendFailure::InvalidInput(_))
    ));
    assert!(matches!(
        writer.append(batch(
            &opened,
            "b1",
            None,
            None,
            vec![message("r1", "user", "x"), message("r1", "user", "y")]
        )),
        Err(PactAppendFailure::InvalidInput(_))
    ));
    assert!(matches!(
        writer.append(batch(
            &opened,
            "b1",
            None,
            None,
            vec![message("r1", "tool", "x")]
        )),
        Err(PactAppendFailure::InvalidInput(_))
    ));
    let mut empty_batch_id = batch(&opened, "", None, None, vec![message("r1", "user", "x")]);
    empty_batch_id.batch_id = String::new();
    assert!(matches!(
        writer.append(empty_batch_id),
        Err(PactAppendFailure::InvalidInput(_))
    ));
    let mut ttl = message("r1", "user", "x");
    ttl.parts = vec![
        Component::from_component_value(
            &json!({"componentType": "text_content", "content": "x", "ttl": 2}),
        )
        .unwrap(),
    ];
    assert!(matches!(
        writer.append(batch(&opened, "b1", None, None, vec![ttl])),
        Err(PactAppendFailure::UnsupportedSemantics(_))
    ));
    assert_eq!(writer.version(), &opened, "nothing advanced");
    assert_eq!(message_texts(&path).len(), 2);

    writer
        .append(batch(
            &opened,
            "b1",
            None,
            None,
            vec![message("r1", "user", "ok")],
        ))
        .expect("append after rejections");
}

#[test]
fn a_batch_beyond_the_rank_field_is_a_format_limit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let mut writer = open(&path);
    let opened = writer.version().clone();
    let messages = (0..65_537)
        .map(|i| message(&format!("r{i}"), "user", "x"))
        .collect::<Vec<_>>();
    let error = writer
        .append(batch(&opened, "huge", None, None, messages))
        .expect_err("format limit");
    assert!(
        matches!(error, PactAppendFailure::FormatLimit(_)),
        "{error}"
    );
    assert_eq!(error.category(), "format_limit");
    assert_eq!(writer.version(), &opened);
}

#[test]
fn a_fresh_file_commits_its_first_batch_through_the_eager_path_then_bounded() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("fresh.pact");
    let mut writer = open(&path);
    assert!(path.exists(), "open creates the genesis");
    let opened = writer.version().clone();
    assert_eq!(opened.revision, 0);
    let first = writer
        .append(batch(
            &opened,
            "b1",
            None,
            Some("cp1"),
            vec![message("r1", "user", "hello")],
        ))
        .expect("first append");
    assert_eq!(first.receipt().version.revision, 1);
    let second = writer
        .append(batch(
            writer.version(),
            "b2",
            Some("cp1"),
            Some("cp2"),
            vec![message("r2", "assistant", "world")],
        ))
        .expect("second append");
    assert_eq!(second.receipt().version.revision, 2);
    drop(writer);
    assert_eq!(
        message_texts(&path),
        vec![
            ("user".to_string(), "hello".to_string()),
            ("assistant".to_string(), "world".to_string())
        ]
    );
    let reopened = open(&path);
    assert_eq!(
        reopened.checkpoint("lens").expect("checkpoint").as_deref(),
        Some("cp2")
    );
    assert!(reopened.lookup("lens", "b1").expect("lookup").is_some());
}

#[test]
fn concurrent_writers_are_excluded_and_external_advancement_conflicts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let mut writer = open(&path);
    let contended = PactAppendWriter::open(&path, "main", PactMemoryOptions::default());
    assert!(
        matches!(contended, Err(PactAppendFailure::ResourceUnavailable(_))),
        "a second writer cannot take the lock"
    );

    let mut tree = ContextTree::open_file(&path, None).expect("open");
    tree.add_message("user", vec![text("outside")], PactAttrs::new())
        .expect("stage");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    drop(tree);
    let opened = writer.version().clone();
    match writer.append(batch(
        &opened,
        "b1",
        None,
        None,
        vec![message("r1", "user", "x")],
    )) {
        Err(PactAppendFailure::Conflict { .. }) => {}
        other => panic!("external advancement must conflict, got {other:?}"),
    }
    assert_eq!(
        message_texts(&path).len(),
        3,
        "the outside commit stands alone"
    );
}

#[test]
fn faults_before_the_root_are_rejected_and_recovered_after_it_are_unknown() {
    for fault in [
        FaultPoint::DirectoryWritten,
        FaultPoint::AllocatorWritten,
        FaultPoint::RecordsWritten,
        FaultPoint::DataSynced,
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = fixture(temp.path(), 2);
        let before = std::fs::metadata(&path).expect("len").len();
        let mut writer = open(&path);
        let opened = writer.version().clone();
        writer.set_fault_point(Some(fault));
        let error = writer
            .append(batch(
                &opened,
                "b1",
                None,
                Some("cp1"),
                vec![message("r1", "user", "x")],
            ))
            .expect_err("injected fault");
        assert!(
            matches!(error, PactAppendFailure::RejectedIo(_)),
            "{fault:?}: {error}"
        );
        assert_eq!(writer.version(), &opened, "{fault:?}: the old root stands");
        assert!(
            std::fs::metadata(&path).expect("len").len() > before,
            "{fault:?}: the failed candidate left a tail"
        );
        assert_eq!(message_texts(&path).len(), 2);
        assert!(writer.lookup("lens", "b1").expect("lookup").is_none());

        writer.set_fault_point(None);
        let outcome = writer
            .append(batch(
                &opened,
                "b1",
                None,
                Some("cp1"),
                vec![message("r1", "user", "x")],
            ))
            .expect("retry commits");
        assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
        assert_eq!(message_texts(&path).len(), 3, "{fault:?}");
    }

    for fault in [FaultPoint::RootSlotWritten, FaultPoint::RootSynced] {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = fixture(temp.path(), 2);
        let mut writer = open(&path);
        let opened = writer.version().clone();
        writer.set_fault_point(Some(fault));
        let request = batch(
            &opened,
            "b1",
            None,
            Some("cp1"),
            vec![message("r1", "user", "x")],
        );
        let error = writer.append(request.clone()).expect_err("injected fault");
        match &error {
            PactAppendFailure::OutcomeUnknown {
                source, batch_id, ..
            } => {
                assert_eq!((source.as_str(), batch_id.as_str()), ("lens", "b1"));
            }
            other => panic!("{fault:?}: expected an unknown outcome, got {other:?}"),
        }
        assert_eq!(error.category(), "outcome_unknown");
        assert!(
            matches!(
                writer.append(request.clone()),
                Err(PactAppendFailure::ResourceUnavailable(_))
            ),
            "{fault:?}: the handle refuses mutation until reopened"
        );
        drop(writer);

        let mut reopened = open(&path);
        assert_eq!(reopened.version().revision, 3, "{fault:?}");
        let receipt = reopened
            .lookup("lens", "b1")
            .expect("lookup")
            .expect("committed");
        assert_eq!(receipt.row_ids, vec!["r1"]);
        assert_eq!(
            reopened.append(request).expect("retry"),
            PactAppendOutcome::AlreadyCommitted(receipt)
        );
        assert_eq!(message_texts(&path).len(), 3, "{fault:?}: exactly one row");
    }
}

#[test]
fn an_unexplained_tail_is_reported_not_truncated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let mut bytes = std::fs::read(&path).expect("read");
    let before = bytes.len();
    bytes.extend_from_slice(&[0xab; 200]);
    std::fs::write(&path, &bytes).expect("write");
    let mut writer = open(&path);
    let opened = writer.version().clone();
    let error = writer
        .append(batch(
            &opened,
            "b1",
            None,
            None,
            vec![message("r1", "user", "x")],
        ))
        .expect_err("unexplained tail");
    assert!(matches!(error, PactAppendFailure::Corrupt(_)), "{error}");
    assert_eq!(
        std::fs::metadata(&path).expect("len").len() as usize,
        before + 200,
        "the tail is left for explicit recovery"
    );
}

#[test]
fn automatic_maintenance_repacks_when_needed_and_records_debt_on_failure() {
    use crate::pact_file::stream_maintenance::{MaintenancePolicy, RepackStage};

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("auto.pact");
    let mut writer = open(&path);
    writer.set_maintenance_policy(MaintenancePolicy {
        max_hot_segments: 4,
        ..MaintenancePolicy::default()
    });
    let mut repacked_at = None;
    for round in 0..8_u64 {
        let outcome = writer
            .append(batch(
                writer.version(),
                &format!("b{round}"),
                (round > 0).then(|| format!("cp{}", round - 1)).as_deref(),
                Some(&format!("cp{round}")),
                vec![message(
                    &format!("r{round}"),
                    "user",
                    &format!("turn {round}"),
                )],
            ))
            .expect("append");
        assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
        assert!(writer.maintenance_debt().is_none(), "round {round}");
        if let Some(report) = writer.last_maintenance()
            && report.repacked
            && repacked_at.is_none()
        {
            repacked_at = Some(round);
            assert!(
                report.file_bytes_after < report.file_bytes_before,
                "{report:?}"
            );
            assert!(!report.reasons.is_empty(), "{report:?}");
        }
    }
    assert!(
        repacked_at.is_some(),
        "the hot-segment policy triggered a repack"
    );
    assert_eq!(message_texts(&path).len(), 8);
    assert_eq!(writer.version().revision, 8);

    writer.set_maintenance_fault(Some(RepackStage::BeforeReplace));
    let outcome = writer
        .append(batch(
            writer.version(),
            "b8",
            Some("cp7"),
            Some("cp8"),
            vec![message("r8", "user", "turn 8")],
        ))
        .expect("append commits even when maintenance fails");
    assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
    if let Some(debt) = writer.maintenance_debt() {
        assert!(debt.contains("rejected I/O"), "{debt}");
    }
    assert_eq!(writer.version().revision, 9);
    assert!(writer.lookup("lens", "b8").expect("lookup").is_some());
    writer.set_maintenance_fault(None);
    let report = writer.maintain().expect("explicit maintenance");
    assert!(writer.maintenance_debt().is_none());
    assert!(report.generation > 0);
    assert_eq!(
        message_texts(&path).len(),
        9,
        "the receipt's rows survived every repack"
    );
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn writer_is_send_and_sync_for_host_threads() {
    assert_send_sync::<PactAppendWriter>();
}

#[test]
#[ignore = "opt-in: PACT_HISTORY_COPY=<disposable copy of a real .pact>"]
fn bounded_append_timing_probe() {
    use crate::pact_file::append_commit::{pin, publish};
    use crate::pact_file::append_prepare::AppendMessage;
    use crate::pact_file::append_state::AppendState;
    use crate::pact_file::backend::IoCounterSet;
    use std::sync::Arc;
    use std::time::Instant;

    let Some(source) = std::env::var_os("PACT_HISTORY_COPY") else {
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("probe.pact");
    std::fs::copy(&source, &path).expect("copy");
    let options = PactMemoryOptions::default();
    let counters = Arc::new(IoCounterSet::default());
    let lap = |label: &str, started: Instant| {
        let rss = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("VmRSS:"))
                    .map(|line| line.trim_start_matches("VmRSS:").trim().to_string())
            })
            .unwrap_or_default();
        eprintln!(
            "PROBE {label}: {:.1} ms, VmRSS {rss}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    };

    let started = Instant::now();
    let pinned = pin(&path, counters.clone(), &options).expect("pin");
    lap("pin (snapshot open + platform identity)", started);
    let started = Instant::now();
    let mut state = AppendState::bootstrap(&pinned.snapshot, "main", &options).expect("bootstrap");
    lap("bootstrap", started);

    if std::env::var_os("PACT_DERIVED_DEPTH").is_some() {
        state.derived_depth = true;
    }
    let rows = std::env::var("PACT_PROBE_ROWS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    eprintln!(
        "PROBE mode: derived_depth {}, rows {rows}",
        state.derived_depth
    );

    let mut identity = crate::append_writer::Identity::Native(
        crate::resolver::message_authoring::NativeIdentity::default(),
    );
    let started = Instant::now();
    let prepared = state
        .prepare(
            &pinned.snapshot,
            (0..rows)
                .map(|row| AppendMessage {
                    role: if row % 2 == 0 { "user" } else { "assistant" }.to_string(),
                    parts: vec![
                        Component::from_component_value(&serde_json::json!({
                            "componentType": "text_content",
                            "content": format!("probe row {row}"),
                        }))
                        .expect("part"),
                    ],
                    aux: PactAttrs::new(),
                })
                .collect(),
            Vec::new(),
            &mut identity,
            &options,
        )
        .expect("prepare");
    lap("prepare", started);
    eprintln!(
        "PROBE prepared: trace ops {} (shift ops {}), index ops {}, symbols {}, scratch {} B",
        prepared.trace.op_count(),
        prepared.shift_ops,
        prepared.index_ops.len(),
        prepared.trace_symbols,
        prepared.trace_scratch_bytes
    );
    let started = Instant::now();
    let receipt = publish(
        &path, &pinned, &mut state, &prepared, &options, &counters, None,
    )
    .expect("publish");
    lap("publish", started);
    eprintln!(
        "PROBE published: {} bytes written, index root {}",
        receipt.bytes_written, receipt.index_root_address
    );
    let started = Instant::now();
    let pinned = pin(&path, counters.clone(), &options).expect("pin");
    lap("re-pin", started);
    let started = Instant::now();
    let _ = AppendState::bootstrap(&pinned.snapshot, "main", &options).expect("bootstrap");
    lap("re-bootstrap", started);
}

fn depth_ops_by_revision(path: &Path) -> Vec<(u64, usize, usize)> {
    use crate::pact_file::backend::IoCounterSet;
    use crate::pact_file::snapshot::Snapshot;
    use crate::pact_file::transaction_stream::TransactionStream;
    use crate::transaction_trace::TraceOpKind;
    let snapshot =
        Snapshot::open(path, std::sync::Arc::new(IoCounterSet::default())).expect("snapshot");
    TransactionStream::new(&snapshot)
        .map(|item| {
            let (transaction, _) = item.expect("transaction");
            let depth_ops = transaction
                .trace
                .ops
                .iter()
                .filter(|op| {
                    matches!(&op.kind, TraceOpKind::SetField { key, .. } if key == "pact_depth")
                })
                .count();
            (
                transaction.trace.to_rev,
                transaction.trace.ops.len(),
                depth_ops,
            )
        })
        .collect()
}

type TurnDepth = (String, String, Option<i32>, Vec<Option<i32>>);

fn turn_depths(path: &Path) -> Vec<TurnDepth> {
    let tree = ContextTree::open_file(path, None).expect("open");
    let head = tree.head().expect("head");
    head.tree
        .root
        .children
        .iter()
        .filter(|child| child.component_type() == "message_turn")
        .map(|turn| {
            (
                turn.role.clone().unwrap_or_default(),
                turn.children
                    .iter()
                    .filter_map(|part| part.content().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join("|"),
                turn.pact_depth,
                turn.children.iter().map(|part| part.pact_depth).collect(),
            )
        })
        .collect()
}

fn append_rounds(writer: &mut PactAppendWriter, rounds: std::ops::Range<usize>) {
    for round in rounds {
        let expected = writer.version().clone();
        let previous = (round > 0).then(|| format!("c{}", round - 1));
        let outcome = writer
            .append(batch(
                &expected,
                &format!("b{round}"),
                previous.as_deref(),
                Some(&format!("c{round}")),
                vec![
                    message(&format!("r{round}u"), "user", &format!("user {round}")),
                    message(
                        &format!("r{round}a"),
                        "assistant",
                        &format!("assistant {round}"),
                    ),
                ],
            ))
            .expect("append");
        assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
    }
}

#[test]
fn derived_depth_writer_records_no_depth_ops_and_every_reader_derives_depth() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 3);
    let twin = temp.path().join("twin.pact");
    std::fs::copy(&path, &twin).expect("copy");
    let fixture_revision = ContextTree::open_file(&path, None)
        .expect("open")
        .head()
        .expect("head")
        .head_revision;

    let mut derived = PactAppendWriter::open_with_options(
        &path,
        "main",
        PactMemoryOptions::default(),
        PactAppendOptions {
            derived_depth: true,
        },
    )
    .expect("open derived writer");
    append_rounds(&mut derived, 0..3);
    derived.close();
    let mut canonical = open(&twin);
    append_rounds(&mut canonical, 0..3);
    canonical.close();

    let derived_ops = depth_ops_by_revision(&path);
    let canonical_ops = depth_ops_by_revision(&twin);
    let later = |ops: &[(u64, usize, usize)]| {
        ops.iter()
            .filter(|(revision, _, _)| *revision > fixture_revision)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(later(&derived_ops).len(), 3);
    assert!(later(&derived_ops).iter().all(|(_, _, depth)| *depth == 0));
    assert!(later(&canonical_ops).iter().all(|(_, _, depth)| *depth > 0));
    let total = |ops: &[(u64, usize, usize)]| ops.iter().map(|(_, ops, _)| *ops).sum::<usize>();
    assert!(total(&later(&derived_ops)) < total(&later(&canonical_ops)));

    let derived_turns = turn_depths(&path);
    assert_eq!(derived_turns, turn_depths(&twin));
    assert_eq!(derived_turns.len(), 9);
    for (index, (_, _, depth, parts)) in derived_turns.iter().enumerate() {
        let expected = Some((8 - index) as i32);
        assert_eq!(*depth, expected);
        assert!(parts.iter().all(|part| *part == expected));
    }
    assert_eq!(derived_turns.last().expect("newest").1, "assistant 2");

    let tree = ContextTree::open_file(&path, None).expect("open");
    let newest = tree.select("d0", false).expect("select");
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].pact_depth, Some(0));
    assert_eq!(tree.select("d8", false).expect("select").len(), 1);
    assert!(tree.select("d9", false).expect("select").is_empty());

    let handle = crate::PactFileReadHandle::open(&path, None).expect("handle");
    let page = handle
        .stream("message_surface", None, None, "before", 50)
        .expect("stream");
    let rows = page["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 9);
    let total_rows = page["position"]["total_rows"].as_u64().expect("total");
    for row in rows {
        let expected = (total_rows - 1 - row["seq"].as_u64().expect("seq")) as i64;
        assert_eq!(row["payload"]["pact_depth"].as_i64(), Some(expected));
        for child in row["payload"]["children"].as_array().expect("children") {
            assert_eq!(child["pact_depth"].as_i64(), Some(expected));
        }
    }
    let oldest_id = rows[0]["component_id"].as_str().expect("id");
    assert_eq!(
        handle.component(oldest_id).expect("component")["pact_depth"].as_i64(),
        Some(8)
    );
    let oldest_part = rows[0]["payload"]["children"][0]["id"]
        .as_str()
        .expect("part id");
    assert_eq!(
        handle.component(oldest_part).expect("component")["pact_depth"].as_i64(),
        Some(8)
    );

    let projection = tree
        .project_at_revision(fixture_revision + 1)
        .expect("project");
    let depths = projection
        .branch
        .tree
        .root
        .children
        .iter()
        .filter(|child| child.component_type() == "message_turn")
        .map(|child| child.pact_depth)
        .collect::<Vec<_>>();
    assert_eq!(depths, (0..5).rev().map(Some).collect::<Vec<_>>());

    let mut plain = open(&path);
    append_rounds(&mut plain, 3..4);
    plain.close();
    let ops = depth_ops_by_revision(&path);
    assert_eq!(ops.last().expect("last").2, 0);
    let turns = turn_depths(&path);
    assert_eq!(turns.len(), 11);
    assert_eq!(turns[0].2, Some(10));
    assert_eq!(turns.last().expect("newest").2, Some(0));

    let mut repacker = open(&path);
    repacker.set_maintenance_policy(crate::pact_file::stream_maintenance::MaintenancePolicy {
        max_derived_amplification_permille: 0,
        max_hot_segments: 0,
        max_dead_page_permille: 0,
    });
    let report = repacker.maintain().expect("maintain");
    assert!(report.repacked, "the forced policy must repack: {report:?}");
    append_rounds(&mut repacker, 4..5);
    repacker.close();
    {
        use crate::pact_file::backend::IoCounterSet;
        use crate::pact_file::snapshot::Snapshot;
        let snapshot =
            Snapshot::open(&path, std::sync::Arc::new(IoCounterSet::default())).expect("snapshot");
        assert!(
            snapshot
                .metadata()
                .expect("metadata")
                .derived_depth_branches
                .contains("main")
        );
    }
    assert_eq!(depth_ops_by_revision(&path).last().expect("last").2, 0);
    let turns = turn_depths(&path);
    assert_eq!(turns.len(), 13);
    assert_eq!(turns[0].2, Some(12));
    assert_eq!(turns.last().expect("newest").2, Some(0));
    let handle = crate::PactFileReadHandle::open(&path, None).expect("handle");
    let page = handle
        .stream("message_surface", None, None, "before", 3)
        .expect("stream");
    let depths = page["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["payload"]["pact_depth"].as_i64())
        .collect::<Vec<_>>();
    assert_eq!(depths, vec![Some(2), Some(1), Some(0)]);

    assert_eq!(replayed_depth_ops(&path), 0);
    assert!(replayed_depth_ops(&twin) > 0);
}

fn replayed_depth_ops(path: &Path) -> usize {
    use crate::pact_file::backend::IoCounterSet;
    use crate::pact_file::snapshot::Snapshot;
    use crate::transaction_trace::TraceOpKind;
    let snapshot =
        Snapshot::open(path, std::sync::Arc::new(IoCounterSet::default())).expect("snapshot");
    let ledger = snapshot.replay_ledger().expect("ledger");
    let is_depth = |op: &crate::transaction_trace::TraceOp| matches!(&op.kind, TraceOpKind::SetField { key, .. } if key == "pact_depth");
    ledger.branches["main"]
        .transaction_traces
        .iter()
        .flat_map(|trace| trace.ops.iter())
        .filter(|op| is_depth(op))
        .count()
}

#[test]
fn latest_receipt_tells_a_fresh_source_from_a_committed_null_checkpoint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let mut writer = open(&path);
    assert_eq!(writer.latest_receipt("lens").expect("fresh"), None);
    assert_eq!(writer.checkpoint("lens").expect("fresh"), None);
    let expected = writer.version().clone();
    let outcome = writer
        .append(batch(
            &expected,
            "b0",
            None,
            None,
            vec![message("r0", "user", "no checkpoint")],
        ))
        .expect("append");
    let PactAppendOutcome::Committed(receipt) = outcome else {
        panic!("first batch must commit");
    };

    assert_eq!(writer.checkpoint("lens").expect("checkpoint"), None);
    let latest = writer
        .latest_receipt("lens")
        .expect("latest")
        .expect("a receipt");
    assert_eq!(latest, receipt);
    assert_eq!(latest.batch_id, "b0");
    assert_eq!(latest.checkpoint, None);
    assert_eq!(latest.row_ids, vec!["r0".to_string()]);
    assert_eq!(writer.latest_receipt("other").expect("other"), None);
    let expected = writer.version().clone();
    writer
        .append(batch(
            &expected,
            "b1",
            None,
            Some("cp1"),
            vec![message("r1", "assistant", "with checkpoint")],
        ))
        .expect("append");
    let latest = writer
        .latest_receipt("lens")
        .expect("latest")
        .expect("a receipt");
    assert_eq!(latest.batch_id, "b1");
    assert_eq!(latest.checkpoint.as_deref(), Some("cp1"));
    assert_eq!(
        writer.checkpoint("lens").expect("checkpoint").as_deref(),
        Some("cp1")
    );
}

fn root_child_list_ops(path: &Path, from_revision: u64) -> usize {
    use crate::pact_file::backend::IoCounterSet;
    use crate::pact_file::snapshot::Snapshot;
    use crate::pact_file::transaction_stream::TransactionStream;
    use crate::transaction_trace::TraceOpKind;
    let snapshot =
        Snapshot::open(path, std::sync::Arc::new(IoCounterSet::default())).expect("snapshot");
    let root_id = snapshot.metadata().expect("metadata").root_id;
    TransactionStream::new(&snapshot)
        .map(|item| item.expect("transaction").0)
        .filter(|transaction| transaction.trace.from_rev >= from_revision)
        .flat_map(|transaction| transaction.trace.ops.into_iter())
        .filter(|op| {
            op.component_id == root_id
                && matches!(&op.kind, TraceOpKind::SetField { key, .. } if key == "child_ids")
        })
        .count()
}

#[test]
fn tail_appends_record_no_root_child_list_and_replay_identically() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 3);
    let fixture_revision = ContextTree::open_file(&path, None)
        .expect("open")
        .head()
        .expect("head")
        .head_revision;
    let mut writer = open(&path);
    append_rounds(&mut writer, 0..3);
    writer.close();

    assert_eq!(root_child_list_ops(&path, fixture_revision), 0);

    let turns = turn_depths(&path);
    assert_eq!(turns.len(), 9);
    assert_eq!(turns.last().expect("newest").1, "assistant 2");
    let tree = ContextTree::open_file(&path, None).expect("open");
    assert_eq!(tree.select("d0", false).expect("select").len(), 1);
    assert_eq!(tree.select("d8", false).expect("select").len(), 1);
    let handle = crate::PactFileReadHandle::open(&path, None).expect("handle");
    let page = handle
        .stream("message_surface", None, None, "before", 50)
        .expect("stream");
    let rows = page["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 9);
    assert_eq!(
        rows.last().expect("row")["payload"]["children"][0]["content"],
        "assistant 2"
    );
    {
        use crate::pact_file::backend::IoCounterSet;
        use crate::pact_file::snapshot::Snapshot;
        let snapshot =
            Snapshot::open(&path, std::sync::Arc::new(IoCounterSet::default())).expect("snapshot");
        let root_id = snapshot.metadata().expect("metadata").root_id;
        let children = snapshot
            .parent_children("main", &root_id)
            .expect("children");
        assert_eq!(
            children.len(),
            10,
            "system header plus nine turns in facet order"
        );
        let hydrated = tree.head().expect("head");
        let hydrated_order = hydrated
            .tree
            .root
            .children
            .iter()
            .map(|child| child.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(children, hydrated_order);
    }
}

#[test]
fn eager_seals_follow_the_tail_rule_and_skip_depth_ops_on_marked_branches() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let fixture_revision = ContextTree::open_file(&path, None)
        .expect("open")
        .head()
        .expect("head")
        .head_revision;

    let mut tree = ContextTree::open_file(&path, None).expect("open");
    let options = tree.next_authoring_component_options();
    tree.add_message(
        "user",
        vec![Component::text_authoring("eager one".to_string(), options)],
        PactAttrs::new(),
    )
    .expect("stage");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    drop(tree);
    let mut writer = open(&path);
    append_rounds(&mut writer, 0..1);
    writer.close();
    let mut tree = ContextTree::open_file(&path, None).expect("open");
    let options = tree.next_authoring_component_options();
    tree.add_message(
        "assistant",
        vec![Component::text_authoring("eager two".to_string(), options)],
        PactAttrs::new(),
    )
    .expect("stage");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    drop(tree);
    assert_eq!(root_child_list_ops(&path, fixture_revision), 0);
    let turns = turn_depths(&path);
    assert_eq!(turns.len(), 6);
    assert_eq!(turns[2].1, "eager one");
    assert_eq!(turns.last().expect("newest").1, "eager two");
    assert_eq!(turns.last().expect("newest").2, Some(0));
    assert_eq!(turns[0].2, Some(5));
    let handle = crate::PactFileReadHandle::open(&path, None).expect("handle");
    let page = handle
        .stream("message_surface", None, None, "before", 50)
        .expect("stream");
    assert_eq!(page["rows"].as_array().expect("rows").len(), 6);

    let mut marker = PactAppendWriter::open_with_options(
        &path,
        "main",
        PactMemoryOptions::default(),
        PactAppendOptions {
            derived_depth: true,
        },
    )
    .expect("open derived writer");
    append_rounds(&mut marker, 1..2);
    marker.close();
    let mut tree = ContextTree::open_file(&path, None).expect("open");
    let options = tree.next_authoring_component_options();
    tree.add_message(
        "user",
        vec![Component::text_authoring(
            "eager marked".to_string(),
            options,
        )],
        PactAttrs::new(),
    )
    .expect("stage");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    drop(tree);
    let ops = depth_ops_by_revision(&path);
    let (_, _, last_depth_ops) = ops.last().expect("last");
    assert_eq!(
        *last_depth_ops, 0,
        "eager seal on a marked branch recorded depth ops"
    );
    let turns = turn_depths(&path);
    assert_eq!(turns.len(), 9);
    assert_eq!(turns.last().expect("newest").1, "eager marked");
    assert_eq!(turns.last().expect("newest").2, Some(0));
    assert_eq!(turns[0].2, Some(8));
    assert_eq!(root_child_list_ops(&path, fixture_revision), 0);
}
