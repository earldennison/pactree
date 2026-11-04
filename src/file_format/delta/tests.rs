use crate::error::PactError;
use crate::file_format::{
    PactForkMode, ParentResolverConfig, file_create_from_ledger_json, file_fork_json,
    file_open_ledger, project_delta_overlay,
};
use crate::store::{
    ContextTreeLedger, PactBaseMode, PactBaseProvenance, PactParentLocator, PactRef,
    new_ledger_json,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("delta_{name}_{nanos}"));
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn open_ledger(path: &Path) -> ContextTreeLedger {
    file_open_ledger(path).expect("decode ledger")
}

fn source_rev0_json() -> String {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "attrs": {"cycle": 1},
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "attrs": {"cycle": 1},
                "content": "base content from A"
            }]
        }]
    });
    new_ledger_json(&tree.to_string(), Some("main"), 8).expect("source rev0 ledger")
}

fn append_later_revision(source_json: &str) -> String {
    use crate::components::Component;
    use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
    use crate::resolver::PactResolver;
    use crate::trace::BoundaryKind;

    let ledger =
        crate::store::decode_context_tree_ledger(source_json).expect("decode source ledger");
    let component = Component::from_component_value(&json!({
        "id": "text_2",
        "componentType": "text_content",
        "type": "text_content",
        "content": "later content from A",
    }))
    .expect("text_2 component");
    let op = PactOperation::Append {
        envelope: PactOperationEnvelope {
            branch_id: "main".to_string(),
            source: SourceActor::Host,
            source_id: Some("delta_test".to_string()),
            host_timestamp_ns: None,
            idempotency_key: None,
            aux: BTreeMap::new(),
        },
        target_component_id: Some("turn_1".to_string()),
        target_selector: None,
        offset: None,
        component,
    };
    let mut resolver = PactResolver::from_ledger(ledger);
    let outcome = resolver
        .resolve(vec![op], "main", BoundaryKind::Authored)
        .expect("resolve append");
    let candidate = outcome.commit_candidate.expect("commit candidate");
    resolver.seal(&candidate).expect("seal append");
    serde_json::to_string(&resolver.into_ledger()).expect("encode mutated ledger")
}

fn find_node<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if node.get("id").and_then(Value::as_str) == Some(id) {
        return Some(node);
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = find_node(child, id) {
            return Some(found);
        }
    }
    None
}

fn config_for_base(base_root_id: &str, base_path: &Path) -> ParentResolverConfig {
    let mut explicit_parents = BTreeMap::new();
    explicit_parents.insert(base_root_id.to_string(), base_path.to_path_buf());
    ParentResolverConfig {
        base_dir: None,
        search_paths: Vec::new(),
        explicit_parents,
    }
}

#[test]
fn base_provenance_serde_round_trip() {
    let provenance = PactBaseProvenance {
        base_ref: PactRef {
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            revision: Some(2),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        },
        mode: PactBaseMode::Pinned,
        created_from_head: PactRef {
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            revision: Some(2),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        },
        locator_hints: vec![PactParentLocator {
            kind: "relative_path".to_string(),
            value: "base.pact".to_string(),
        }],
        aux: BTreeMap::new(),
    };
    let encoded = serde_json::to_string(&provenance).expect("encode base provenance");

    assert!(
        encoded.contains("\"pinned\""),
        "mode serializes snake_case: {encoded}"
    );
    let decoded: PactBaseProvenance =
        serde_json::from_str(&encoded).expect("decode base provenance");
    assert_eq!(decoded, provenance);

    let following = PactBaseProvenance {
        mode: PactBaseMode::Following,
        ..provenance.clone()
    };
    let encoded = serde_json::to_string(&following).expect("encode following");
    assert!(encoded.contains("\"following\""), "{encoded}");
}

#[test]
fn old_ledger_decodes_with_base_none() {
    let source_json = source_rev0_json();
    let mut value: Value = serde_json::from_str(&source_json).expect("ledger json");
    let object = value.as_object_mut().unwrap();
    assert!(
        object.remove("base").is_none(),
        "fresh ledger fixture should not carry a base field"
    );
    let ledger: ContextTreeLedger =
        serde_json::from_str(&value.to_string()).expect("decode old ledger");
    assert!(ledger.base.is_none(), "missing base must decode to None");
    assert!(ledger.parent.is_none());
}

#[test]
fn base_provenance_round_trips_through_pact_file() {
    let dir = unique_dir("base_file_round_trip");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");
    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source");

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        Some(0),
        None,
        8,
    )
    .expect("delta fork");

    let child_ledger = open_ledger(&child);
    let base = child_ledger
        .base
        .as_ref()
        .expect("child carries base provenance");
    assert_eq!(base.mode, PactBaseMode::Pinned);
    assert_eq!(base.base_ref.root_id, "root");
    assert_eq!(base.base_ref.branch_id, "main");
    assert_eq!(base.base_ref.revision, Some(0));
    assert_eq!(base.created_from_head.revision, Some(0));

    assert_ne!(child_ledger.root_id, "root");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn delta_overlay_writes_child_and_leaves_parent_byte_unchanged() {
    let dir = unique_dir("writer_parent_unchanged");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");
    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source");

    let source_bytes_before = fs::read(&source).expect("read source before");

    let report_json = file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        Some(0),
        None,
        8,
    )
    .expect("delta fork");
    let report: Value = serde_json::from_str(&report_json).expect("report json");

    assert!(child.is_file(), "child .pact must exist");
    assert_eq!(report["mode"], "delta");
    assert_eq!(report["base_mode"], "pinned");
    assert_eq!(report["base_ref"]["root_id"], "root");
    assert_eq!(report["created_from_head"]["revision"], 0);

    let source_bytes_after = fs::read(&source).expect("read source after");
    assert_eq!(
        source_bytes_before, source_bytes_after,
        "delta overlay must not mutate the base file"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn delta_overlay_stores_local_traces_only_and_references_parent() {
    let dir = unique_dir("local_traces_only");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");
    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source");

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        Some(0),
        None,
        8,
    )
    .expect("delta fork");

    let child_ledger = open_ledger(&child);

    let base = child_ledger.base.as_ref().expect("base provenance");
    assert_eq!(base.mode, PactBaseMode::Following);
    assert_eq!(
        base.base_ref.revision, None,
        "following leaves base_ref open"
    );
    assert_eq!(base.created_from_head.revision, Some(0));

    let child_branch = child_ledger.branches.get("main").expect("child branch");
    assert!(
        child_branch.transaction_traces.is_empty(),
        "fresh delta overlay has an empty local trace stream"
    );

    assert!(
        find_node(
            &child_branch.current.tree.root.project_component_value(),
            "text_1"
        )
        .is_none(),
        "child must not copy the base's text_1 into its local context"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn delta_child_size_is_independent_of_parent_history() {
    let dir = unique_dir("size_independent");
    let small_source = dir.join("small.pact");
    let large_source = dir.join("large.pact");
    let small_child = dir.join("small_child.pact");
    let large_child = dir.join("large_child.pact");

    let small_json = source_rev0_json();
    file_create_from_ledger_json(&small_source, &small_json, 8).expect("create small source");

    let turns = (0..60)
        .map(|index| {
            json!({
                "id": format!("turn_{index}"),
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "user",
                "pact_depth": index,
                "attrs": {"cycle": index},
                "children": [{
                    "id": format!("text_{index}"),
                    "componentType": "text_content",
                    "type": "text_content",
                    "content": format!("large parent message {index}: {}", "context payload ".repeat(40))
                }]
            })
        })
        .collect::<Vec<_>>();
    let large_tree = json!({
        "id": "root",
        "componentType": "root",
        "children": turns,
    });
    let large_json =
        new_ledger_json(&large_tree.to_string(), Some("main"), 8).expect("large ledger");
    file_create_from_ledger_json(&large_source, &large_json, 8).expect("create large source");

    let small_parent_bytes = fs::metadata(&small_source).expect("small meta").len();
    let large_parent_bytes = fs::metadata(&large_source).expect("large meta").len();
    assert!(
        large_parent_bytes > small_parent_bytes * 2,
        "large parent must be substantially bigger ({large_parent_bytes} vs {small_parent_bytes})"
    );

    file_fork_json(
        &small_source,
        Some(&small_child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        Some(0),
        None,
        8,
    )
    .expect("small delta fork");
    file_fork_json(
        &large_source,
        Some(&large_child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("large delta fork");

    let small_child_bytes = fs::metadata(&small_child).expect("small child meta").len();
    let large_child_bytes = fs::metadata(&large_child).expect("large child meta").len();

    assert!(
        large_child_bytes < large_parent_bytes / 2,
        "delta child ({large_child_bytes}) must not approach parent size ({large_parent_bytes})"
    );
    let diff = large_child_bytes.abs_diff(small_child_bytes);
    assert!(
        diff < 2048,
        "delta child size must be ~independent of parent history (small={small_child_bytes}, large={large_child_bytes}, diff={diff})"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn pinned_delta_projection_is_stable_against_later_parent_writes() {
    let dir = unique_dir("pinned_stable");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source rev0");
    let base_root_id = open_ledger(&source).root_id;

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("pinned delta fork");

    let source_v2_json = append_later_revision(&source_json);
    file_create_from_ledger_json(&source, &source_v2_json, 8).expect("rewrite source v2");

    let child_ledger = open_ledger(&child);
    let config = config_for_base(&base_root_id, &source);
    let projection = project_delta_overlay(Some(&child), &child_ledger, Some("main"), &config)
        .expect("pinned projection");

    assert_eq!(projection.base_mode, PactBaseMode::Pinned);
    assert_eq!(
        projection.base_revision, 0,
        "pinned composes at fork-time head"
    );
    assert!(
        find_node(&projection.tree, "text_1").is_some(),
        "pinned overlay sees fork-time base content"
    );
    assert!(
        find_node(&projection.tree, "text_2").is_none(),
        "pinned overlay must be stable against later parent writes"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn following_delta_projection_sees_later_parent_writes() {
    let dir = unique_dir("following_tracks");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source rev0");
    let base_root_id = open_ledger(&source).root_id;

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("following delta fork");

    let source_v2_json = append_later_revision(&source_json);
    file_create_from_ledger_json(&source, &source_v2_json, 8).expect("rewrite source v2");

    let child_ledger = open_ledger(&child);
    let config = config_for_base(&base_root_id, &source);
    let projection = project_delta_overlay(Some(&child), &child_ledger, Some("main"), &config)
        .expect("following projection");

    assert_eq!(projection.base_mode, PactBaseMode::Following);
    assert_eq!(
        projection.base_revision, 1,
        "following composes at latest head"
    );
    assert!(
        find_node(&projection.tree, "text_1").is_some(),
        "following overlay sees base content"
    );
    assert!(
        find_node(&projection.tree, "text_2").is_some(),
        "following overlay must track later parent writes"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn delta_projection_does_not_mutate_parent() {
    let dir = unique_dir("no_parent_mutation");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source");
    let base_root_id = open_ledger(&source).root_id;

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork");

    let source_bytes_before = fs::read(&source).expect("read source before");
    let child_ledger = open_ledger(&child);
    let config = config_for_base(&base_root_id, &source);
    let _ = project_delta_overlay(Some(&child), &child_ledger, Some("main"), &config)
        .expect("projection");
    let source_bytes_after = fs::read(&source).expect("read source after");
    assert_eq!(
        source_bytes_before, source_bytes_after,
        "projecting the overlay must never mutate the base file"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn wrong_base_path_rejected_by_root_id_mismatch() {
    let dir = unique_dir("wrong_base");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");
    let other = dir.join("other.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source");
    let base_root_id = open_ledger(&source).root_id;

    let other_tree = json!({
        "id": "different_root",
        "componentType": "root",
        "children": [{
            "id": "x_turn",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "children": []
        }]
    });
    let other_json =
        new_ledger_json(&other_tree.to_string(), Some("main"), 8).expect("other ledger");
    file_create_from_ledger_json(&other, &other_json, 8).expect("create other");

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork");

    file_create_from_ledger_json(&source, &other_json, 8)
        .expect("overwrite base with a wrong-root-id ledger");

    let mut explicit_parents = BTreeMap::new();
    explicit_parents.insert(base_root_id.clone(), other.clone());
    let config = ParentResolverConfig {
        base_dir: None,
        search_paths: Vec::new(),
        explicit_parents,
    };
    let child_ledger = open_ledger(&child);
    let err = project_delta_overlay(Some(&child), &child_ledger, Some("main"), &config)
        .expect_err("wrong base must be rejected");
    assert!(
        matches!(err, PactError::ParentMismatch(_)),
        "wrong base path must be a typed base mismatch, got: {err:?}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn missing_base_is_typed_unresolved_error() {
    let dir = unique_dir("missing_base");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");

    let source_json = source_rev0_json();
    file_create_from_ledger_json(&source, &source_json, 8).expect("create source");

    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Delta { base_pinned: true },
        Some("main"),
        None,
        None,
        8,
    )
    .expect("delta fork");

    fs::remove_file(&source).expect("remove base");
    let child_ledger = open_ledger(&child);
    let config = ParentResolverConfig::default();
    let err = project_delta_overlay(Some(&child), &child_ledger, Some("main"), &config)
        .expect_err("missing base must be rejected");
    assert!(
        matches!(err, PactError::UnresolvedParent(_)),
        "missing base must be a typed unresolved error, got: {err:?}"
    );

    let _ = fs::remove_dir_all(dir);
}
