use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::json;

use super::{AppendState, branch_scalars, check_metadata_eligibility, classify_component};
use crate::append_writer::{
    PactAppendBatch, PactAppendFailure, PactAppendMessage, PactAppendOptions, PactAppendOutcome,
    PactAppendWriter,
};
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::index::HeadMetadata;
use crate::pact_file::snapshot::Snapshot;
use crate::tests::instrumentation;
use crate::trace::BoundaryKind;
use crate::{Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs};

fn fixture(dir: &std::path::Path, turns: usize) -> std::path::PathBuf {
    fixture_with_options(dir, turns, false, None)
}

fn fixture_with_options(
    dir: &std::path::Path,
    turns: usize,
    derived_depth: bool,
    ttl: Option<i64>,
) -> std::path::PathBuf {
    let path = dir.join("append-state.pact");
    let options = ContextTreeOptions {
        derived_depth,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new("as_".to_string(), 1_000_000, 1)
                .expect("deterministic"),
        ),
    };
    let mut tree = ContextTree::with_id_and_options("as_root", options).expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..turns {
        let role = if turn % 2 == 0 { "user" } else { "assistant" };
        let parts = (0..2)
            .map(|part| {
                let options = tree.next_authoring_component_options();
                let mut component =
                    Component::text_authoring(format!("turn {turn} part {part}"), options);
                if turn + 1 == turns {
                    component.lifecycle.policy.ttl = ttl;
                }
                component
            })
            .collect();
        tree.add_message(role, parts, PactAttrs::new())
            .expect("stage");
        tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    }
    path
}

fn bootstrap_counting_replayed_transactions(
    path: &std::path::Path,
) -> (crate::PactResult<AppendState>, usize) {
    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    instrumentation::reset_positional_replay_transactions();
    let result = AppendState::bootstrap(&snapshot, "main", &crate::PactMemoryOptions::default());
    (result, instrumentation::positional_replay_transactions())
}

fn policy_free_branches(path: &std::path::Path) -> BTreeSet<String> {
    Snapshot::open(path, Arc::new(IoCounterSet::default()))
        .expect("snapshot")
        .metadata()
        .expect("metadata")
        .lifecycle_policy_free_branches
}

fn append_three_rows(path: &std::path::Path, derived_depth: bool) {
    let mut writer = PactAppendWriter::open_with_options(
        path,
        "main",
        crate::PactMemoryOptions::default(),
        PactAppendOptions { derived_depth },
    )
    .expect("open writer");
    for round in 0..3 {
        let outcome = writer
            .append(PactAppendBatch {
                source: "lens".to_string(),
                batch_id: format!("b{round}"),
                expected: writer.version().clone(),
                previous_checkpoint: None,
                checkpoint: None,
                messages: vec![PactAppendMessage {
                    row_id: format!("r{round}"),
                    role: "user".to_string(),
                    parts: vec![
                        Component::from_component_value(&json!({
                            "componentType": "text_content",
                            "content": format!("row {round}"),
                        }))
                        .expect("part"),
                    ],
                    aux: PactAttrs::new(),
                }],
            })
            .expect("append");
        assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
    }
    writer.close();
}

#[test]
fn a_marked_policy_free_branch_bootstraps_without_reading_a_trace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture_with_options(temp.path(), 2, true, None);
    assert!(
        policy_free_branches(&path).contains("main"),
        "the eager writer proves the branch policy-free from the ledger"
    );
    append_three_rows(&path, true);
    assert!(
        policy_free_branches(&path).contains("main"),
        "the bounded writer carries the proof forward"
    );

    let (state, replayed) = bootstrap_counting_replayed_transactions(&path);
    let state = state.expect("bootstrap");
    assert_eq!(replayed, 0, "a marked policy-free branch scans no trace");
    assert!(state.derived_depth);
    let truth = materialized(&path);
    let root_children = state.root_child_ids().expect("root children");
    assert_eq!(
        root_children.len(),
        6,
        "system container, two turns, three rows"
    );
    for child_id in root_children {
        let row = state.node(&child_id).expect("node").expect("row");
        assert_eq!(
            row.depth.map(|depth| depth as i32),
            truth.depths[&child_id],
            "{child_id} ranks itself from the turn table"
        );
    }
}

#[test]
fn an_unmarked_branch_still_scans_its_traces() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture_with_options(temp.path(), 2, false, None);
    append_three_rows(&path, false);
    assert!(policy_free_branches(&path).contains("main"));

    let (state, replayed) = bootstrap_counting_replayed_transactions(&path);
    let state = state.expect("bootstrap");
    assert!(!state.derived_depth);
    assert_eq!(
        replayed, 3,
        "every bounded transaction after the baseline is replayed"
    );
}

#[test]
fn a_marked_branch_with_a_policy_is_not_proven_free_and_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture_with_options(temp.path(), 2, true, Some(2));
    assert!(!policy_free_branches(&path).contains("main"));

    let (state, _) = bootstrap_counting_replayed_transactions(&path);
    let Err(error) = state else {
        panic!("a ttl policy is unsupported by the bounded writer");
    };
    assert_eq!(error.kind(), crate::PactErrorKind::Unsupported);
    assert!(
        matches!(
            PactAppendWriter::open_with_options(
                &path,
                "main",
                crate::PactMemoryOptions::default(),
                PactAppendOptions {
                    derived_depth: true
                },
            ),
            Err(PactAppendFailure::UnsupportedSemantics(_))
        ),
        "the writer refuses the file the same way"
    );
}

#[test]
fn metadata_from_an_older_writer_keeps_scanning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture_with_options(temp.path(), 2, true, None);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let mut encoded: serde_json::Value = serde_json::from_slice(
        &snapshot
            .metadata()
            .expect("metadata")
            .encode()
            .expect("encode"),
    )
    .expect("json");
    encoded
        .as_object_mut()
        .expect("object")
        .remove("lifecycle_policy_free_branches");
    let decoded = HeadMetadata::decode(&serde_json::to_vec(&encoded).expect("bytes"))
        .expect("older metadata decodes");
    assert!(decoded.lifecycle_policy_free_branches.is_empty());
    assert!(decoded.derived_depth_branches.contains("main"));
}

struct Materialized {
    depths: BTreeMap<String, Option<i32>>,
    lifecycle_seen: BTreeMap<String, u64>,
}

fn materialized(path: &std::path::Path) -> Materialized {
    let tree = ContextTree::open_file(path, None).expect("open");
    let head = tree.head().expect("head");
    let mut depths = BTreeMap::new();
    fn walk(component: &Component, depths: &mut BTreeMap<String, Option<i32>>) {
        depths.insert(component.id.clone(), component.pact_depth);
        for child in &component.children {
            walk(child, depths);
        }
    }
    walk(&head.tree.root, &mut depths);
    Materialized {
        depths,
        lifecycle_seen: head.lifecycle_seen.clone(),
    }
}

#[test]
fn eligible_direct_branch_bootstraps_with_the_exact_topology() {
    for turns in [6_usize, 3] {
        bootstrap_matches_the_materialized_truth(turns);
    }
}

fn bootstrap_matches_the_materialized_truth(turns: usize) {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), turns);
    let counters = Arc::new(IoCounterSet::default());
    let snapshot = Snapshot::open(&path, Arc::clone(&counters)).expect("snapshot");
    let state = AppendState::bootstrap(&snapshot, "main", &crate::PactMemoryOptions::default())
        .expect("bootstrap");
    assert_eq!(state.branch_id, "main");

    assert_eq!(state.root_child_count as usize, turns + 1);
    assert_eq!(state.node_count as usize, (turns + 1) + 2 * turns);
    assert_eq!(
        state.next_offset as usize,
        turns + 1,
        "max nonnegative sibling offset + 1"
    );
    assert_eq!(state.head_revision as usize, turns);
    assert!(
        state.first_seen_count as usize >= 3 * turns,
        "every admitted id is a first-seen member"
    );

    let oracle = materialized(&path);
    let depths = &oracle.depths;
    let mut turn_range = state.turn_pages(4);
    let mut ordered = Vec::new();
    loop {
        let page = turn_range.next_page(state.pages()).expect("page");
        if page.is_empty() {
            break;
        }
        ordered.extend(
            page.into_iter()
                .map(|(_, id)| String::from_utf8(id).expect("id")),
        );
    }
    assert_eq!(ordered.len(), turns + 1);
    let header = state.node(&ordered[0]).expect("row").expect("header row");
    assert_eq!(
        (header.kind.as_str(), header.offset, header.depth),
        ("system_header", 0, Some(-1))
    );
    for (position, turn_id) in ordered.iter().enumerate().skip(1) {
        let row = state.node(turn_id).expect("row").expect("turn row");
        assert_eq!(row.parent, "as_root");
        assert_eq!(row.offset, position as i64);
        assert_eq!(row.kind, "message_turn");
        assert_eq!(row.depth, depths[turn_id].map(i64::from));
        assert_eq!(
            row.depth,
            Some((turns - position) as i64),
            "older turns were shifted deeper"
        );
        let mut nodes = state.turn_node_pages(turn_id, 8);
        let page = nodes.next_page(state.pages()).expect("turn nodes");
        assert_eq!(page.len(), 3, "the turn and its two parts");
        let ids = page
            .into_iter()
            .map(|(_, value)| String::from_utf8(value).expect("node id"))
            .collect::<Vec<_>>();
        assert_eq!(&ids[0], turn_id, "the turn leads its own pre-order");
        for node_id in &ids {
            let node = state.node(node_id).expect("row").expect("node row");
            let depth = state.current_depth(&node).expect("depth");
            assert_eq!(depth, depths[node_id].map(i64::from));
            assert!(state.is_known_id(node_id).expect("known"));
        }
        let (children, max_offset) = state.direct_children(turn_id, turn_id).expect("children");
        assert_eq!(children, ids[1..].to_vec());
        assert_eq!(max_offset, Some(1));
        assert_eq!(row.subtree_len, 3);
    }
    assert_eq!(
        (
            state.first_seen_missing,
            state.lifecycle_missing,
            state.lifecycle_stale
        ),
        (0, 0, 0),
        "a file written by the eager path has complete membership tables"
    );
    assert!(!state.is_known_id("never_seen").expect("unknown"));

    assert_eq!(state.lifecycle_tick, 0);
    assert_eq!(
        state.lifecycle_seen_count,
        oracle.lifecycle_seen.len() as u64
    );
    assert_eq!(state.lifecycle_seen_count, state.node_count + 1);
    for (id, tick) in &oracle.lifecycle_seen {
        assert_eq!(state.lifecycle_seen(id).expect("tick"), Some(*tick), "{id}");
    }
    assert_eq!(state.lifecycle_seen("never_seen").expect("tick"), None);
    let mut seen_pages = state.lifecycle_seen_pages(5);
    let mut seen_total = 0;
    loop {
        let page = seen_pages.next_page(state.pages()).expect("page");
        if page.is_empty() {
            break;
        }
        seen_total += page.len() as u64;
    }
    assert_eq!(seen_total, state.lifecycle_seen_count);
    assert!(state.scratch_bytes() > 0, "tables live on scratch");

    let io = counters.snapshot();
    assert_eq!(
        io.content_ranges_hydrated, 0,
        "bootstrap hydrates no content bodies"
    );
    assert_eq!(io.operations_replayed, 0, "bootstrap replays no trace ops");
}

#[test]
fn external_advancement_is_a_stale_pin_not_a_rebase() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let state = AppendState::bootstrap(&snapshot, "main", &crate::PactMemoryOptions::default())
        .expect("bootstrap");
    state.verify_pinned(&snapshot).expect("still current");

    let mut tree = ContextTree::open_file(&path, None).expect("open");
    let options = tree.next_authoring_component_options();
    tree.add_message(
        "user",
        vec![Component::text_authoring("external".to_string(), options)],
        PactAttrs::new(),
    )
    .expect("stage");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    drop(tree);

    let advanced = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("reopen");
    let error = state.verify_pinned(&advanced).expect_err("stale pin");
    assert!(error.to_string().contains("stale"), "{error}");
}

#[test]
fn wrong_branch_requests_and_composed_files_are_unsupported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let Err(error) =
        AppendState::bootstrap(&snapshot, "side", &crate::PactMemoryOptions::default())
    else {
        panic!("a branch mismatch must be refused");
    };
    assert_eq!(error.kind(), crate::PactErrorKind::Unsupported);

    let mut metadata: HeadMetadata = snapshot.metadata().expect("metadata");
    assert!(check_metadata_eligibility(&metadata, "main").is_ok());
    metadata.branch_count = 2;
    assert!(check_metadata_eligibility(&metadata, "main").is_err());
    metadata.branch_count = 1;
    metadata.composition_branches.insert("other".to_string());
    assert!(check_metadata_eligibility(&metadata, "main").is_err());
}

#[test]
fn branch_scalars_reject_forks_and_compositions_but_read_direct_branches() {
    let direct = json!({
        "head_revision": 7,
        "lifecycle_tick": 3,
        "base_ref": {"root_id": "r", "branch_id": "main", "revision": 0},
        "fork_ref": null,
        "parent_branch_id": null,
    });
    let (head, tick, base) = branch_scalars(&direct).expect("direct branch");
    assert_eq!((head, tick, base.revision), (7, 3, Some(0)));
    let forked = json!({"head_revision": 1, "base_ref": {"root_id": "r", "branch_id": "main"}, "fork_ref": {"root_id": "r", "branch_id": "main", "revision": 1}});
    assert_eq!(
        branch_scalars(&forked).expect_err("fork").kind(),
        crate::PactErrorKind::Unsupported
    );
    let composed = json!({"head_revision": 1, "base_ref": {"root_id": "r", "branch_id": "main"}, "parent_branch_id": "root_branch"});
    assert_eq!(
        branch_scalars(&composed).expect_err("composition").kind(),
        crate::PactErrorKind::Unsupported
    );
    assert!(
        branch_scalars(&json!({"lifecycle_tick": 1})).is_err(),
        "missing head_revision"
    );
}

#[test]
fn lifecycle_policies_and_hollow_turns_are_ineligible() {
    let plain = json!({"componentType": "text_content", "content": "x"});
    classify_component("t", &plain, false).expect("plain text is eligible");
    let ttl = json!({"componentType": "text_content", "content": "x", "ttl": 2});
    assert_eq!(
        classify_component("t", &ttl, false)
            .expect_err("ttl")
            .kind(),
        crate::PactErrorKind::Unsupported
    );
    let nested_cad = json!({"componentType": "text_content", "lifecycle": {"cad": 1}});
    assert!(classify_component("t", &nested_cad, false).is_err());
    let null_policy =
        json!({"componentType": "text_content", "ttl": null, "lifecycle": {"ttl": null}});
    classify_component("t", &null_policy, false).expect("null policies are absent policies");
    let hollow = json!({"componentType": "message_turn", "role": "user"});
    assert!(
        classify_component("m", &hollow, false).is_err(),
        "hollow turn"
    );
    classify_component("m", &hollow, true).expect("a turn with children is fine");
    let with_content = json!({"componentType": "message_turn", "content": "inline"});
    classify_component("m", &with_content, false).expect("content makes it non-hollow");
    let with_ref = json!({"componentType": "message_turn", "content_ref": {"id": "c"}});
    classify_component("m", &with_ref, false).expect("a content ref makes it non-hollow");
}

#[test]
fn a_file_without_a_persisted_identity_is_unsupported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let mut bytes = std::fs::read(&path).expect("read");
    let start = crate::pact_file::layout::FILE_IDENTITY_START;
    let end = start + crate::pact_file::layout::FILE_IDENTITY_BYTES;
    assert!(super::persisted_identity_present(&bytes));
    bytes[start..end].fill(0);
    assert!(!super::persisted_identity_present(&bytes));
    let stripped = temp.path().join("no-identity.pact");
    std::fs::write(&stripped, &bytes).expect("write");

    let snapshot = Snapshot::open(&stripped, Arc::new(IoCounterSet::default()))
        .expect("the reader derives a candidate identity and still opens");
    let Err(error) =
        AppendState::bootstrap(&snapshot, "main", &crate::PactMemoryOptions::default())
    else {
        panic!("a derived identity must not be used for appends");
    };
    assert_eq!(error.kind(), crate::PactErrorKind::Unsupported);
    assert!(error.to_string().contains("identity"), "{error}");
}

#[test]
fn later_ledger_deltas_update_lifecycle_and_branch_scalars() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let mut state = AppendState::bootstrap(&snapshot, "main", &crate::PactMemoryOptions::default())
        .expect("bootstrap");
    let before = state.lifecycle_seen_count;
    state
        .apply_ledger_delta(&json!({
            "lifecycle_tick": 4,
            "base_ref": {"root_id": "as_root", "branch_id": "main", "revision": 0, "content_ref": "c1"},
            "first_seen_additions": {"late": 9},
            "lifecycle_seen_additions": {"late": 5},
            "lifecycle_seen_removals": ["late", "never_tracked"],
        }))
        .expect("delta");
    assert_eq!(state.lifecycle_tick, 4);
    assert_eq!(state.base_ref.content_ref.as_deref(), Some("c1"));
    assert!(
        state.is_known_id("late").expect("known"),
        "first-seen membership survives removal"
    );
    assert_eq!(state.lifecycle_seen("late").expect("tick"), None);
    assert_eq!(state.lifecycle_seen_count, before);
    assert!(
        state
            .apply_ledger_delta(&json!({"lifecycle_seen_removals": [7]}))
            .is_err(),
        "malformed removals are rejected, not skipped"
    );
}

#[test]
fn admissions_shift_existing_turns_through_the_epoch_model() {
    use crate::resolver::message_authoring::{message_turn_component, text_component};
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let mut state = AppendState::bootstrap(&snapshot, "main", &crate::PactMemoryOptions::default())
        .expect("bootstrap");
    let old_turns = {
        let mut turns = state.turn_pages(8);
        turns
            .next_page(state.pages())
            .expect("page")
            .into_iter()
            .map(|(_, id)| String::from_utf8(id).expect("id"))
            .collect::<Vec<_>>()
    };
    assert_eq!(old_turns.len(), 3, "system header + two turns");
    let depth_now = |state: &AppendState, id: &str| {
        let row = state.node(id).expect("row").expect("present");
        state.current_depth(&row).expect("depth")
    };
    assert_eq!(depth_now(&state, &old_turns[0]), Some(-1));
    assert_eq!(depth_now(&state, &old_turns[1]), Some(1));
    assert_eq!(depth_now(&state, &old_turns[2]), Some(0));

    let new_turn = |id: &str, offset: i32| {
        let mut part = text_component(
            format!("{id}:p"),
            "x".to_string(),
            Some(id.to_string()),
            0,
            Some(0),
        );
        part.created_at_ns = 5;
        let mut turn = message_turn_component(
            id.to_string(),
            "as_root",
            "user",
            vec![part],
            Default::default(),
        )
        .expect("turn");
        turn.offset = offset;
        turn.pact_depth = Some(0);
        turn
    };
    assert_eq!(state.next_offset, 3);
    let first = new_turn("msg_new_1", 3);
    let admission = state.admit_root_turn(&first, 3).expect("admit");
    assert_eq!(admission.first_seen_added, vec!["msg_new_1", "msg_new_1:p"]);
    assert_eq!(admission.lifecycle_added, vec!["msg_new_1", "msg_new_1:p"]);
    assert_eq!(state.epoch, 1);
    assert_eq!(state.root_child_count, 4);
    assert_eq!(state.next_offset, 4);
    assert_eq!(
        depth_now(&state, &old_turns[0]),
        Some(-1),
        "the header never shifts"
    );
    assert_eq!(depth_now(&state, &old_turns[1]), Some(2));
    assert_eq!(depth_now(&state, &old_turns[2]), Some(1));
    assert_eq!(depth_now(&state, "msg_new_1"), Some(0));
    assert_eq!(depth_now(&state, "msg_new_1:p"), Some(0));
    let old_part = {
        let (children, _) = state
            .direct_children(&old_turns[1], &old_turns[1])
            .expect("children");
        children[0].clone()
    };
    assert_eq!(
        depth_now(&state, &old_part),
        Some(2),
        "descendants follow their turn"
    );
    assert_eq!(state.first_seen("msg_new_1").expect("rev"), Some(3));
    assert_eq!(state.lifecycle_seen("msg_new_1").expect("tick"), Some(1));

    let second = new_turn("msg_new_2", 4);
    state.admit_root_turn(&second, 3).expect("admit");
    assert_eq!(state.epoch, 2);
    assert_eq!(depth_now(&state, &old_turns[1]), Some(3));
    assert_eq!(depth_now(&state, "msg_new_1"), Some(1));
    assert_eq!(depth_now(&state, "msg_new_1:p"), Some(1));
    assert_eq!(depth_now(&state, "msg_new_2"), Some(0));
    let mut turns = state.turn_pages(8);
    let order = turns
        .next_page(state.pages())
        .expect("page")
        .into_iter()
        .map(|(_, id)| String::from_utf8(id).expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(
        order[3..],
        ["msg_new_1".to_string(), "msg_new_2".to_string()]
    );
    assert!(
        state.admit_root_turn(&second, 3).is_err(),
        "an id that is already present is refused"
    );

    let sys = text_component(
        "sys_1".to_string(),
        "rule".to_string(),
        Some("system".to_string()),
        0,
        Some(-1),
    );
    let admission = state.admit_child("system", &sys, 3).expect("admit child");
    assert_eq!(admission.first_seen_added, vec!["sys_1"]);
    let (children, max_offset) = state.direct_children("system", "system").expect("children");
    assert_eq!(children, vec!["sys_1".to_string()]);
    assert_eq!(max_offset, Some(0));
    assert_eq!(depth_now(&state, "sys_1"), Some(-1));
    state
        .admit_root_turn(&new_turn("msg_new_3", 5), 3)
        .expect("admit");
    assert_eq!(depth_now(&state, "sys_1"), Some(-1));
    let header = state.node("system").expect("row").expect("header");
    assert_eq!(header.subtree_len, 2);

    assert!(
        state
            .repair_membership(4)
            .expect("repair")
            .first_seen_added
            .is_empty()
    );
    assert!(state.stale_lifecycle_ids().expect("stale").is_empty());
}
