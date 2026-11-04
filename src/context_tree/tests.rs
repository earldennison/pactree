use super::*;

#[path = "placement_tests.rs"]
mod placement;

use crate::file_format::file_projection_json;
use crate::{
    file_format::{file_content_payload_json, file_create_from_ledger_json, file_stream_json},
    transaction_trace::TraceOpKind,
};
use serde_json::json;

fn text_component(id: &str, content: &str) -> Component {
    Component::from_component_value(&json!({
        "id": id,
        "componentType": "text_content",
        "content": content
    }))
    .expect("component")
}

fn ttl_text_component(id: &str, content: &str, ttl: i64) -> Component {
    Component::from_component_value(&json!({
        "id": id,
        "componentType": "text_content",
        "content": content,
        "ttl": ttl
    }))
    .expect("component")
}

fn project_current_view(tree: &ContextTree) -> PactView {
    match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("project current")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected view projection, got {other:?}"),
    }
}

fn view_has_text(view: &PactView, content: &str) -> bool {
    view.component_records()
        .values()
        .any(|node| node.node_type == "text_content" && node.content.as_deref() == Some(content))
}

fn lifecycle_tick(tree: &ContextTree) -> u64 {
    tree.resolver
        .ledger()
        .branches
        .get(tree.branch_id())
        .expect("branch")
        .lifecycle_tick
}

fn temp_pact_path(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}_{nanos}.pact"))
}

#[test]
fn save_file_rejects_existing_unrelated_pact_without_byte_change() {
    let path = temp_pact_path("save_file_identity_collision");
    let mut tree_a = ContextTree::with_id("identity_a").expect("tree A");
    tree_a.save_file(&path).expect("save tree A");
    tree_a
        .add_message(
            "user",
            vec![text_component("identity_a_text", "tree A")],
            PactAttrs::new(),
        )
        .expect("add A");
    tree_a
        .send(&ProviderThreadOptions::default())
        .expect("seal A");
    let before = std::fs::read(&path).expect("read A");

    let mut tree_b = ContextTree::with_id("identity_b").expect("tree B");
    let error = tree_b
        .save_file(&path)
        .expect_err("unrelated tree must not attach");

    assert_eq!(error.kind(), crate::PactErrorKind::Storage);
    assert!(error.to_string().starts_with("file identity mismatch:"));
    assert_eq!(std::fs::read(&path).expect("read unchanged A"), before);
    assert_eq!(
        ContextTree::open_file(&path, Some("main"))
            .expect("reopen A")
            .context_tree_id,
        "identity_a"
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn failed_save_does_not_bind_future_seals() {
    let path = temp_pact_path("failed_save_does_not_bind");
    let mut tree_a = ContextTree::with_id("bound_owner").expect("tree A");
    tree_a.save_file(&path).expect("save tree A");
    let before = std::fs::read(&path).expect("read A");

    let mut tree_b = ContextTree::with_id("rejected_writer").expect("tree B");
    tree_b
        .save_file(&path)
        .expect_err("unrelated tree must not bind");
    tree_b
        .add_message(
            "user",
            vec![text_component("rejected_text", "must stay in memory")],
            PactAttrs::new(),
        )
        .expect("add B");
    tree_b
        .send(&ProviderThreadOptions::default())
        .expect("seal B in memory");

    assert!(tree_b.resolver.file_path.is_none());
    assert_eq!(std::fs::read(&path).expect("read unchanged A"), before);
    std::fs::remove_file(path).ok();
}

#[test]
fn attach_rejects_same_root_divergent_head() {
    let path = temp_pact_path("attach_divergent_head");
    let mut owner = ContextTree::with_id("shared_root").expect("owner");
    owner.save_file(&path).expect("save owner");
    owner
        .insert(text_component("owner_text", "owner"), "root")
        .expect("owner insert");
    owner
        .send(&ProviderThreadOptions::default())
        .expect("owner seal");

    let mut divergent = ContextTree::with_id("shared_root").expect("divergent");
    divergent
        .insert(text_component("divergent_text", "divergent"), "root")
        .expect("divergent insert");
    divergent
        .send(&ProviderThreadOptions::default())
        .expect("divergent seal");

    let branch_id = divergent.branch_id().to_string();
    let error = divergent
        .resolver
        .attach_file(path.clone(), &branch_id)
        .expect_err("divergent head must be rejected");
    assert!(error.to_string().starts_with("file identity mismatch:"));
    assert!(divergent.resolver.file_path.is_none());
    std::fs::remove_file(path).ok();
}

#[test]
fn attach_accepts_identical_semantic_identity() {
    let path = temp_pact_path("attach_identical_identity");
    let mut owner = ContextTree::with_id("identical_root").expect("owner");
    owner.save_file(&path).expect("save owner");
    owner
        .insert(text_component("identical_text", "same"), "root")
        .expect("insert");
    owner.send(&ProviderThreadOptions::default()).expect("seal");

    let mut reopened = ContextTree::open_file(&path, Some("main")).expect("reopen");
    reopened.resolver.file_path = None;
    let branch_id = reopened.branch_id().to_string();
    reopened
        .resolver
        .attach_file(path.clone(), &branch_id)
        .expect("identical identity may attach");
    assert_eq!(reopened.resolver.file_path.as_deref(), Some(path.as_path()));
    std::fs::remove_file(path).ok();
}

#[test]
fn reopen_does_not_restore_deleted_lifecycle_entries() {
    let path = temp_pact_path("reopen_deleted_lifecycle");
    let mut tree = ContextTree::with_id("reopen_deleted_lifecycle").expect("tree");
    tree.save_file(&path).expect("save initial file");
    tree.add_message(
        "user",
        vec![text_component("before_reopen", "before reopen")],
        PactAttrs::new(),
    )
    .expect("add message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal message");

    let reopened = ContextTree::open_file(&path, Some("main")).expect("reopen file");
    assert!(
        !reopened
            .head()
            .expect("reopened head")
            .lifecycle_seen
            .contains_key("active"),
        "cold reopen must not restore lifecycle entries for deleted components"
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn attach_accepts_identity_after_sealed_tool_pair() {
    let path = temp_pact_path("attach_tool_pair_identity");
    let mut tree = ContextTree::with_id("tool_pair_identity").expect("tree");
    tree.save_file(&path).expect("save initial file");
    tree.add_message(
        "user",
        vec![text_component("historical_text", "historical turn")],
        PactAttrs::new(),
    )
    .expect("add historical turn");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal historical turn");

    let tool_call = Component::from_component_value(&json!({
        "id": "identity_tool_call",
        "componentType": "tool_call",
        "tool_call_id": "call_identity",
        "tool_name": "shell_execute",
        "parameters": {"command": "pwd"}
    }))
    .expect("tool call");
    let tool_result = Component::from_component_value(&json!({
        "id": "identity_tool_result",
        "componentType": "tool_result",
        "tool_call_id": "call_identity",
        "tool_name": "shell_execute",
        "success": true,
        "result": "/workspace"
    }))
    .expect("tool result");
    tree.receive(vec![tool_call, tool_result], PactAttrs::new())
        .expect("receive tool pair");
    tree.send(&ProviderThreadOptions {
        fail_on_orphan_tool_calls: false,
        ..ProviderThreadOptions::default()
    })
    .expect("seal tool pair");

    tree.save_file(&path)
        .expect("the bound file must retain the live ledger identity");
    std::fs::remove_file(path).ok();
}

#[test]
fn save_after_in_memory_seal_can_rebind_the_created_file() {
    let path = temp_pact_path("save_after_seal_identity");
    let mut tree = ContextTree::with_id("save_after_seal_identity").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("saved_text", "saved after seal")],
        PactAttrs::new(),
    )
    .expect("add message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal before save");

    tree.save_file(&path)
        .expect("create file from sealed ledger");
    tree.save_file(&path)
        .expect("created file must retain the live ledger identity");
    std::fs::remove_file(path).ok();
}

fn force_current_view_empty_for_trace_replay(tree: &mut ContextTree) {
    let mut ledger: crate::store::ContextTreeLedger =
        serde_json::from_str(&tree.resolver.ledger_json().expect("ledger json"))
            .expect("decode ledger");
    let branch = ledger.branches.get_mut(tree.branch_id()).expect("branch");
    branch.current.tree.root.children.clear();
    branch.current.tree.index = Default::default();
    branch.current.tree.first_seen = BTreeMap::new();
    branch.current.tree.tool_pairs = BTreeMap::new();
    branch.current.tree.stats = crate::store::PactStoreStats {
        node_count: 0,
        edge_count: 0,
        blob_count: 0,
        content_chars: 0,
    };
    tree.resolver = PactResolver::from_ledger(ledger);
}

fn assert_trace_has_ops(tree: &ContextTree, trace: &TraceRef) {
    let staged = tree.staged_traces().expect("staged traces");
    let staged_trace = staged
        .iter()
        .find(|staged| staged.trace_id == trace.trace_id)
        .expect("trace should be staged");
    assert!(
        !staged_trace.ops.is_empty(),
        "{} trace should carry replay ops",
        trace.op_kind
    );
}

fn first_produced_id(trace: &TraceRef) -> String {
    trace
        .produced_component_ids
        .first()
        .cloned()
        .expect("trace should expose a produced component id")
}

fn first_report_produced_id(report: &serde_json::Value) -> String {
    report["produced_component_ids"]
        .as_array()
        .and_then(|ids| ids.first())
        .and_then(serde_json::Value::as_str)
        .expect("report should expose a produced component id")
        .to_string()
}

fn fixed_text_component(local_id: &str, content: &str, created_at_ns: u64) -> Component {
    Component::text_authoring(
        content.to_string(),
        crate::components::AuthoringComponentOptions {
            local_id: Some(local_id.to_string()),
            created_at_ns: Some(created_at_ns),
            ..crate::components::AuthoringComponentOptions::default()
        },
    )
}

fn deterministic_options(prefix: &str) -> ContextTreeOptions {
    ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new(prefix, 1_000_000, 10).expect("options"),
        ),
    }
}

fn deterministic_text_component(tree: &mut ContextTree, content: &str) -> Component {
    Component::text_authoring(content.to_string(), tree.next_authoring_component_options())
}

fn write_deterministic_fixture(path: &Path) -> Vec<u8> {
    let _ = std::fs::remove_file(path);
    let mut tree =
        ContextTree::with_id_and_options("deterministic_ctx", deterministic_options("node_"))
            .expect("tree");
    tree.save_file(path).expect("save");
    let seed = deterministic_text_component(&mut tree, "seed");
    tree.insert(seed, "root").expect("insert seed");
    tree.send(&ProviderThreadOptions::default())
        .expect("send seed");
    let next = deterministic_text_component(&mut tree, "next");
    tree.insert(next, "root").expect("insert next");
    tree.send(&ProviderThreadOptions::default())
        .expect("send next");
    std::fs::read(path).expect("read fixture")
}

#[test]
fn deterministic_authoring_options_make_context_tree_bytes_reproducible() {
    let first = temp_pact_path("deterministic_authoring_first");
    let second = temp_pact_path("deterministic_authoring_second");

    let first_bytes = write_deterministic_fixture(&first);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second_bytes = write_deterministic_fixture(&second);

    assert_bytes_eq(
        &first_bytes,
        &second_bytes,
        "deterministic authoring should produce byte-identical fresh files",
    );
}

fn write_collision_fixture(path: &Path) -> (Vec<u8>, String) {
    let _ = std::fs::remove_file(path);
    let mut tree =
        ContextTree::with_id_and_options("deterministic_ctx", deterministic_options("dup_"))
            .expect("tree");
    tree.save_file(path).expect("save");

    let _ = tree.next_authoring_component_options();
    tree.insert(
        fixed_text_component("dup_00000002", "blocker", 1_001),
        "root",
    )
    .expect("insert blocker");
    tree.send(&ProviderThreadOptions::default())
        .expect("send blocker");

    let trace = tree
        .insert(
            fixed_text_component("dup_00000002", "collision", 1_002),
            "root",
        )
        .expect("insert collision");
    let reminted_id = first_produced_id(&trace);
    assert_eq!(reminted_id, "deterministic_ctx:dup_00000003");
    tree.send(&ProviderThreadOptions::default())
        .expect("send collision");

    (std::fs::read(path).expect("read fixture"), reminted_id)
}

#[test]
fn deterministic_authoring_collision_remint_uses_next_sequence_values() {
    let first = temp_pact_path("deterministic_collision_first");
    let second = temp_pact_path("deterministic_collision_second");

    let (first_bytes, first_id) = write_collision_fixture(&first);
    let (second_bytes, second_id) = write_collision_fixture(&second);

    assert_eq!(first_id, second_id);
    assert_bytes_eq(
        &first_bytes,
        &second_bytes,
        "deterministic collision remint should remain byte-identical",
    );
}

#[test]
fn deterministic_authoring_options_reject_zero_step() {
    let err = DeterministicAuthoringOptions::new("node_", 1_000, 0)
        .expect_err("zero step must be rejected");
    assert!(
        err.to_string().contains("timestamp step"),
        "unexpected error: {err}"
    );
}

fn assert_bytes_eq(left: &[u8], right: &[u8], message: &str) {
    if left != right {
        let first_diff = left
            .iter()
            .zip(right.iter())
            .position(|(left, right)| left != right);
        panic!(
            "{message}: lengths {} vs {}, first differing byte {:?}",
            left.len(),
            right.len(),
            first_diff
        );
    }
}

fn find_component_value<'a>(
    value: &'a serde_json::Value,
    id: &str,
) -> Option<&'a serde_json::Value> {
    if value.get("id").and_then(serde_json::Value::as_str) == Some(id) {
        return Some(value);
    }
    value
        .get("children")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find_map(|child| find_component_value(child, id))
}

fn find_component_value_by_content<'a>(
    value: &'a serde_json::Value,
    content: &str,
) -> Option<&'a serde_json::Value> {
    if value.get("content").and_then(serde_json::Value::as_str) == Some(content) {
        return Some(value);
    }
    value
        .get("children")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find_map(|child| find_component_value_by_content(child, content))
}

fn projection_components(value: &serde_json::Value) -> &serde_json::Map<String, serde_json::Value> {
    value["branch"]["components"]
        .as_object()
        .expect("projection components")
}

fn write_stream_fixture(path: &Path) {
    let mut tree = ContextTree::with_id("rh3_stream_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("rh3_user", "first stream message")],
        PactAttrs::new(),
    )
    .expect("first message");
    tree.send(&ProviderThreadOptions::default())
        .expect("send first message");
    tree.add_message(
        "assistant",
        vec![text_component("rh3_assistant", "second stream message")],
        PactAttrs::new(),
    )
    .expect("second message");
    tree.send(&ProviderThreadOptions::default())
        .expect("send second message");

    let mut ledger: crate::store::ContextTreeLedger =
        serde_json::from_str(&tree.resolver.ledger_json().expect("ledger json"))
            .expect("decode ledger");
    let trace = ledger
        .event_index
        .transaction_traces
        .values_mut()
        .next()
        .expect("trace");
    trace.aux.insert(
        "events".to_string(),
        json!([
            {
                "op_kind": "activity_event",
                "aux": {
                    "event_type": "notification",
                    "message": "stream parity event"
                }
            }
        ]),
    );
    let ledger_json = serde_json::to_string(&ledger).expect("ledger json");
    file_create_from_ledger_json(path, &ledger_json, 1024).expect("create stream fixture");
}

fn inject_birth_system_header(tree: &mut ContextTree) {
    let mut ledger: crate::store::ContextTreeLedger =
        serde_json::from_str(&tree.resolver.ledger_json().expect("ledger json"))
            .expect("decode ledger");
    let branch = ledger.branches.get_mut(tree.branch_id()).expect("branch");
    let mut root = branch.current.tree.root.clone();
    let mut system = Component::from_component_value(&json!({
        "id": "system",
        "componentType": "system_header",
        "role": "system",
        "children": [
            {
                "id": "system_title",
                "componentType": "text_content",
                "content": "XstaticWerkz session"
            },
            {
                "id": "system_policy",
                "componentType": "text_content",
                "content": "Keep shop context stable."
            }
        ]
    }))
    .expect("system header");
    if let Some(existing) = root.children.iter_mut().find(|child| child.id == "system") {
        existing.children = std::mem::take(&mut system.children);
    } else {
        root.children.push(system);
    }
    let projection = crate::store::build_projection_from_component(&root, &BTreeMap::new(), 1024)
        .expect("birth-system projection");
    branch.current = crate::store::PactView::from_projection(
        projection,
        branch.branch_id.clone(),
        branch.head_revision,
    );
    branch.base_ref.content_ref = None;
    tree.resolver = PactResolver::from_ledger(ledger);
}

fn write_replay_birth_skeleton_fixture(path: &Path) -> u64 {
    let _ = std::fs::remove_file(path);
    let mut tree = ContextTree::with_id("eng_rb_replay_birth_ctx").expect("tree");
    inject_birth_system_header(&mut tree);

    let system_insert = tree
        .insert(
            text_component("system_message", "system message before update"),
            "system",
        )
        .expect("insert under birth system");
    let system_message_id = first_produced_id(&system_insert);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal system insert");

    let root_insert = tree
        .insert(
            text_component("root_message", "root message to prune"),
            "root",
        )
        .expect("insert under root");
    let root_message_id = first_produced_id(&root_insert);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal root insert");

    tree.update(
        &system_message_id,
        ComponentChanges {
            content: Some("system message after update".to_string()),
            ..ComponentChanges::default()
        },
    )
    .expect("update system message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal system update");

    tree.prune(&root_message_id).expect("prune root message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal root prune");

    let head_revision = tree.head().expect("head").head_revision;
    let ledger_json = tree.resolver.ledger_json().expect("ledger json");
    file_create_from_ledger_json(path, &ledger_json, 1024).expect("create replay-birth fixture");
    head_revision
}

fn stream_without_source(mut stream: serde_json::Value) -> serde_json::Value {
    let object = stream.as_object_mut().expect("stream object");
    object.remove("source");
    object.remove("fallback_used");
    stream
}

#[test]
fn context_tree_constructs_without_serializing_as_ledger() {
    let tree = ContextTree::with_id("work_context").expect("tree");

    assert_eq!(tree.id(), "work_context");
    assert_eq!(tree.context_tree_id(), "work_context");
    assert_eq!(tree.branch_id(), "main");
    assert_eq!(tree.head().expect("head").branch_id, "main");
    assert_eq!(tree.resolver.ledger().root_id, "work_context");
    assert_eq!(tree.head().expect("head").tree.root_id, "work_context");
}

#[test]
fn context_tree_new_mints_distinct_ids() {
    let first = ContextTree::new().expect("first tree");
    let second = ContextTree::new().expect("second tree");

    assert_ne!(first.id(), second.id());
    assert_eq!(first.branch_id(), "main");
    assert_eq!(second.branch_id(), "main");
    assert_eq!(first.resolver.ledger().root_id, first.id());
    assert_eq!(second.resolver.ledger().root_id, second.id());
}

#[test]
fn authoring_insert_does_not_project_the_branch_per_call() {
    let mut ctx = ContextTree::with_id("authoring_projection_cost").expect("tree");
    for i in 0..3 {
        ctx.insert(
            Component::text_authoring(
                format!("t{i}"),
                crate::components::AuthoringComponentOptions::default(),
            ),
            "root",
        )
        .expect("insert");
    }
    ctx.send(&ProviderThreadOptions::default())
        .expect("seal setup inserts");
    force_current_view_empty_for_trace_replay(&mut ctx);

    crate::tests::instrumentation::reset_branch_trace_component_tree_replay_count();
    let before = crate::tests::instrumentation::TREE_EDIT_CALLS.with(|calls| calls.get());
    ctx.insert(
        Component::text_authoring(
            "probe".to_string(),
            crate::components::AuthoringComponentOptions::default(),
        ),
        "root",
    )
    .expect("insert");
    let edits = crate::tests::instrumentation::TREE_EDIT_CALLS.with(|calls| calls.get()) - before;
    let trace_replays = crate::tests::instrumentation::branch_trace_component_tree_replay_count();

    assert_eq!(
        edits, 3,
        "two trace projections plus the real mutation should cost exactly three tree edits"
    );
    assert_eq!(
        trace_replays, 2,
        "one cold id-collection projection plus the real resolve projection should replay traces exactly twice"
    );
}

#[test]
fn authoring_insert_remints_explicit_id_colliding_with_trace_replayed_component() {
    let mut ctx = ContextTree::new().expect("tree");
    let seed = ctx
        .insert(
            Component::text_authoring(
                "seed".to_string(),
                crate::components::AuthoringComponentOptions::default(),
            ),
            "root",
        )
        .expect("seed insert");
    let existing_id = first_produced_id(&seed);
    ctx.send(&ProviderThreadOptions::default())
        .expect("seal setup insert");
    force_current_view_empty_for_trace_replay(&mut ctx);

    let mut collision = Component::text_authoring(
        "collision".to_string(),
        crate::components::AuthoringComponentOptions::default(),
    );
    collision.id = existing_id.clone();
    let inserted = ctx
        .insert(collision, "root")
        .expect("colliding explicit id should be re-minted before resolve");
    let inserted_id = first_produced_id(&inserted);

    assert_ne!(
        inserted_id, existing_id,
        "explicit id colliding with replayed history must be re-minted"
    );
}

#[test]
fn mutation_stages_preview_without_advancing_head() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let initial_revision = tree.head().expect("head").head_revision;

    let trace = tree
        .insert(text_component("draft", "hello"), "root")
        .expect("insert");

    assert_eq!(trace.op_kind, "insert");
    assert!(tree.has_staged());
    let staged = tree.staged_traces().expect("staged traces");
    assert_eq!(staged.len(), 1);
    let draft_id = first_produced_id(&trace);
    assert!(draft_id.starts_with("test_context:"));
    assert_ne!(draft_id, "draft");
    assert!(staged[0].ops.iter().any(|op| {
        op.component_id == draft_id && matches!(op.kind, TraceOpKind::InsertComponent { .. })
    }));
    assert!(tree.head().expect("head").component("draft").is_none());
    assert_eq!(tree.head().expect("head").head_revision, initial_revision);
    assert!(
        tree.preview()
            .expect("preview")
            .component(&draft_id)
            .is_some()
    );
    assert!(tree.view().expect("view").component(&draft_id).is_some());
}

#[test]
fn staged_trace_discard_replays_remaining_work() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let first = tree
        .insert(text_component("first", "first"), "root")
        .expect("first");
    let first_id = first_produced_id(&first);
    let second = tree
        .insert(text_component("second", "second"), "root")
        .expect("second");
    let second_id = first_produced_id(&second);

    assert!(tree.discard_trace(&first.trace_id).expect("discard"));

    let view = tree.view().expect("view");
    assert!(view.component(&first_id).is_none());
    assert!(view.component(&second_id).is_some());
}

#[test]
fn staged_trace_discard_accepts_typed_targets() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let discard = tree
        .insert(text_component("discard_by_component", "discard me"), "root")
        .expect("discard target");
    let discard_id = first_produced_id(&discard);
    let keep = tree
        .insert(
            text_component("keep_after_component_discard", "keep me"),
            "root",
        )
        .expect("keep target");
    let keep_id = first_produced_id(&keep);

    let removed = tree
        .discard_traces(StagedTraceTarget::ComponentId(discard_id.clone()))
        .expect("discard by component");
    assert_eq!(removed, 1);

    let view = tree.view().expect("view");
    assert!(view.component(&discard_id).is_none());
    assert!(view.component(&keep_id).is_some());
}

#[test]
fn send_seals_staged_work_and_returns_provider_thread() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let system_prompt = tree
        .insert(text_component("system_prompt", "be concise"), "root")
        .expect("insert");
    let system_prompt_id = first_produced_id(&system_prompt);

    let _thread = tree
        .send(&ProviderThreadOptions::default())
        .expect("send provider thread");

    assert!(!tree.has_staged());
    assert!(
        tree.head()
            .expect("head")
            .component(&system_prompt_id)
            .is_some()
    );
}

#[test]
fn sealed_transaction_traces_project_without_branch_patches() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let durable = tree
        .insert(text_component("durable_trace", "from trace"), "root")
        .expect("insert");
    let durable_id = first_produced_id(&durable);
    tree.send(&ProviderThreadOptions::default()).expect("send");

    let mut ledger = tree.resolver.ledger().clone();
    let branch = ledger.branches.get("main").expect("branch").clone();
    assert!(!branch.transaction_traces.is_empty());
    ledger.branches.insert("main".to_string(), branch.clone());

    let projected =
        crate::store::project_branch_component_tree(&ledger, &branch, Some(branch.head_revision))
            .expect("trace replay projection");
    assert!(projected.contains(&durable_id));
}

#[test]
fn sealed_runtime_work_projects_from_traces_without_branch_patches() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let equiv = tree
        .insert(text_component("equiv_trace", "before"), "root")
        .expect("insert");
    let equiv_id = first_produced_id(&equiv);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal insert");
    tree.update(
        &equiv_id,
        ComponentChanges {
            content: Some("after".to_string()),
            ..ComponentChanges::default()
        },
    )
    .expect("update");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal update");

    let ledger = tree.resolver.ledger().clone();
    let branch = ledger.branches.get("main").expect("branch").clone();
    assert!(!branch.transaction_traces.is_empty());

    let trace_branch = branch.clone();
    let trace_tree = crate::store::project_branch_component_tree(
        &ledger,
        &trace_branch,
        Some(trace_branch.head_revision),
    )
    .expect("trace replay projection");
    let trace_replayed = trace_tree.project_value();

    let trace_component =
        find_component_value(&trace_replayed, &equiv_id).expect("trace component");
    assert_eq!(trace_component.get("content"), Some(&json!("after")));
    assert_eq!(
        trace_component.get("componentType"),
        Some(&json!("text_content"))
    );
    let component = trace_tree.component(&equiv_id).expect("component");
    assert_eq!(component.content(), Some("after"));
}

#[test]
fn file_round_trip_hydrates_replayable_transaction_traces() {
    let path = temp_pact_path("context_tree_transaction_trace_round_trip");
    let seed = ContextTree::with_id("test_context").expect("seed tree");
    let bytes = crate::file_format::PactFileBuilder::from_ledger(seed.resolver.ledger())
        .expect("file bytes");
    std::fs::write(&path, bytes).expect("write seed file");

    let mut tree = ContextTree::open_file(path.clone(), Some("main")).expect("open file");
    let file_trace = tree
        .insert(text_component("file_trace", "persist me"), "root")
        .expect("insert");
    let file_trace_id = first_produced_id(&file_trace);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal to file");

    let reopened = ContextTree::open_file(path.clone(), Some("main")).expect("reopen file");
    let mut ledger = reopened.resolver.ledger().clone();
    let branch = ledger.branches.get("main").expect("branch").clone();
    assert!(
        !branch.transaction_traces.is_empty(),
        "file load must hydrate replay traces from trace records"
    );
    ledger.branches.insert("main".to_string(), branch.clone());

    let projected =
        crate::store::project_branch_component_tree(&ledger, &branch, Some(branch.head_revision))
            .expect("trace replay after file round trip");
    assert!(projected.contains(&file_trace_id));

    let _ = std::fs::remove_file(path);
}

#[test]
fn fork_at_revision_replays_parent_transaction_trace_history() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let fork_base = tree
        .insert(text_component("fork_base", "base"), "root")
        .expect("base insert");
    let fork_base_id = first_produced_id(&fork_base);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal base");
    let after_fork_point = tree
        .insert(text_component("after_fork_point", "after"), "root")
        .expect("after insert");
    let after_fork_point_id = first_produced_id(&after_fork_point);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal after");

    let ledger = tree.resolver.ledger().clone();

    let (ledger, forked_id) = ledger
        .fork_at_revision(
            "main",
            Some(1),
            Some("trace_fork".to_string()),
            BTreeMap::new(),
            1024,
        )
        .expect("fork at revision");
    let forked = ledger.branches.get(&forked_id).expect("forked branch");
    assert!(forked.current.component(&fork_base_id).is_some());
    assert!(forked.current.component(&after_fork_point_id).is_none());
}

#[test]
fn hard_fork_before_later_tool_pair_does_not_replay_duplicate_components() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let source = temp_pact_path("historical_tool_pair_source");
    let child = temp_pact_path("historical_tool_pair_child");
    let mut tree = ContextTree::with_id("historical_tool_pair").expect("tree");
    tree.save_file(&source).expect("save source");
    tree.add_message(
        "user",
        vec![text_component("historical_user", "before tools")],
        PactAttrs::new(),
    )
    .expect("user message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal user");
    let fork_revision = tree.head().expect("head").head_revision;

    let tool_call = Component::from_component_value(&json!({
        "id": "historical_tool_call",
        "componentType": "tool_call",
        "tool_call_id": "call_1",
        "tool_name": "shell_execute",
        "parameters": {"command": "pwd"}
    }))
    .expect("tool call");
    let tool_result = Component::from_component_value(&json!({
        "id": "historical_tool_result",
        "componentType": "tool_result",
        "tool_call_id": "call_1",
        "tool_name": "shell_execute",
        "success": true,
        "result": "/workspace"
    }))
    .expect("tool result");
    tree.receive(vec![tool_call, tool_result], PactAttrs::new())
        .expect("receive tool pair");
    tree.send(&ProviderThreadOptions {
        fail_on_orphan_tool_calls: false,
        ..ProviderThreadOptions::default()
    })
    .expect("seal tool pair");

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(fork_revision),
        None,
        1024,
    )
    .expect("hard fork before tool pair");
    let forked = ContextTree::open_file(&child, Some("main")).expect("open child");
    let xml = match forked
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("child XML")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected XML, got {other:?}"),
    };
    assert!(xml.contains("before tools"));
    assert!(!xml.contains("call_1"));

    std::fs::remove_file(source).ok();
    std::fs::remove_file(child).ok();
}

#[test]
fn sealed_transaction_traces_archive_deleted_nodes_without_branch_patches() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let archived_trace = tree
        .insert(text_component("archived_trace", "archive me"), "root")
        .expect("insert");
    let archived_trace_id = first_produced_id(&archived_trace);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal insert");
    tree.prune(&archived_trace_id).expect("delete");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal delete");

    let mut ledger = tree.resolver.ledger().clone();
    let branch = ledger.branches.get("main").expect("branch").clone();
    assert_eq!(branch.head_revision, 2);
    assert!(!branch.transaction_traces.is_empty());
    ledger.branches.insert("main".to_string(), branch.clone());

    let (archive, first_seen) = crate::store::project_branch_archive_tree_value(
        &ledger,
        &branch,
        Some(branch.head_revision),
    )
    .expect("trace archive projection");
    let archived = find_component_value(&archive, &archived_trace_id).expect("archived node");
    assert_eq!(first_seen.get(&archived_trace_id), Some(&1));
    assert_eq!(
        archived
            .get("visible_in_head")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        archived
            .get("deleted_at")
            .and_then(serde_json::Value::as_u64),
        Some(2)
    );
}

#[test]
fn project_routes_view_json_xml_and_provider_thread() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    tree.insert(text_component("draft", "hello"), "root")
        .expect("insert");

    assert!(matches!(
        tree.project(ProjectionRequest::View {
            mode: ViewMode::Current
        })
        .expect("view"),
        ProjectionOutput::View(_)
    ));
    assert!(matches!(
        tree.project(ProjectionRequest::Json {
            mode: ViewMode::Current
        })
        .expect("json"),
        ProjectionOutput::Json(_)
    ));
    assert!(matches!(
        tree.project(ProjectionRequest::Xml {
            mode: ViewMode::Current
        })
        .expect("xml"),
        ProjectionOutput::Xml(_)
    ));
    assert!(matches!(
        tree.project(ProjectionRequest::ProviderThread {
            options: ProviderThreadOptions::default()
        })
        .expect("provider thread"),
        ProjectionOutput::ProviderThread(_)
    ));
}

#[test]
fn project_view_preserves_first_seen_for_current_and_head() {
    let mut tree = ContextTree::with_id("first_seen_ctx").expect("tree");
    let first = tree
        .insert(text_component("first_seen_a", "first"), "root")
        .expect("insert first");
    let first_id = first_produced_id(&first);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal first");

    let second = tree
        .insert(text_component("first_seen_b", "second"), "root")
        .expect("insert second");
    let second_id = first_produced_id(&second);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal second");

    let head = tree.head().expect("head").clone();
    assert_eq!(head.tree.first_seen.get(&first_id), Some(&1));
    assert_eq!(head.tree.first_seen.get(&second_id), Some(&2));

    let current_projected = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("current view")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected current view projection, got {other:?}"),
    };
    let head_projected = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Head,
        })
        .expect("head view")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected head view projection, got {other:?}"),
    };

    assert_eq!(current_projected.tree.first_seen, head.tree.first_seen);
    assert_eq!(head_projected.tree.first_seen, head.tree.first_seen);
}

#[test]
fn project_view_marks_staged_first_seen_at_next_revision() {
    let mut tree = ContextTree::with_id("first_seen_staged_ctx").expect("tree");
    let committed = tree
        .insert(text_component("first_seen_committed", "committed"), "root")
        .expect("insert committed");
    let committed_id = first_produced_id(&committed);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal committed");
    let committed_head = tree.head().expect("head").head_revision;

    let staged = tree
        .insert(text_component("first_seen_staged", "staged"), "root")
        .expect("insert staged");
    let staged_id = first_produced_id(&staged);
    let projected = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("current view")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected current view projection, got {other:?}"),
    };

    assert_eq!(projected.head_revision, committed_head + 1);
    assert_eq!(
        projected.tree.first_seen.get(&committed_id),
        Some(&committed_head),
        "project(View Current) must preserve committed first_seen revisions"
    );
    assert_eq!(
        projected.tree.first_seen.get(&staged_id),
        Some(&(committed_head + 1)),
        "project(View Current) must mark staged components as first seen at the next revision"
    );
}

#[test]
fn project_archive_view_preserves_original_first_seen_revision() {
    let mut tree = ContextTree::with_id("first_seen_archive_ctx").expect("tree");
    let keep = tree
        .insert(text_component("archive_keep", "keep"), "root")
        .expect("insert keep");
    let keep_id = first_produced_id(&keep);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal keep");

    let deleted = tree
        .insert(text_component("archive_deleted", "deleted"), "root")
        .expect("insert deleted");
    let deleted_id = first_produced_id(&deleted);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal deleted");
    tree.prune(&deleted_id).expect("prune deleted");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal prune");

    let archive_projected = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Archive,
        })
        .expect("archive view")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected archive view projection, got {other:?}"),
    };

    assert_eq!(archive_projected.tree.first_seen.get(&keep_id), Some(&1));
    assert_eq!(archive_projected.tree.first_seen.get(&deleted_id), Some(&2));
}

#[test]
fn mutation_surface_covers_update_replace_upsert_prune_and_select() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let insert = tree
        .insert(text_component("item", "draft"), "root")
        .expect("insert");
    assert_trace_has_ops(&tree, &insert);
    let item_id = first_produced_id(&insert);

    let update = tree
        .update(
            &item_id,
            ComponentChanges {
                content: Some("updated".to_string()),
                ..ComponentChanges::default()
            },
        )
        .expect("update");
    assert_eq!(update.op_kind, "update");
    assert_trace_has_ops(&tree, &update);
    assert_eq!(
        tree.view()
            .expect("view")
            .component(&item_id)
            .and_then(Component::content),
        Some("updated")
    );

    let replace = tree
        .replace(text_component("item", "replaced"), &item_id)
        .expect("replace");
    assert_eq!(replace.op_kind, "replace");
    assert_trace_has_ops(&tree, &replace);
    assert_eq!(
        tree.view()
            .expect("view")
            .component(&item_id)
            .and_then(Component::content),
        Some("replaced")
    );

    let upsert_insert = tree
        .upsert(text_component("upserted", "inserted"), None, "root")
        .expect("upsert insert");
    assert_eq!(upsert_insert.op_kind, "upsert");
    assert_trace_has_ops(&tree, &upsert_insert);
    let upserted_id = first_produced_id(&upsert_insert);
    assert!(tree.view().expect("view").component(&upserted_id).is_some());

    let append = tree
        .append(text_component("appended", "appended"), "root")
        .expect("append");
    assert_eq!(append.op_kind, "append");
    assert_trace_has_ops(&tree, &append);

    let selected = tree
        .select(&format!("#{upserted_id}"), false)
        .expect("select");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, upserted_id);

    let prune = tree.prune(&item_id).expect("prune");
    assert_eq!(prune.op_kind, "delete");
    assert_trace_has_ops(&tree, &prune);
    assert!(tree.view().expect("view").component(&item_id).is_none());
}

#[test]
fn seal_staged_persists_runtime_mutations_to_file() {
    let path = temp_pact_path("context_tree_seal_staged_runtime");
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let insert = tree
        .insert(text_component("item", "draft"), "root")
        .expect("insert");
    let item_id = first_produced_id(&insert);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal initial insert");
    tree.save_file(&path).expect("save initial file");

    tree.replace(text_component("item", "compacted"), &item_id)
        .expect("replace");
    assert!(tree.has_staged());
    let commit = tree
        .seal_staged(BoundaryKind::Adhoc)
        .expect("seal staged")
        .expect("commit ref");
    assert_eq!(commit.boundary, BoundaryKind::Adhoc);
    assert!(!tree.has_staged());

    let reopened = ContextTree::open_file(path.clone(), Some("main")).expect("reopen file");
    let view = reopened.view().expect("view");
    let component = view
        .component(&item_id)
        .expect("component should survive reopen");
    assert_eq!(component.content(), Some("compacted"));
}

#[test]
fn prune_accepts_selector_targets() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let insert = tree
        .insert(
            Component::from_component_value(&json!({
                "id": "victim",
                "componentType": "text_content",
                "content": "bye",
                "tags": ["evictable"]
            }))
            .expect("component"),
            "root",
        )
        .expect("insert");
    let victim_id = first_produced_id(&insert);
    let keep = tree
        .insert(text_component("survivor", "hi"), "root")
        .expect("insert");
    let survivor_id = first_produced_id(&keep);

    let prune = tree.prune("+evictable").expect("prune by tag selector");
    assert_eq!(prune.op_kind, "delete");
    let view = tree.view().expect("view");
    assert!(view.component(&victim_id).is_none());
    assert!(view.component(&survivor_id).is_some());

    tree.prune(".text_content").expect("prune by type selector");
    assert!(tree.view().expect("view").component(&survivor_id).is_none());

    let insert = tree
        .insert(text_component("by_id", "id"), "root")
        .expect("insert");
    let by_id = first_produced_id(&insert);
    tree.prune(&by_id).expect("prune by id");
    assert!(tree.view().expect("view").component(&by_id).is_none());

    assert!(tree.prune("root").is_err());
}

#[test]
fn discard_staged_drops_all_staged_work() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    tree.insert(text_component("draft", "hello"), "root")
        .expect("insert");

    assert!(tree.has_staged());
    assert!(tree.discard_staged().expect("discard all"));
    assert!(!tree.has_staged());
    assert!(tree.view().expect("view").component("draft").is_none());
    assert!(tree.staged_traces().expect("traces").is_empty());
}

#[test]
fn read_only_views_do_not_advance_head_revision() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    tree.insert(text_component("draft", "hello"), "root")
        .expect("insert");
    let before = tree.head().expect("head").head_revision;

    let _ = tree.head().expect("head");
    let _ = tree.view().expect("view");
    let _ = tree.preview().expect("preview");
    let _ = tree
        .project(ProjectionRequest::Json {
            mode: ViewMode::Current,
        })
        .expect("json");
    let _ = tree
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("xml");
    let _ = tree
        .project(ProjectionRequest::ProviderThread {
            options: ProviderThreadOptions::default(),
        })
        .expect("provider thread");
    let _ = tree.select("#draft", false).expect("select");

    assert_eq!(tree.head().expect("head").head_revision, before);
    assert_eq!(lifecycle_tick(&tree), 0);
    assert!(tree.has_staged());
}

#[test]
fn receive_seals_post_response_without_manual_resolver_call() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    let report = tree
        .receive(
            vec![text_component("assistant_text", "done")],
            PactAttrs::new(),
        )
        .expect("receive");
    let assistant_text_id = first_report_produced_id(&report);

    assert_eq!(report["schema"], "pact.receive.v1");
    assert!(!tree.has_staged());
    assert!(
        tree.head()
            .expect("head")
            .component(&assistant_text_id)
            .is_some()
    );
}

#[test]
fn send_receive_pair_advances_lifecycle_once_per_boundary() {
    let mut tree = ContextTree::with_id("test_context").expect("tree");
    tree.insert(text_component("request", "hello"), "root")
        .expect("insert request");

    assert_eq!(lifecycle_tick(&tree), 0);
    tree.send(&ProviderThreadOptions::default()).expect("send");
    assert_eq!(lifecycle_tick(&tree), 1);

    tree.receive(vec![text_component("response", "done")], PactAttrs::new())
        .expect("receive");
    assert_eq!(lifecycle_tick(&tree), 2);
    assert!(!tree.has_staged());
}

#[test]
fn move_component_relocates_same_identity() {
    let mut tree = ContextTree::with_id("move_ctx").expect("tree");
    let a = tree
        .insert(text_component("box_a", "container a"), "root")
        .expect("insert a");
    let b = tree
        .insert(text_component("box_b", "container b"), "root")
        .expect("insert b");
    let a_id = first_produced_id(&a);
    let b_id = first_produced_id(&b);
    let item = tree
        .insert(text_component("item", "payload"), a_id.as_str())
        .expect("insert item");
    let item_id = first_produced_id(&item);

    let trace = tree
        .move_component(
            format!("#{item_id}").as_str(),
            format!("#{b_id}").as_str(),
            None,
            None,
        )
        .expect("move");
    assert_eq!(trace.op_kind, "move");

    let view = tree.view().expect("view");
    let moved = view.component(&item_id).expect("moved survives");
    assert_eq!(moved.id, item_id, "same identity across the move");
    assert!(
        view.child_ids(&b_id).contains(&item_id),
        "item now lives under box_b"
    );
    assert!(!view.child_ids(&a_id).contains(&item_id), "item left box_a");
}

#[test]
fn move_replace_kills_occupant_in_same_op() {
    let mut tree = ContextTree::with_id("move_replace_ctx").expect("tree");
    let holder = tree
        .insert(text_component("holder", "holder"), "root")
        .expect("insert holder");
    let holder_id = first_produced_id(&holder);
    let occupant = tree
        .insert(text_component("occupant", "old"), holder_id.as_str())
        .expect("insert occupant");
    let occupant_id = first_produced_id(&occupant);
    let mover = tree
        .insert(text_component("mover", "new"), "root")
        .expect("insert mover");
    let mover_id = first_produced_id(&mover);

    let trace = tree
        .move_component(
            format!("#{mover_id}").as_str(),
            format!("#{occupant_id}").as_str(),
            Some("replace"),
            None,
        )
        .expect("move replace");
    assert_eq!(trace.op_kind, "move");

    let view = tree.view().expect("view");
    assert!(view.component(&mover_id).is_some(), "mover survives");
    assert!(
        view.component(&occupant_id).is_none(),
        "occupant died in the same op"
    );
}

#[test]
fn move_rejects_message_containers() {
    let mut tree = ContextTree::with_id("move_guard_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("msg", "hello")],
        PactAttrs::new(),
    )
    .expect("message");

    let view = tree.view().expect("view");
    let container_id = view
        .component_values()
        .iter()
        .find(|c| c.component_type() == "message_turn")
        .map(|c| c.id.clone())
        .expect("message container exists");

    let err = tree
        .move_component(format!("#{container_id}").as_str(), "root", None, None)
        .expect_err("containers are immovable");
    assert!(err.to_string().contains("message container"));
}

#[test]
fn large_content_survives_save_reopen_and_renders_everywhere() {
    let big = "z".repeat(2000);
    let mut tree = ContextTree::with_id("durability_ctx").expect("tree");
    tree.add_message("user", vec![text_component("u", "hi")], PactAttrs::new())
        .expect("user");
    tree.send(&ProviderThreadOptions::default()).expect("send");
    tree.receive(vec![text_component("a", &big)], PactAttrs::new())
        .expect("receive");

    let path = temp_pact_path("durability");
    tree.save_file(&path).expect("save");

    let reopened = ContextTree::open_file(&path, None).expect("reopen");

    let xml = match reopened
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("xml")
    {
        ProjectionOutput::Xml(s) => s,
        other => panic!("expected xml, got {other:?}"),
    };
    assert!(xml.contains(&big), "xml projection must hydrate content");

    let thread = reopened
        .project(ProjectionRequest::ProviderThread {
            options: ProviderThreadOptions::default(),
        })
        .expect("thread");
    let thread_json = serde_json::to_string(&thread).expect("thread json");
    assert!(
        thread_json.contains(&big),
        "provider thread must hydrate content"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn component_churn_stores_diffs_not_snapshots() {
    let render = |cycle: usize| -> String {
        (0..800)
            .map(|i| {
                if i == cycle % 800 {
                    format!("scaffold line {i} EDITED on cycle {cycle}")
                } else {
                    format!("scaffold line {i} with recurring filler text to mimic a real render")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let render_bytes = render(0).len();

    let path = temp_pact_path("churn");
    let mut tree = ContextTree::with_id("churn_ctx").expect("tree");
    tree.bind_file(&path);

    tree.add_message("user", vec![text_component("u0", "hi")], PactAttrs::new())
        .expect("u0");
    tree.send(&ProviderThreadOptions::default()).expect("send0");
    tree.receive(vec![text_component("canvas", &render(0))], PactAttrs::new())
        .expect("recv0");
    let canvas_id = tree
        .select(".text_content", false)
        .expect("select")
        .into_iter()
        .find(|c| c.content.as_deref().map(str::len).unwrap_or(0) > 1000 || c.content_ref.is_some())
        .map(|c| c.id)
        .expect("canvas id");

    let cycles = 20usize;
    for cycle in 1..=cycles {
        tree.replace(text_component("canvas", &render(cycle)), &canvas_id)
            .expect("canvas replace");
        tree.add_message(
            "user",
            vec![text_component(&format!("u{cycle}"), "next")],
            PactAttrs::new(),
        )
        .expect("user");
        tree.send(&ProviderThreadOptions::default()).expect("send");
        tree.receive(
            vec![text_component(&format!("a{cycle}"), "ok")],
            PactAttrs::new(),
        )
        .expect("receive");
    }

    let file_bytes = std::fs::metadata(&path).expect("file").len() as usize;
    println!("churn file: {file_bytes} bytes for {cycles} cycles of ~{render_bytes}-byte renders");

    assert!(
        file_bytes < render_bytes * 8,
        "file is {file_bytes} bytes — over {}x a single {render_bytes}-byte render; \
         seal payloads are being stored as snapshots, not diffs",
        file_bytes / render_bytes.max(1),
    );

    let reopened = ContextTree::open_file(&path, None).expect("reopen");
    let xml = match reopened
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("xml")
    {
        ProjectionOutput::Xml(s) => s,
        other => panic!("expected xml, got {other:?}"),
    };
    let latest = format!("EDITED on cycle {cycles}");
    let stale = format!("EDITED on cycle {}", cycles - 1);
    assert!(
        xml.contains(&latest),
        "cold reopen must hydrate the latest replaced render"
    );
    assert!(
        !xml.contains(&stale),
        "cold reopen must not resurrect a stale render"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn overlay_child_renders_base_plus_local() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let base_path = temp_pact_path("overlay_base");
    let mut base = ContextTree::new().expect("base");
    base.add_message(
        "user",
        vec![text_component("u_marker", "ZEBRA_BASE_MARKER")],
        PactAttrs::new(),
    )
    .expect("add base message");
    base.send(&ProviderThreadOptions::default())
        .expect("seal base message into a revision");
    base.save_file(&base_path).expect("save base");

    let overlay_path = temp_pact_path("overlay_child");
    file_fork_json(
        &base_path,
        Some(overlay_path.as_path()),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        1024,
    )
    .expect("overlay fork");

    let overlay = ContextTree::open_file(&overlay_path, Some("main")).expect("open overlay");
    assert!(
        overlay.resolver.ledger().base.is_some(),
        "overlay child must carry base provenance after reopen (else compose is skipped)"
    );
    let xml = match overlay
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("project overlay")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected xml projection, got {other:?}"),
    };
    assert!(
        xml.contains("ZEBRA_BASE_MARKER"),
        "overlay must compose the base content on read; got: {xml}"
    );

    std::fs::remove_file(&base_path).ok();
    std::fs::remove_file(&overlay_path).ok();
}

#[test]
fn fork_reference_composes_parent_plus_local_without_polluting_parent() {
    let mut worker = ContextTree::with_id("worker_ctx").expect("worker");
    worker
        .add_message(
            "user",
            vec![text_component("u_marker", "ZEBRA_WORKER_MARKER")],
            PactAttrs::new(),
        )
        .expect("add worker message");
    worker
        .send(&ProviderThreadOptions::default())
        .expect("seal worker message into a revision");

    let mut pager = worker.fork_reference().expect("fork reference");
    assert_ne!(
        pager.branch_id(),
        worker.branch_id(),
        "reference fork must be a distinct branch"
    );
    assert_eq!(
        pager.resolver.ledger().root_id,
        worker.resolver.ledger().root_id,
        "reference fork shares the worker root (true reference, no minted root)"
    );

    let read_xml = match pager
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("project pager")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected xml projection, got {other:?}"),
    };
    assert!(
        read_xml.contains("ZEBRA_WORKER_MARKER"),
        "pager READ must compose the worker content via fork_ref; got: {read_xml}"
    );

    pager
        .add_message(
            "assistant",
            vec![text_component("p_note", "PAGER_LOCAL_NOTE")],
            PactAttrs::new(),
        )
        .expect("add pager message");
    let thread = pager
        .send(&ProviderThreadOptions::default())
        .expect("pager send");
    let thread_str = serde_json::to_string(&thread).expect("thread json");
    assert!(
        thread_str.contains("ZEBRA_WORKER_MARKER"),
        "pager SEND (egress) must compose the worker content; got: {thread_str}"
    );
    assert!(
        thread_str.contains("PAGER_LOCAL_NOTE"),
        "pager SEND must include the pager's own write; got: {thread_str}"
    );

    let worker_xml = match worker
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("project worker")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected xml projection, got {other:?}"),
    };
    assert!(
        worker_xml.contains("ZEBRA_WORKER_MARKER"),
        "worker keeps its own content"
    );
    assert!(
        !worker_xml.contains("PAGER_LOCAL_NOTE"),
        "worker must NOT see the pager's write; got: {worker_xml}"
    );
}

#[test]
fn auxiliary_reference_fork_is_point_in_time_and_fresh_per_execution() {
    let mut worker = ContextTree::with_id("aux_worker_ctx").expect("worker");
    worker
        .add_message(
            "user",
            vec![text_component("aux_base", "AUX_BASE_MARKER")],
            PactAttrs::new(),
        )
        .expect("add worker base");
    worker
        .send(&ProviderThreadOptions::default())
        .expect("seal worker base");

    let mut first_aux = worker.fork_reference().expect("first aux overlay");
    first_aux
        .add_message(
            "assistant",
            vec![text_component("aux_local", "AUX_LOCAL_MARKER")],
            PactAttrs::new(),
        )
        .expect("add local aux message");
    let first_thread = first_aux
        .send(&ProviderThreadOptions::default())
        .expect("first aux send");
    let first_thread = serde_json::to_string(&first_thread).expect("first aux thread");
    assert!(first_thread.contains("AUX_BASE_MARKER"));
    assert!(first_thread.contains("AUX_LOCAL_MARKER"));

    worker
        .add_message(
            "user",
            vec![text_component("aux_later", "AUX_LATER_PARENT_MARKER")],
            PactAttrs::new(),
        )
        .expect("add later worker message");
    worker
        .send(&ProviderThreadOptions::default())
        .expect("seal later worker message");

    let first_aux_xml = match first_aux
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("project first aux")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected xml projection, got {other:?}"),
    };
    assert!(
        !first_aux_xml.contains("AUX_LATER_PARENT_MARKER"),
        "an existing aux overlay must stay pinned to its fork-time parent revision"
    );

    let second_aux = worker.fork_reference().expect("second aux overlay");
    let second_aux_xml = match second_aux
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("project second aux")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected xml projection, got {other:?}"),
    };
    assert!(
        second_aux_xml.contains("AUX_LATER_PARENT_MARKER"),
        "a fresh aux overlay must see the worker's latest head at execution time"
    );
    assert!(
        !second_aux_xml.contains("AUX_LOCAL_MARKER"),
        "local writes from an earlier aux overlay must not leak into the parent or next overlay"
    );

    let worker_xml = match worker
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("project worker")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected xml projection, got {other:?}"),
    };
    assert!(worker_xml.contains("AUX_BASE_MARKER"));
    assert!(worker_xml.contains("AUX_LATER_PARENT_MARKER"));
    assert!(
        !worker_xml.contains("AUX_LOCAL_MARKER"),
        "aux writes must remain isolated from the parent worker"
    );
}

#[test]
fn reference_fork_construction_shares_one_immutable_ledger_allocation() {
    let mut worker = ContextTree::with_id("shared_snapshot_worker").expect("worker");
    worker
        .add_message(
            "user",
            vec![text_component("shared", "SHARED_SNAPSHOT_MARKER")],
            PactAttrs::new(),
        )
        .expect("message");
    worker
        .send(&ProviderThreadOptions::default())
        .expect("seal");

    let mut auxiliary = worker.fork_reference().expect("reference fork");
    assert!(worker.shares_ledger_allocation_with(&auxiliary));
    assert!(!auxiliary.reference_fork_is_active());
    auxiliary
        .add_message(
            "assistant",
            vec![text_component("local", "LOCAL_DELTA_MARKER")],
            PactAttrs::new(),
        )
        .expect("local write");
    assert!(auxiliary.reference_fork_is_active());
    assert!(!worker.shares_ledger_allocation_with(&auxiliary));
}

#[test]
fn context_tree_project_view_builds_staged_projection_once() {
    let mut tree = ContextTree::with_id("pc_view_ctx").expect("tree");
    tree.insert(text_component("pc_view", "PC_VIEW_STAGED_MARKER"), "root")
        .expect("stage insert");

    crate::tests::instrumentation::reset_projection_build_count();
    let view = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("project view")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected view projection, got {other:?}"),
    };
    assert!(
        view.component_records().values().any(|node| {
            node.node_type == "text_content"
                && node.content.as_deref() == Some("PC_VIEW_STAGED_MARKER")
        }),
        "project(View) must include staged content"
    );
    assert_eq!(
        crate::tests::instrumentation::projection_build_count(),
        1,
        "project(View) should stream once into PactViewSink without a separate metadata projection"
    );
}

#[test]
fn pc2_current_reads_reuse_one_composed_tree_until_mutation() {
    let mut tree = ContextTree::with_id("pc2_reuse_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("pc2_reuse", "PC2_REUSE_MARKER")],
        PactAttrs::new(),
    )
    .expect("add setup message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal setup message");
    force_current_view_empty_for_trace_replay(&mut tree);

    crate::tests::instrumentation::reset_branch_trace_component_tree_replay_count();
    let view = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("project current")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected view projection, got {other:?}"),
    };
    assert!(
        view.component_records().values().any(|node| {
            node.node_type == "text_content" && node.content.as_deref() == Some("PC2_REUSE_MARKER")
        }),
        "initial project must include replayed marker"
    );

    let stream = tree
        .stream("current_surface", None, None, "after", 100, None)
        .expect("stream current");
    assert_eq!(stream["schema"], "pact.stream.v1");
    assert_eq!(
        crate::tests::instrumentation::branch_trace_component_tree_replay_count(),
        1,
        "an unchanged handle should compose the current tree once across project+stream"
    );
}

#[test]
fn pc2_current_cache_invalidates_after_staged_write() {
    let mut tree = ContextTree::with_id("pc2_invalidate_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("pc2_before", "PC2_BEFORE_CACHE")],
        PactAttrs::new(),
    )
    .expect("add setup message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal setup message");
    force_current_view_empty_for_trace_replay(&mut tree);

    let initial = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("project initial")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected view projection, got {other:?}"),
    };
    assert!(
        initial.component_records().values().any(|node| {
            node.node_type == "text_content" && node.content.as_deref() == Some("PC2_BEFORE_CACHE")
        }),
        "initial project must populate the current cache"
    );

    tree.insert(text_component("pc2_after", "PC2_AFTER_CACHE"), "root")
        .expect("stage after cache");
    let after = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("project after staged write")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected view projection, got {other:?}"),
    };
    assert!(
        after.component_records().values().any(|node| {
            node.node_type == "text_content" && node.content.as_deref() == Some("PC2_AFTER_CACHE")
        }),
        "a staged write must invalidate the cached current tree before the next read"
    );
}

#[test]
fn pc2_current_cache_invalidates_after_send_lifecycle_boundary() {
    let mut tree = ContextTree::with_id("pc2_send_invalidate_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![ttl_text_component("pc2_send_ttl", "PC2_SEND_TTL", 1)],
        PactAttrs::new(),
    )
    .expect("add ttl message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal ttl birth");

    let warm = project_current_view(&tree);
    assert!(
        view_has_text(&warm, "PC2_SEND_TTL"),
        "warm current cache must include the live ttl=1 component"
    );

    tree.send(&ProviderThreadOptions::default())
        .expect("close lifecycle boundary");
    let after = project_current_view(&tree);
    assert!(
        !view_has_text(&after, "PC2_SEND_TTL"),
        "send() must invalidate the current cache after lifecycle expiry changes the tree"
    );
}

#[test]
fn pc2_current_cache_invalidates_after_discard_staged() {
    let mut tree = ContextTree::with_id("pc2_discard_invalidate_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("pc2_discard_base", "PC2_DISCARD_BASE")],
        PactAttrs::new(),
    )
    .expect("add setup message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal setup message");

    tree.insert(
        text_component("pc2_discard_staged", "PC2_DISCARD_STAGED"),
        "root",
    )
    .expect("stage discard marker");
    let staged = project_current_view(&tree);
    assert!(
        view_has_text(&staged, "PC2_DISCARD_STAGED"),
        "staged marker must warm the current cache before discard"
    );

    assert!(tree.discard_staged().expect("discard staged"));
    let after = project_current_view(&tree);
    assert!(
        !view_has_text(&after, "PC2_DISCARD_STAGED"),
        "discard_staged() must invalidate the current cache so discarded content is gone"
    );
}

#[test]
fn pc2_following_overlay_same_handle_bypasses_current_cache_for_base_freshness() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let base_path = temp_pact_path("pc2_following_cache_base");
    let overlay_path = temp_pact_path("pc2_following_cache_overlay");
    let mut base = ContextTree::new().expect("base");
    base.add_message(
        "user",
        vec![text_component(
            "pc2_following_initial",
            "PC2_FOLLOWING_INITIAL",
        )],
        PactAttrs::new(),
    )
    .expect("add initial base message");
    base.send(&ProviderThreadOptions::default())
        .expect("seal initial base message");
    base.save_file(&base_path).expect("save base");

    file_fork_json(
        &base_path,
        Some(overlay_path.as_path()),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        1024,
    )
    .expect("create following overlay");
    let overlay = ContextTree::open_file(&overlay_path, Some("main")).expect("open overlay");

    let warm = project_current_view(&overlay);
    assert!(view_has_text(&warm, "PC2_FOLLOWING_INITIAL"));
    assert!(
        !view_has_text(&warm, "PC2_FOLLOWING_ADVANCED"),
        "warm following overlay read happens before the base advances"
    );

    base.add_message(
        "user",
        vec![text_component(
            "pc2_following_advanced",
            "PC2_FOLLOWING_ADVANCED",
        )],
        PactAttrs::new(),
    )
    .expect("add advanced base message");
    base.send(&ProviderThreadOptions::default())
        .expect("seal advanced base message");

    let fresh = project_current_view(&overlay);
    assert!(
        view_has_text(&fresh, "PC2_FOLLOWING_ADVANCED"),
        "following overlays must bypass the current cache so same-handle reads see base advances"
    );

    let _ = std::fs::remove_file(base_path);
    let _ = std::fs::remove_file(overlay_path);
}

#[test]
fn context_tree_project_component_reads_composed_overlay_subtree() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let base_path = temp_pact_path("rh1_overlay_base");
    let mut base = ContextTree::new().expect("base");
    base.add_message(
        "user",
        vec![text_component("rh1_marker", "RH1_BASE_MARKER")],
        PactAttrs::new(),
    )
    .expect("add base message");
    base.send(&ProviderThreadOptions::default())
        .expect("seal base message into a revision");
    base.save_file(&base_path).expect("save base");

    let overlay_path = temp_pact_path("rh1_overlay_child");
    file_fork_json(
        &base_path,
        Some(overlay_path.as_path()),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        1024,
    )
    .expect("overlay fork");

    let overlay = ContextTree::open_file(&overlay_path, Some("main")).expect("open overlay");
    let composed = match overlay
        .project(ProjectionRequest::Json {
            mode: ViewMode::Current,
        })
        .expect("project composed overlay")
    {
        ProjectionOutput::Json(json) => {
            serde_json::from_str::<serde_json::Value>(&json).expect("decode composed overlay json")
        }
        other => panic!("expected json projection, got {other:?}"),
    };
    let expected_component = find_component_value_by_content(&composed, "RH1_BASE_MARKER")
        .expect("marker component from composed projection");
    let component_id = expected_component
        .get("id")
        .and_then(serde_json::Value::as_str)
        .expect("marker component id from composed projection")
        .to_string();
    let projected = overlay
        .project_component(&component_id)
        .expect("project component from composed overlay");

    assert_eq!(
        &projected, expected_component,
        "project_component must route through tree_for(Current), not raw child-local state"
    );

    std::fs::remove_file(&base_path).ok();
    std::fs::remove_file(&overlay_path).ok();
}

#[test]
fn context_tree_project_at_revision_matches_file_projection_at_revision() {
    let path = temp_pact_path("rh4_revision_projection");
    let mut tree = ContextTree::with_id("rh4_revision_ctx").expect("tree");
    tree.save_file(&path).expect("save initial file");
    let first_payload = "f".repeat(crate::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES + 140);
    let second_payload = "s".repeat(crate::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES + 140);

    let insert = tree
        .insert(text_component("rh4_item", &first_payload), "root")
        .expect("insert");
    let item_id = first_produced_id(&insert);
    tree.send(&ProviderThreadOptions::default())
        .expect("seal insert");

    tree.update(
        &item_id,
        ComponentChanges {
            content: Some(second_payload.clone()),
            ..ComponentChanges::default()
        },
    )
    .expect("update");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal update");

    tree.insert(text_component("rh4_other", "other"), "root")
        .expect("insert other");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal other");

    tree.prune(&item_id).expect("prune original");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal prune");

    let opened = ContextTree::open_file(&path, Some("main")).expect("open file");
    let head_revision = opened.head().expect("head").head_revision;
    assert!(
        head_revision >= 4,
        "fixture should have at least four revisions"
    );

    for revision in [0, 2, head_revision] {
        let held_projection = serde_json::to_value(
            opened
                .project_at_revision(revision)
                .expect("held projection at revision"),
        )
        .expect("held projection json");
        let file_projection: serde_json::Value = serde_json::from_str(
            &file_projection_json(&path, Some("main"), Some(revision), 1024, None, None)
                .expect("file projection at revision"),
        )
        .expect("file projection json");
        assert_eq!(
            held_projection, file_projection,
            "held projection must match file projection exactly at revision {revision}"
        );
        if revision == 2 {
            assert!(
                held_projection.to_string().contains(&second_payload),
                "revision projection should use the handle resolver inline limit, not preview limit"
            );
        }
    }

    let current_view = match opened
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("current projection")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected current view, got {other:?}"),
    };
    let head_projection = opened
        .project_at_revision(head_revision)
        .expect("head projection");
    assert_eq!(
        head_projection.branch, current_view,
        "project_at(head) should overlap with the current projection branch payload"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn context_tree_project_at_revision_rules_out_overlay_handles() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let base_path = temp_pact_path("rh4_revision_overlay_base");
    let mut base = ContextTree::new().expect("base");
    base.add_message(
        "user",
        vec![text_component("rh4_base", "RH4_BASE_MARKER")],
        PactAttrs::new(),
    )
    .expect("add base message");
    base.send(&ProviderThreadOptions::default())
        .expect("seal base message");
    base.save_file(&base_path).expect("save base");

    let overlay_path = temp_pact_path("rh4_revision_overlay_child");
    file_fork_json(
        &base_path,
        Some(overlay_path.as_path()),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        crate::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
    )
    .expect("overlay fork");

    let overlay = ContextTree::open_file(&overlay_path, Some("main")).expect("open overlay");
    let err = overlay
        .project_at_revision(0)
        .expect_err("overlay at-revision must be explicitly unsupported");
    assert!(
        matches!(err, PactError::Unsupported(_)),
        "overlay at-revision should be a typed unsupported error: {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "at-revision projection not supported on overlay handles yet"
    );

    std::fs::remove_file(&base_path).ok();
    std::fs::remove_file(&overlay_path).ok();
}

#[test]
fn at_revision_replay_preserves_birth_structural_components() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let path = temp_pact_path("eng_rb_replay_birth");
    let fork_path = temp_pact_path("eng_rb_replay_birth_fork");
    let head_revision = write_replay_birth_skeleton_fixture(&path);
    assert_eq!(head_revision, 4, "fixture should seal four transactions");

    let head_projection: serde_json::Value = serde_json::from_str(
        &file_projection_json(&path, Some("main"), None, 1024, None, None)
            .expect("normal head projection"),
    )
    .expect("head projection json");
    let explicit_head_projection: serde_json::Value = serde_json::from_str(
        &file_projection_json(&path, Some("main"), Some(head_revision), 1024, None, None)
            .expect("explicit head projection"),
    )
    .expect("explicit head projection json");
    assert_eq!(
        explicit_head_projection, head_projection,
        "at-head projection must stay byte-shape equivalent to the non-replay head path"
    );

    let opened = ContextTree::open_file(&path, Some("main")).expect("open fixture");
    for revision in 0..=head_revision {
        let file_projection: serde_json::Value = serde_json::from_str(
            &file_projection_json(&path, Some("main"), Some(revision), 1024, None, None)
                .expect("file projection at revision"),
        )
        .expect("file projection json");
        let handle_projection = serde_json::to_value(
            opened
                .project_at_revision(revision)
                .expect("handle projection at revision"),
        )
        .expect("handle projection json");
        assert_eq!(
            handle_projection, file_projection,
            "file/handle projection parity at revision {revision}"
        );
    }

    let revision_zero: serde_json::Value = serde_json::from_str(
        &file_projection_json(&path, Some("main"), Some(0), 1024, None, None)
            .expect("revision zero projection"),
    )
    .expect("revision zero json");
    let revision_zero_components = projection_components(&revision_zero);
    let system = revision_zero_components
        .get("system")
        .expect("birth system header should survive revision zero replay");
    assert!(
        revision_zero_components.contains_key("system_title"),
        "birth structural descendants survive"
    );
    assert!(
        !revision_zero_components.contains_key("system_message"),
        "trace-created descendants must not be present in the birth skeleton"
    );
    let system_child_ids = system["child_ids"]
        .as_array()
        .expect("system child ids")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        system_child_ids,
        BTreeSet::from(["system_title", "system_policy"]),
        "birth structural children are preserved"
    );

    let revision_one: serde_json::Value = serde_json::from_str(
        &file_projection_json(&path, Some("main"), Some(1), 1024, None, None)
            .expect("revision one projection"),
    )
    .expect("revision one json");
    let revision_one_components = projection_components(&revision_one);
    assert!(
        revision_one_components
            .values()
            .any(|component| component["content"] == "system message before update"),
        "first replayed insert under birth system succeeds"
    );
    assert!(
        !revision_one_components
            .values()
            .any(|component| component["content"] == "root message to prune"),
        "later root insert is absent from the early revision"
    );

    let revision_three: serde_json::Value = serde_json::from_str(
        &file_projection_json(&path, Some("main"), Some(3), 1024, None, None)
            .expect("revision three projection"),
    )
    .expect("revision three json");
    assert!(
        projection_components(&revision_three)
            .values()
            .any(|component| component["content"] == "system message after update"),
        "updates replay over birth structural parents"
    );

    file_fork_json(
        &path,
        Some(fork_path.as_path()),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        None,
        1024,
    )
    .expect("fork at early revision");
    let fork_projection: serde_json::Value = serde_json::from_str(
        &file_projection_json(&fork_path, Some("main"), None, 1024, None, None)
            .expect("fork head projection"),
    )
    .expect("fork projection json");
    let fork_components = projection_components(&fork_projection);
    assert!(fork_components.contains_key("system"));
    assert!(
        fork_components
            .values()
            .any(|component| component["content"] == "system message before update")
    );
    assert!(
        !fork_components
            .values()
            .any(|component| component["content"] == "root message to prune"),
        "fork materializes exactly the requested pre-root-insert revision"
    );

    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&fork_path).ok();
}

#[test]
fn context_tree_rewind_targets_follow_message_order_and_revision_windows() {
    let mut tree = ContextTree::with_id("rh4_targets_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("rh4_user_1", "first user")],
        PactAttrs::new(),
    )
    .expect("first user");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal first user");
    tree.receive(
        vec![text_component("rh4_assistant_1", "first assistant")],
        PactAttrs::new(),
    )
    .expect("receive first assistant");
    tree.add_message(
        "user",
        vec![text_component("rh4_user_2", "second user")],
        PactAttrs::new(),
    )
    .expect("second user");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal second user");

    let mut ledger = tree.resolver.ledger().clone();
    let branch = ledger.branches.get_mut("main").expect("branch");
    let mut root = branch.current.tree.root.clone();
    let staged_turn = Component::from_component_value(&json!({
        "id": "rh4_untraced_turn",
        "componentType": "message_turn",
        "role": "assistant",
        "created_at_ns": 9_999_999_u64,
        "creation_index": 9_999_u64,
        "children": [
            {
                "id": "rh4_untraced_text",
                "componentType": "text_content",
                "content": "staged assistant",
                "created_at_ns": 10_000_000_u64,
                "creation_index": 10_000_u64
            }
        ]
    }))
    .expect("staged turn");
    let staged_id = staged_turn.id.clone();
    root.children.push(staged_turn);
    let mut first_seen = branch.current.tree.first_seen.clone();
    first_seen.insert(staged_id.clone(), branch.head_revision);
    first_seen.insert("rh4_untraced_text".to_string(), branch.head_revision);
    let projection = crate::store::build_projection_from_component(
        &root,
        &first_seen,
        crate::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
    )
    .expect("projection with untraced turn");
    branch.current = crate::store::PactView::from_projection(
        projection,
        branch.branch_id.clone(),
        branch.head_revision,
    );

    let traces = std::mem::take(&mut ledger.event_index.transaction_traces);
    for (index, (_, trace)) in traces.into_iter().rev().enumerate() {
        ledger
            .event_index
            .transaction_traces
            .insert(format!("rh4_scrambled_{index:02}"), trace);
    }
    tree.resolver = PactResolver::from_ledger(ledger);
    let view = match tree
        .project(ProjectionRequest::View {
            mode: ViewMode::Current,
        })
        .expect("current view")
    {
        ProjectionOutput::View(view) => view,
        other => panic!("expected current view, got {other:?}"),
    };
    assert!(
        view.message_order.contains(&staged_id),
        "fixture should include the untraced message turn: {:?}",
        view.message_order
    );

    let envelope = tree.rewind_targets().expect("rewind targets");
    assert_eq!(envelope["schema"], "pact.rewind_targets.v1");
    assert_eq!(envelope["branch_id"], "main");
    let targets = envelope["targets"].as_array().expect("targets array");
    assert_eq!(targets.len(), view.message_order.len());
    for (order, target) in targets.iter().enumerate() {
        assert_eq!(target["order"], order);
        assert_eq!(target["message_id"], view.message_order[order]);
    }

    let first_user = targets
        .iter()
        .find(|target| target["role"] == "user" && target["transaction_id"].is_string())
        .expect("first sealed user target");
    assert_eq!(first_user["from_revision"], 0);
    assert_eq!(first_user["to_revision"], 1);
    assert_eq!(first_user["before_ref"]["revision"], 0);
    assert_eq!(first_user["at_ref"]["revision"], 1);
    assert_eq!(first_user["at_ref"]["branch_id"], "main");
    assert!(first_user["at_ref"]["root_id"].is_string());
    assert!(first_user["at_ref"]["transaction_id"].is_string());

    let first_assistant = targets
        .iter()
        .find(|target| target["role"] == "assistant" && target["transaction_id"].is_string())
        .expect("first sealed assistant target");
    assert_eq!(first_assistant["from_revision"], 1);
    assert_eq!(first_assistant["to_revision"], 2);

    let staged = targets
        .iter()
        .find(|target| target["role"] == "assistant" && target["transaction_id"].is_null())
        .expect("staged assistant target");
    assert!(staged["from_revision"].is_null());
    assert!(staged["to_revision"].is_null());
    assert!(staged["before_ref"].is_null());
    assert!(staged["at_ref"].is_null());
}

#[test]
fn prune_removes_current_projection_but_preserves_historical_rewind() {
    let mut tree = ContextTree::with_id("dead_turn_contract").expect("tree");
    let path = temp_pact_path("dead_turn_contract");
    tree.save_file(&path).expect("bind file");
    let mut first = text_component("dead_turn_text", "turn one");
    first.key = Some("u1".to_string());
    tree.add_message("user", vec![first], PactAttrs::new())
        .expect("add user turn");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal turn");
    tree.receive(
        vec![text_component("dead_reply_1", "reply one")],
        PactAttrs::new(),
    )
    .expect("receive first reply");
    let mut second = text_component("dead_turn_text_2", "turn two");
    second.key = Some("u2".to_string());
    tree.add_message("user", vec![second], PactAttrs::new())
        .expect("add second user turn");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal second turn");
    tree.receive(
        vec![text_component("dead_reply_2", "reply two")],
        PactAttrs::new(),
    )
    .expect("receive second reply");
    let mut third = text_component("dead_turn_text_3", "turn three");
    third.key = Some("u3".to_string());
    tree.add_message("user", vec![third], PactAttrs::new())
        .expect("add third user turn");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal third turn");

    let before_prune = tree.head().expect("head").head_revision;
    let first_turn = tree
        .select("*", false)
        .expect("select turn")
        .into_iter()
        .find(|message| message.key.as_deref() == Some("u1"))
        .map(|message| message.id)
        .expect("first turn");

    tree.prune(&first_turn).expect("prune");
    let staged_xml = match tree
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("staged current xml")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected XML, got {other:?}"),
    };
    assert!(!staged_xml.contains("turn one"));

    tree.send(&ProviderThreadOptions::default())
        .expect("seal prune");

    let current_xml = match tree
        .project(ProjectionRequest::Xml {
            mode: ViewMode::Current,
        })
        .expect("current xml")
    {
        ProjectionOutput::Xml(xml) => xml,
        other => panic!("expected XML, got {other:?}"),
    };
    assert!(!current_xml.contains("turn one"));

    let historical =
        serde_json::to_string(&tree.project_at_revision(before_prune).expect("historical"))
            .expect("historical JSON");
    assert!(historical.contains("turn one"));

    std::fs::remove_file(path).ok();
}

#[test]
fn context_tree_stream_matches_file_stream_window_shapes() {
    let path = temp_pact_path("rh3_stream_parity");
    write_stream_fixture(&path);
    let opened = ContextTree::open_file(&path, Some("main")).expect("open stream fixture");
    let path_display = path.display().to_string();

    let first_page: serde_json::Value = serde_json::from_str(
        &file_stream_json(
            &path,
            "current_surface",
            Some("main"),
            None,
            None,
            None,
            "after",
            10,
            1024,
        )
        .expect("file stream"),
    )
    .expect("file stream json");
    let first_row_id = first_page["rows"][0]["row_id"]
        .as_str()
        .expect("first row id")
        .to_string();

    let cases = [
        ("current_surface", None, None, "before", 80),
        (
            "current_surface",
            Some(first_row_id.as_str()),
            None,
            "after",
            1,
        ),
        ("current_surface", None, Some(0), "around", 1),
        ("activity_surface", None, None, "after", 500),
        ("live-surface", None, None, "after", 500),
        (
            "current_surface",
            Some("node:missing"),
            Some(999),
            "before",
            9999,
        ),
    ];
    for (mode, anchor_row_id, anchor_seq, direction, limit) in cases {
        let file_stream: serde_json::Value = serde_json::from_str(
            &file_stream_json(
                &path,
                mode,
                Some("main"),
                None,
                anchor_row_id,
                anchor_seq,
                direction,
                limit,
                1024,
            )
            .expect("file stream"),
        )
        .expect("file stream json");
        let held_stream = opened
            .stream(
                mode,
                anchor_row_id,
                anchor_seq,
                direction,
                limit,
                Some(&path_display),
            )
            .expect("held stream");
        assert_eq!(
            stream_without_source(held_stream),
            stream_without_source(file_stream),
            "held stream must match file stream for {mode}/{direction}/{anchor_row_id:?}/{anchor_seq:?}/{limit}"
        );
    }

    std::fs::remove_file(&path).ok();
}

#[test]
fn context_tree_stream_reports_live_alias_and_bad_mode_contract() {
    let path = temp_pact_path("rh3_stream_alias");
    write_stream_fixture(&path);
    let opened = ContextTree::open_file(&path, Some("main")).expect("open stream fixture");

    let stream = opened
        .stream("live", None, None, "after", 10, None)
        .expect("live alias stream");
    assert_eq!(stream["mode"], "activity_surface");
    let error = opened
        .stream("nonsense", None, None, "after", 10, None)
        .expect_err("bad stream mode")
        .to_string();
    assert!(
        error.contains("live_surface"),
        "bad mode error must name live_surface alias: {error}"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn context_tree_stream_reads_composed_overlay_rows() {
    use crate::file_format::{PactForkMode, file_fork_json};

    let base_path = temp_pact_path("rh3_stream_overlay_base");
    let mut base = ContextTree::new().expect("base");
    base.add_message(
        "user",
        vec![text_component("rh3_base", "RH3_BASE_STREAM_MARKER")],
        PactAttrs::new(),
    )
    .expect("add base message");
    base.send(&ProviderThreadOptions::default())
        .expect("seal base message");
    base.save_file(&base_path).expect("save base");

    let overlay_path = temp_pact_path("rh3_stream_overlay_child");
    file_fork_json(
        &base_path,
        Some(overlay_path.as_path()),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        1024,
    )
    .expect("overlay fork");

    let overlay = ContextTree::open_file(&overlay_path, Some("main")).expect("open overlay");
    let stream = overlay
        .stream("current_surface", None, None, "after", 10, None)
        .expect("stream overlay");
    assert!(
        stream["rows"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row.get("payload")
                    .and_then(|payload| payload.get("child_ids"))
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|child_ids| {
                        child_ids.iter().any(|child_id| {
                            child_id
                                .as_str()
                                .is_some_and(|child_id| child_id.ends_with(":rh3_base"))
                        })
                    })
            })
        }),
        "overlay stream must use composed tree_for(Current), not raw local child state: {stream}"
    );

    std::fs::remove_file(&base_path).ok();
    std::fs::remove_file(&overlay_path).ok();
}

#[test]
fn context_tree_read_methods_do_not_reopen_the_backing_file() {
    let path = temp_pact_path("rh1_no_reread");
    let big = "read handle content ".repeat(150);
    let mut tree = ContextTree::with_id("rh1_read_ctx").expect("tree");
    tree.add_message(
        "user",
        vec![text_component("rh1_user", "hello")],
        PactAttrs::new(),
    )
    .expect("user");
    tree.send(&ProviderThreadOptions::default()).expect("send");
    tree.receive(vec![text_component("rh1_payload", &big)], PactAttrs::new())
        .expect("receive");
    tree.save_file(&path).expect("save");

    let opened = ContextTree::open_file(&path, Some("main")).expect("open file once");
    let selected = opened
        .select(".text_content", false)
        .expect("select text components");
    let content_id = selected
        .iter()
        .find(|component| component.id.ends_with(":rh1_payload"))
        .and_then(|component| component.content_ref.as_ref())
        .and_then(|content_ref| content_ref.get("id"))
        .and_then(serde_json::Value::as_str)
        .expect("externalized content id")
        .to_string();

    let file_payload = serde_json::from_str::<serde_json::Value>(
        &file_content_payload_json(&path, &content_id).expect("file payload"),
    )
    .expect("decode file payload");
    let held_payload = opened
        .content_payload(&content_id)
        .expect("read held content payload");
    assert_eq!(
        held_payload, file_payload,
        "read-handle content payload must match file_content_payload_json exactly"
    );

    let renamed = path.with_extension("renamed");
    std::fs::rename(&path, &renamed).expect("rename backing file after open");

    let component = opened
        .project_component("rh1_payload")
        .expect("project component after backing file rename");
    assert_eq!(component["content"], big);

    let payload = opened
        .content_payload(&content_id)
        .expect("read held content payload after backing file rename");
    assert_eq!(payload["content_id"], content_id);
    assert_eq!(payload["text"], big);

    let traces = opened
        .traces()
        .expect("read held transaction traces after backing file rename");
    assert!(
        traces.as_array().is_some_and(|items| !items.is_empty()),
        "transaction traces should read from held ledger state"
    );
    let first_trace = traces
        .as_array()
        .and_then(|items| items.first())
        .expect("trace row");
    assert_eq!(first_trace["branch_id"], "main");
    assert!(
        first_trace
            .get("transaction_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "transaction trace row must expose transaction identity"
    );

    std::fs::remove_file(&renamed).ok();
}

#[test]
fn context_tree_read_methods_report_missing_ids() {
    let path = temp_pact_path("rh1_not_found");
    let mut tree = ContextTree::with_id("rh1_not_found_ctx").expect("tree");
    tree.save_file(&path).expect("save");
    tree.insert(text_component("present", "present"), "root")
        .expect("insert");
    tree.send(&ProviderThreadOptions::default()).expect("send");

    let opened = ContextTree::open_file(&path, Some("main")).expect("open");
    let missing_component = opened
        .project_component("missing_component_rh1")
        .expect_err("missing component should error")
        .to_string();
    assert!(
        missing_component.contains("missing_component_rh1"),
        "component error should name the missing id: {missing_component}"
    );
    let missing_content = opened
        .content_payload("missing_content_rh1")
        .expect_err("missing content should error")
        .to_string();
    assert!(
        missing_content.contains("missing_content_rh1"),
        "content error should name the missing id: {missing_content}"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn context_tree_read_handle_snapshot_requires_explicit_reopen() {
    let path = temp_pact_path("rh1_snapshot");
    let mut writer = ContextTree::with_id("rh1_snapshot_ctx").expect("writer");
    writer.save_file(&path).expect("save");
    writer
        .insert(text_component("before_snapshot", "before"), "root")
        .expect("insert before");
    writer
        .send(&ProviderThreadOptions::default())
        .expect("send before");

    let opened = ContextTree::open_file(&path, Some("main")).expect("open snapshot");

    writer
        .insert(text_component("after_snapshot", "after"), "root")
        .expect("insert after");
    writer
        .send(&ProviderThreadOptions::default())
        .expect("send after");

    assert!(
        opened.project_component("after_snapshot").is_err(),
        "held read state must not observe later writes implicitly"
    );

    let refreshed = ContextTree::open_file(&path, Some("main")).expect("explicit reopen");
    let after = refreshed
        .project_component("after_snapshot")
        .expect("explicit reopen sees later write");
    assert_eq!(after["content"], "after");

    std::fs::remove_file(&path).ok();
}
