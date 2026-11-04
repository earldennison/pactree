use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::head_ledger;
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::snapshot::Snapshot;
use crate::trace::BoundaryKind;
use crate::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAppendBatch,
    PactAppendMessage, PactAppendOptions, PactAppendWriter, PactAttrs, PactMemoryOptions,
};

fn options(prefix: &str) -> DeterministicAuthoringOptions {
    DeterministicAuthoringOptions::new(prefix.to_string(), 1_000_000, 1).expect("options")
}

fn fixture(dir: &Path, derived: bool) -> PathBuf {
    let path = dir.join(if derived {
        "derived.pact"
    } else {
        "plain.pact"
    });
    let mut tree = ContextTree::with_id_and_options(
        "fp_root",
        ContextTreeOptions {
            derived_depth: false,
            deterministic_authoring: Some(options("fp_")),
        },
    )
    .expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..3 {
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
    drop(tree);
    let mut writer = PactAppendWriter::open_with_options(
        &path,
        "main",
        PactMemoryOptions::default(),
        PactAppendOptions {
            derived_depth: derived,
        },
    )
    .expect("writer");
    for round in 0..3 {
        let expected = writer.version().clone();
        let previous = (round > 0).then(|| format!("c{}", round - 1));
        writer
            .append(PactAppendBatch {
                source: "lens".to_string(),
                batch_id: format!("b{round}"),
                expected,
                previous_checkpoint: previous,
                checkpoint: Some(format!("c{round}")),
                messages: vec![
                    PactAppendMessage {
                        row_id: format!("r{round}u"),
                        role: "user".to_string(),
                        parts: vec![
                            Component::from_component_value(&serde_json::json!({
                                "componentType": "text_content",
                                "content": format!("user {round}"),
                            }))
                            .expect("part"),
                        ],
                        aux: PactAttrs::new(),
                    },
                    PactAppendMessage {
                        row_id: format!("r{round}a"),
                        role: "assistant".to_string(),
                        parts: vec![
                            Component::from_component_value(&serde_json::json!({
                                "componentType": "text_content",
                                "content": format!("assistant {round}"),
                            }))
                            .expect("part"),
                        ],
                        aux: PactAttrs::new(),
                    },
                ],
            })
            .expect("append");
    }
    writer.close();
    path
}

fn first_difference(
    left: &serde_json::Value,
    right: &serde_json::Value,
    path: &str,
) -> Option<String> {
    use serde_json::Value;
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                match (left.get(key), right.get(key)) {
                    (Some(l), Some(r)) => {
                        if let Some(found) = first_difference(l, r, &format!("{path}.{key}")) {
                            return Some(found);
                        }
                    }
                    (l, r) => {
                        return Some(format!(
                            "{path}.{key}: {} vs {}",
                            l.map_or("missing".to_string(), |v| v.to_string()),
                            r.map_or("missing".to_string(), |v| v.to_string())
                        ));
                    }
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!("{path}: lengths {} != {}", left.len(), right.len()));
            }
            left.iter()
                .zip(right)
                .enumerate()
                .find_map(|(index, (l, r))| first_difference(l, r, &format!("{path}[{index}]")))
        }
        _ => (left != right).then(|| format!("{path}: {left} vs {right}")),
    }
}

#[test]
fn facet_head_matches_the_replayed_head() {
    let temp = tempfile::tempdir().expect("tempdir");
    for derived in [false, true] {
        let path = fixture(temp.path(), derived);
        let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
        let head = head_ledger(&snapshot).expect("head ledger");
        let full = snapshot.replay_ledger().expect("full ledger");
        if !derived {
            assert!(
                head.is_none(),
                "unmarked branch must fall back to the full replay"
            );
            continue;
        }
        let head = head.expect("a marked branch assembles its head from the facets");
        assert!(head.is_lazy() && !full.is_lazy());
        assert_eq!(head.root_id, full.root_id);
        assert_eq!(head.current_branch_id, full.current_branch_id);
        assert_eq!(head.trace_count(), full.trace_count(), "derived {derived}");
        assert_eq!(head.parent, full.parent);
        let (hb, fb) = (&head.branches["main"], &full.branches["main"]);
        assert_eq!(hb.head_revision, fb.head_revision);
        assert_eq!(hb.lifecycle_tick, fb.lifecycle_tick);
        assert_eq!(hb.base_ref, fb.base_ref);
        assert_eq!(hb.aux, fb.aux, "derived {derived}");
        let left = serde_json::to_value(&hb.current).expect("view");
        let right = serde_json::to_value(&fb.current).expect("view");
        assert_eq!(
            first_difference(&left, &right, "current"),
            None,
            "derived {derived}"
        );

        for (id, record) in &head.content.records {
            assert_eq!(Some(record), full.content.records.get(id), "content {id}");
        }

        let (newest_id, newest) = head
            .event_index
            .transaction_traces
            .iter()
            .next()
            .expect("newest trace");
        assert_eq!(
            Some(newest),
            full.event_index.transaction_traces.get(newest_id)
        );
        assert_eq!(head.event_index.transaction_traces.len(), 1);
    }
}

#[test]
fn open_file_is_head_only_and_history_loads_on_demand() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), true);
    let lazy = ContextTree::open_file(&path, None).expect("open");
    assert!(lazy.resolver_ledger_is_lazy());
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let full = ContextTree::from_file_snapshot(
        Arc::new(snapshot.replay_ledger().expect("full")),
        path.clone(),
        "main",
    )
    .expect("full tree");
    assert_eq!(lazy.head().expect("head"), full.head().expect("head"));
    for selector in [
        "d0",
        "d3",
        "d8",
        ".text_content",
        ".message_turn{role='assistant'}",
    ] {
        let l = lazy.select(selector, false).expect("select");
        let r = full.select(selector, false).expect("select");
        assert_eq!(
            serde_json::to_value(&l).unwrap(),
            serde_json::to_value(&r).unwrap(),
            "{selector}"
        );
    }

    assert_eq!(
        lazy.traces().expect("traces"),
        full.traces().expect("traces")
    );
    assert_eq!(
        lazy.rewind_targets().expect("targets"),
        full.rewind_targets().expect("targets")
    );
    let l = lazy.project_at_revision(2).expect("project");
    let r = full.project_at_revision(2).expect("project");
    assert_eq!(
        serde_json::to_value(&l.branch).unwrap(),
        serde_json::to_value(&r.branch).unwrap()
    );
    let stream_l = lazy
        .stream("current_surface", None, None, "before", 20, None)
        .expect("stream");
    let stream_r = full
        .stream("current_surface", None, None, "before", 20, None)
        .expect("stream");
    assert_eq!(stream_l["rows"], stream_r["rows"]);
    assert_eq!(
        stream_l["head"]["transaction_id"],
        stream_r["head"]["transaction_id"]
    );

    let mut lazy = lazy;
    let options = lazy.next_authoring_component_options();
    lazy.add_message(
        "user",
        vec![Component::text_authoring("after open".to_string(), options)],
        PactAttrs::new(),
    )
    .expect("stage");
    lazy.seal_staged(BoundaryKind::Adhoc).expect("seal");
    let lazy_head = lazy.head().expect("head").clone();
    let reopened = Snapshot::open(&path, Arc::new(IoCounterSet::default()))
        .expect("snapshot")
        .replay_ledger()
        .expect("full");
    let full_head = &reopened.branches["main"].current;
    assert_eq!(
        first_difference(
            &serde_json::to_value(&lazy_head).unwrap(),
            &serde_json::to_value(full_head).unwrap(),
            "head"
        ),
        None
    );
    assert_eq!(reopened.trace_count(), lazy.resolver_trace_count());
}

fn run_turn_cycle(
    tree: &mut ContextTree,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, String) {
    use crate::ProviderThreadOptions;
    let note = Component::text_authoring(
        "system note".to_string(),
        tree.next_authoring_component_options(),
    );
    tree.insert(note, "d-1").expect("mount at d-1");
    let mut expiring = Component::text_authoring(
        "expires after two episodes".to_string(),
        tree.next_authoring_component_options(),
    );
    expiring.lifecycle.policy.ttl = Some(2);
    let expiring_id = tree
        .insert(expiring, (0, 1))
        .expect("mount at (d0,1)")
        .produced_component_ids
        .first()
        .expect("expiring id")
        .clone();
    let mut threads = Vec::new();
    let mut reports = Vec::new();
    for cycle in 0..3 {
        threads.push(tree.send(&ProviderThreadOptions::default()).expect("send"));
        let reply = Component::text_authoring(
            format!("reply {cycle}"),
            tree.next_authoring_component_options(),
        );
        reports.push(
            tree.receive(vec![reply], PactAttrs::new())
                .expect("receive"),
        );
    }
    (threads, reports, expiring_id)
}

fn head_contains(tree: &ContextTree, component_id: &str) -> bool {
    fn walk(component: &Component, id: &str) -> bool {
        component.id == id || component.children.iter().any(|child| walk(child, id))
    }
    walk(&tree.head().expect("head").tree.root, component_id)
}

#[test]
fn the_framework_turn_cycle_runs_on_the_facet_built_head() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), true);
    let twin = temp.path().join("twin.pact");
    std::fs::copy(&path, &twin).expect("copy");
    let authoring = || {
        Some(DeterministicAuthoringOptions::new("cyc_".to_string(), 2_000_000, 1).expect("options"))
    };

    crate::tests::instrumentation::reset_pact_transaction_decode_count();
    crate::tests::instrumentation::reset_full_history_loads();
    let mut lazy = ContextTree::open_file_with_options(
        &path,
        Some("main"),
        ContextTreeOptions {
            derived_depth: false,
            deterministic_authoring: authoring(),
        },
    )
    .expect("open");
    assert!(lazy.resolver_ledger_is_lazy());
    let decoded_by_open = crate::tests::instrumentation::pact_transaction_decode_count();
    assert!(
        decoded_by_open <= 1,
        "open decoded {decoded_by_open} transactions"
    );
    let (lazy_threads, lazy_reports, expiring_id) = run_turn_cycle(&mut lazy);
    assert!(
        lazy.resolver_ledger_is_lazy(),
        "the cycle keeps the head-only ledger"
    );
    let decoded_by_cycle =
        crate::tests::instrumentation::pact_transaction_decode_count() - decoded_by_open;
    const SEALS: usize = 6;
    const ROOT_SLOTS: usize = 2;
    assert!(
        decoded_by_cycle <= SEALS * ROOT_SLOTS,
        "the turn cycle decoded {decoded_by_cycle} transactions, more than its commits' tail checks"
    );
    assert_eq!(
        crate::tests::instrumentation::full_history_loads(),
        0,
        "the turn cycle loaded the full history"
    );
    let lazy_head = lazy.head().expect("head").clone();
    assert!(
        !head_contains(&lazy, &expiring_id),
        "the ttl 2 part expired after two episodes"
    );

    let snapshot = Snapshot::open(&twin, Arc::new(IoCounterSet::default())).expect("snapshot");
    let mut full = ContextTree::from_file_snapshot(
        Arc::new(snapshot.replay_ledger().expect("full")),
        twin.clone(),
        "main",
    )
    .expect("full tree");
    full.set_deterministic_authoring(authoring())
        .expect("deterministic twin");
    assert!(!full.resolver_ledger_is_lazy());
    let (full_threads, full_reports, full_expiring_id) = run_turn_cycle(&mut full);
    assert_eq!(expiring_id, full_expiring_id);
    assert_eq!(lazy_threads, full_threads, "provider threads");
    assert_eq!(lazy_reports, full_reports, "receive reports");
    assert_eq!(
        first_difference(
            &serde_json::to_value(&lazy_head).unwrap(),
            &serde_json::to_value(full.head().expect("head")).unwrap(),
            "head"
        ),
        None
    );
    assert!(!head_contains(&full, &expiring_id));

    let head_revision = lazy_head.head_revision;
    let fork_at = head_revision - 2;
    let fork_of = |source: &std::path::Path, output: &std::path::Path| {
        crate::file_format::file_fork_json(
            source,
            Some(output),
            crate::file_format::PactForkMode::Hard,
            Some("main"),
            Some(fork_at),
            None,
            1024,
        )
        .expect("hard fork");
        ContextTree::open_file(output, None)
            .expect("open fork")
            .head()
            .expect("fork head")
            .clone()
    };

    lazy.traces().expect("traces");
    assert_eq!(crate::tests::instrumentation::full_history_loads(), 1);
    let before_fork = crate::tests::instrumentation::pact_transaction_decode_count();
    let lazy_fork = fork_of(&path, &temp.path().join("lazy-fork.pact"));
    assert!(
        crate::tests::instrumentation::pact_transaction_decode_count() > before_fork,
        "the fork is what loads the history"
    );
    let full_fork = fork_of(&twin, &temp.path().join("full-fork.pact"));
    assert_eq!(lazy_fork.head_revision, full_fork.head_revision);

    let neutral_fork_tree = |view: &crate::PactView| -> serde_json::Value {
        let mut tree = serde_json::to_value(&view.tree).unwrap();
        let root_id = tree["root"]["id"].as_str().unwrap_or_default().to_string();
        let text = serde_json::to_string(&tree)
            .unwrap()
            .replace(&root_id, "FORK_ROOT");
        tree = serde_json::from_str(&text).unwrap();
        tree
    };
    assert_eq!(
        first_difference(
            &neutral_fork_tree(&lazy_fork),
            &neutral_fork_tree(&full_fork),
            "fork.tree"
        ),
        None
    );

    let reopened = ContextTree::open_file(&path, None).expect("reopen");
    let replayed = Snapshot::open(&path, Arc::new(IoCounterSet::default()))
        .expect("snapshot")
        .replay_ledger()
        .expect("full");
    assert_eq!(
        first_difference(
            &serde_json::to_value(reopened.head().expect("head")).unwrap(),
            &serde_json::to_value(&replayed.branches["main"].current).unwrap(),
            "reopened"
        ),
        None
    );
    let twin_reopened = ContextTree::open_file(&twin, None).expect("reopen twin");
    assert_eq!(
        first_difference(
            &serde_json::to_value(reopened.head().expect("head")).unwrap(),
            &serde_json::to_value(twin_reopened.head().expect("head")).unwrap(),
            "twin"
        ),
        None
    );
}

#[test]
#[ignore = "opt-in real-copy probe"]
fn head_only_open_probe() {
    let Some(source) = std::env::var_os("PACT_HISTORY_COPY") else {
        return;
    };
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
    let peak = || {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("VmHWM:"))
                    .map(|line| line.trim_start_matches("VmHWM:").trim().to_string())
            })
            .unwrap_or_default()
    };
    eprintln!("PROBE before open: VmRSS {}, VmHWM {}", rss(), peak());
    let started = std::time::Instant::now();
    let tree = ContextTree::open_file(&source, None).expect("open");
    let lazy = tree.resolver_ledger_is_lazy();
    let d0 = tree.select("d0", false).expect("select").len();
    eprintln!(
        "PROBE head-only open: {:.1} ms, lazy {lazy}, d0 {d0}, VmRSS {}, VmHWM {}",
        started.elapsed().as_secs_f64() * 1000.0,
        rss(),
        peak()
    );
    drop(tree);
    let started = std::time::Instant::now();
    let snapshot =
        Snapshot::open(source.as_ref(), Arc::new(IoCounterSet::default())).expect("snapshot");
    let full = snapshot.replay_ledger().expect("full");
    eprintln!(
        "PROBE full replay: {:.1} ms, traces {}, VmRSS {}",
        started.elapsed().as_secs_f64() * 1000.0,
        full.trace_count(),
        rss()
    );
}
