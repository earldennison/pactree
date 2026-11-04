use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pactree::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs,
    PactFileReadHandle, trace::BoundaryKind,
};
use serde_json::Value;

static COUNTER_TESTS: Mutex<()> = Mutex::new(());

fn deterministic_options(prefix: &str) -> ContextTreeOptions {
    ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new(prefix.to_string(), 1_000_000, 1)
                .expect("deterministic authoring options"),
        ),
    }
}

fn append_turn(tree: &mut ContextTree, turn: usize) {
    let role = if turn.is_multiple_of(2) {
        "user"
    } else {
        "assistant"
    };
    let options = tree.next_authoring_component_options();
    tree.add_message(
        role,
        vec![Component::text_authoring(
            format!("turn {turn}: {}", "fixed small part ".repeat(4)),
            options,
        )],
        PactAttrs::new(),
    )
    .expect("stage message");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal turn");
}

fn incremental_oracle(dir: &Path, turns: usize) -> PathBuf {
    let path = dir.join(format!("incremental-{turns}.pact"));
    let mut tree = ContextTree::with_id_and_options("lms_root", deterministic_options("lms_inc_"))
        .expect("tree");
    tree.save_file(&path).expect("create file");
    for turn in 0..turns {
        append_turn(&mut tree, turn);
    }
    path
}

fn cold_oracle(dir: &Path, turns: usize) -> PathBuf {
    let path = dir.join(format!("cold-{turns}.pact"));
    let mut tree = ContextTree::with_id_and_options("lms_root", deterministic_options("lms_cold_"))
        .expect("tree");
    for turn in 0..turns {
        append_turn(&mut tree, turn);
    }
    tree.save_file(&path).expect("save cold baseline");
    path
}

fn stream_rows(window: &Value) -> &Vec<Value> {
    window
        .as_object()
        .expect("stream window object")
        .values()
        .filter_map(Value::as_array)
        .find(|rows| rows.iter().all(|row| row.get("row_id").is_some()) && !rows.is_empty())
        .expect("stream window carries a row array")
}

fn assert_indexed_view_matches_materialized(path: &Path, turns: usize) {
    let handle = PactFileReadHandle::open(path, None).expect("indexed open");
    let info = handle.info().expect("info");
    let inspect: Value =
        serde_json::from_str(&pactree::file_format::file_inspect_json(path).expect("inspect"))
            .expect("inspect json");
    assert_eq!(
        info["stats"]["component_count"], inspect["component_count"],
        "indexed component count must equal the materialized count"
    );
    let window = handle
        .stream("current_surface", None, None, "before", 20)
        .expect("latest window");
    let rows = stream_rows(&window);

    let message_rows = rows
        .iter()
        .filter(|row| row["node_type"] == "message_turn" && row["component_id"] != "active")
        .collect::<Vec<_>>();
    let non_message_rows = rows.len() - message_rows.len();
    assert_eq!(
        message_rows.len(),
        turns.min(20 - non_message_rows),
        "message rows in the latest window; all rows: {:?}",
        rows.iter()
            .map(|row| (
                row["kind"].clone(),
                row["node_type"].clone(),
                row["role"].clone()
            ))
            .collect::<Vec<_>>()
    );
    let seqs = message_rows
        .iter()
        .map(|row| row["seq"].as_u64().expect("seq"))
        .collect::<Vec<_>>();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(seqs, sorted, "latest window is in ascending message order");
    let report = pactree::file_format::file_message_order_report(path).expect("order report");
    assert_eq!(report["message_rows"].as_u64(), Some(turns as u64));
    assert_eq!(report["head_index_matches"]["neither"], 0, "{report}");
}

#[test]
fn incremental_oracle_indexed_view_matches_materialized_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = incremental_oracle(temp.path(), 40);
    assert_indexed_view_matches_materialized(&path, 40);
}

#[test]
fn cold_oracle_indexed_view_matches_materialized_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = cold_oracle(temp.path(), 40);
    assert_indexed_view_matches_materialized(&path, 40);
}

#[test]
fn one_more_turn_on_an_oracle_file_keeps_the_index_consistent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = incremental_oracle(temp.path(), 12);
    let before = PactFileReadHandle::open(&path, None)
        .expect("open")
        .info()
        .expect("info")["revision"]
        .as_u64()
        .expect("revision");
    let mut tree = ContextTree::open_file(&path, None).expect("materialized open");
    append_turn(&mut tree, 12);
    drop(tree);
    let after = PactFileReadHandle::open(&path, None).expect("reopen");
    assert_eq!(
        after.info().expect("info")["revision"].as_u64(),
        Some(before + 1)
    );
    assert_indexed_view_matches_materialized(&path, 13);
}

#[test]
fn indexed_open_replays_nothing_and_materialized_open_hydrates_once() {
    let _guard = COUNTER_TESTS.lock().expect("counter test lock");
    let temp = tempfile::tempdir().expect("tempdir");

    let written = incremental_oracle(temp.path(), 8);
    let path = temp.path().join("fresh-path.pact");
    std::fs::copy(&written, &path).expect("copy fixture");

    pactree::reset_storage_counters();
    let handle = PactFileReadHandle::open(&path, None).expect("indexed open");
    handle.info().expect("info");
    handle
        .stream("current_surface", None, None, "before", 20)
        .expect("latest window");
    let indexed = pactree::storage_counters();
    assert_eq!(indexed.operations_replayed, 0, "{indexed:?}");
    assert_eq!(indexed.ledger_hydrations, 0, "{indexed:?}");
    drop(handle);

    pactree::reset_storage_counters();
    let tree = ContextTree::open_file(&path, None).expect("materialized open");
    tree.head().expect("head");
    let materialized = pactree::storage_counters();
    assert_eq!(materialized.ledger_hydrations, 1, "{materialized:?}");

    assert!(materialized.records_decoded >= 1, "{materialized:?}");
}

#[test]
fn inspect_summary_from_the_index_matches_the_hydrated_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut files = vec![
        incremental_oracle(dir.path(), 6),
        cold_oracle(dir.path(), 6),
    ];
    let appended = incremental_oracle(dir.path().join("appended").as_path(), 4);
    let mut writer =
        pactree::PactAppendWriter::open(&appended, "main", pactree::PactMemoryOptions::default())
            .expect("writer");
    writer.set_automatic_maintenance(false);
    for batch in 0..3 {
        writer
            .append(pactree::PactAppendBatch {
                source: "lens".to_string(),
                batch_id: format!("b{batch}"),
                expected: writer.version().clone(),
                previous_checkpoint: (batch > 0).then(|| format!("cp{}", batch - 1)),
                checkpoint: Some(format!("cp{batch}")),
                messages: vec![pactree::PactAppendMessage {
                    row_id: format!("r{batch}"),
                    role: "user".to_string(),
                    parts: vec![
                        pactree::Component::from_component_value(&serde_json::json!({
                            "componentType": "text_content",
                            "content": format!("bounded turn {batch}"),
                        }))
                        .expect("part"),
                    ],
                    aux: pactree::PactAttrs::new(),
                }],
            })
            .expect("append");
    }
    writer.close();
    files.push(appended);
    for path in files {
        let summary: Value =
            serde_json::from_str(&pactree::file_format::file_inspect_json(&path).expect("summary"))
                .expect("summary json");
        let full: Value = serde_json::from_str(
            &pactree::file_format::file_inspect_full_json(&path).expect("full"),
        )
        .expect("full json");

        assert_eq!(
            summary["capabilities"],
            serde_json::json!(["canonical-ledger", "root-slots"])
        );

        assert_eq!(
            summary["record_count"].as_u64(),
            Some(
                summary["node_count"].as_u64().expect("nodes")
                    + summary["trace_count"].as_u64().expect("traces")
                    + 3
            )
        );
        for field in [
            "schema",
            "version",
            "file_kind",
            "file_bytes",
            "branch_count",
            "node_count",
            "trace_count",
            "component_count",
            "commit_count",
            "head_count",
            "last_valid_offset",
        ] {
            assert_eq!(
                summary[field],
                full[field],
                "{field} differs for {}",
                path.display()
            );
        }
        assert_eq!(summary["cycle_count"], summary["commit_count"]);
        assert!(summary["node_count"].as_u64().expect("count") > 1);
    }
}

#[test]
#[ignore]
fn depth_anchor_selector_probe_on_a_history_copy() {
    let Some(source) = std::env::var_os("PACT_HISTORY_COPY") else {
        return;
    };
    let depth = std::env::var("PACT_PROBE_DEPTH").unwrap_or_else(|_| "1404".to_string());
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("probe.pact");
    std::fs::copy(&source, &path).expect("copy");
    let rss = || {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("VmRSS:"))
                    .map(|line| line.trim_start_matches("VmRSS:").trim().to_string())
            })
            .unwrap_or_default()
    };
    let started = std::time::Instant::now();
    let mut query = pactree::file_format::PactFileSelectorQuery::new(format!("d{depth}"));
    query.limit = 1;
    let output = pactree::file_format::file_select_json(&path, query).expect("select");
    let elapsed = started.elapsed();
    let value: Value = serde_json::from_str(&output).expect("json");
    let hits = value["hits"].as_array().expect("hits");
    eprintln!(
        "PROBE d{depth}: {:.1} ms, VmRSS {}, hits {}, first {}",
        elapsed.as_secs_f64() * 1000.0,
        rss(),
        hits.len(),
        hits.first()
            .and_then(|hit| hit["component_id"].as_str())
            .unwrap_or("-")
    );
    assert_eq!(hits.len(), 1, "one turn at depth {depth}");
    assert_eq!(value["selector_stats"]["fallback_used"], false);
}
