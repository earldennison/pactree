use std::collections::BTreeSet;
use std::sync::Arc;

use super::{SearchSelectorPlan, execute};
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::snapshot::Snapshot;

fn selector_fixture() -> (tempfile::TempDir, std::path::PathBuf, crate::ContextTree) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("selector-eligibility.pact");
    let mut tree = crate::ContextTree::with_id("selector_search_root").expect("tree");
    tree.save_file(&path).expect("save");

    for (index, role) in ["user", "assistant", "user"].into_iter().enumerate() {
        let mut text = crate::Component::text_authoring(
            format!("message {index}"),
            tree.next_authoring_component_options(),
        );
        text.tags.push(if index == 1 {
            "semantic:answer".to_string()
        } else {
            "semantic:request".to_string()
        });
        text.aux.insert(
            "lane".to_string(),
            serde_json::Value::String(if index == 1 { "answer" } else { "request" }.to_string()),
        );
        if index == 0 {
            text.tags.push("scaffold:file_manager:body".to_string());
        }
        let mut turn = crate::Component::message_turn_authoring(
            Some(role.to_string()),
            None,
            tree.next_authoring_component_options(),
        );
        turn.pact_depth = Some(index as i32 - 1);
        turn.priority = index as i32;
        turn.children.push(text);
        if index == 1 {
            let mut tool_call = crate::Component::tool_call_authoring(
                "call_1".to_string(),
                "lookup".to_string(),
                Some(serde_json::json!({"query": "bounded"})),
                tree.next_authoring_component_options(),
            );
            tool_call.tags.push("tool_call".to_string());
            tool_call.tags.push("urgent".to_string());
            turn.children.push(tool_call);
            let mut tool_result = crate::Component::tool_result_authoring(
                "call_1".to_string(),
                Some("lookup".to_string()),
                Some(true),
                Some(serde_json::json!({"rows": [1, 2, 3]})),
                None,
                None,
                tree.next_authoring_component_options(),
            );
            tool_result.key = Some("surface:shell".to_string());
            turn.children.push(tool_result);
        }
        tree.insert(turn, "root").expect("insert turn");
    }
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit fixture");
    (temp, path, tree)
}

fn canonical_ids(tree: &crate::ContextTree, selector: &str) -> BTreeSet<String> {
    tree.select(selector, true)
        .expect("canonical selector")
        .into_iter()
        .map(|component| component.id)
        .collect()
}

fn planned_ids(path: &std::path::Path, selector: &str) -> (SearchSelectorPlan, BTreeSet<String>) {
    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let plan = execute(&snapshot, "main", selector, false).expect("selector plan");
    let mut ids = BTreeSet::new();
    for ordinal in plan
        .bitmap()
        .map_or_else(Vec::new, super::OrdinalBitmap::ordinals)
    {
        if ordinal == 0 {
            ids.insert(snapshot.metadata().expect("metadata").root_id);
            continue;
        }
        let offset = usize::try_from(ordinal - 1).expect("test ordinal fits usize");
        let (_, components) = snapshot
            .components_shallow_window("main", offset, 1)
            .expect("component at ordinal");
        let component = components.first().expect("ordinal references component");
        ids.insert(
            super::component_id(&component.value)
                .expect("component id")
                .to_string(),
        );
    }
    (plan, ids)
}

#[test]
fn indexed_and_metadata_plans_match_canonical_node_local_selectors() {
    let (_temp, path, tree) = selector_fixture();
    crate::tests::instrumentation::reset_ledger_hydration_count();
    for selector in [
        ".text_content",
        ".message_turn[role=user]",
        "*[role=user]",
        "+semantic:answer",
        "+semantic",
        "+semantic:*",
        "*[lane~=req.*]",
        ".tool_call[tool_name=lookup]",
        "{tool_call_id=call_1}",
        ".tool_result[success=true]",
        ".tool_call, .text_content",
        ".text_content, .text_content",
        "*",
    ] {
        let (_plan, actual) = planned_ids(&path, selector);
        assert_eq!(
            actual,
            canonical_ids(&tree, selector),
            "selector {selector}"
        );
    }
    let handle = crate::PactFileReadHandle::open(&path, None).expect("read handle");
    let eligibility = handle
        .search_selector_eligibility(".text_content", false)
        .expect("public eligibility");
    assert_eq!(eligibility.eligible_components, 3);
    assert_eq!(eligibility.execution_plan, "indexed_bitmap");
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
fn bounded_structural_plan_matches_canonical_combinators_and_has() {
    let (_temp, path, tree) = selector_fixture();
    crate::tests::instrumentation::reset_ledger_hydration_count();
    for selector in [
        ".message_turn > .text_content",
        ".message_turn:has(.tool_call)",
        ".message_turn > .tool_call, .message_turn > .tool_result",
        ".text_content:first",
        ".text_content:nth(1)",
        "(d0,0)",
        "d0 > .text_content",
        ".text_content @offset=1 @limit=1",
        ".text_content @mode=latest",
    ] {
        let (plan, actual) = planned_ids(&path, selector);
        assert!(matches!(
            plan,
            SearchSelectorPlan::BoundedStructuralTraversal(_)
        ));
        assert_eq!(
            actual,
            canonical_ids(&tree, selector),
            "selector {selector}"
        );
    }
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);
}

#[test]
fn consumer_selector_syntax_matches_canonical_pact_semantics() {
    let (_temp, path, tree) = selector_fixture();
    for selector in [
        "+tool_call",
        "+tool_call+urgent",
        "+scaffold:file_manager:body",
        "+scaffold:file_manager:*",
        "d-1 +scaffold:file_manager:body",
        "(d0) +tool_call",
        "(d0) .tool_call",
        "d0,*",
        ".tool_result{key='surface:shell'}",
        ".message_turn{role='assistant'}:last",
        ".block:tool_result",
    ] {
        let (_plan, actual) = planned_ids(&path, selector);
        assert_eq!(
            actual,
            canonical_ids(&tree, selector),
            "consumer selector {selector}"
        );
    }
}

#[test]
fn noncanonical_consumer_forwarding_cases_remain_invalid() {
    let (_temp, path, tree) = selector_fixture();
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    for selector in ["p1", "@t*", "@t-1"] {
        let canonical = tree
            .select(selector, true)
            .expect_err("canonical selector must reject noncanonical syntax");
        let planned = execute(&snapshot, "main", selector, false)
            .expect_err("snapshot selector must preserve canonical rejection");
        assert_eq!(planned.kind(), canonical.kind(), "selector {selector}");
    }
}

#[test]
fn historical_selector_is_explicitly_unsupported_without_opt_in() {
    let (_temp, path, _tree) = selector_fixture();
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    for selector in [
        "@t-1 *",
        "@t-5 .text_content",
        "@c5 *",
        "@c10 .tool_call",
        "@history .text_content",
        "@* .text_content",
    ] {
        let error = execute(&snapshot, "main", selector, false)
            .expect_err("historical selector needs explicit opt in");
        assert!(
            error.to_string().contains("allow_expensive_selector=true"),
            "historical selector {selector}"
        );

        let plan = execute(&snapshot, "main", selector, true).expect("explicit expensive selector");
        assert!(matches!(
            plan,
            SearchSelectorPlan::UnsupportedForSnapshot { .. }
        ));
    }
}

#[test]
fn current_temporal_alias_and_cold_repack_preserve_eligibility() {
    let (temp, path, tree) = selector_fixture();
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let current =
        execute(&snapshot, "main", "@t0 .text_content", false).expect("current temporal selector");
    assert_eq!(current.bitmap().expect("current eligibility").count(), 3);
    assert_eq!(
        planned_ids(&path, "@t0 .text_content").1,
        canonical_ids(&tree, ".text_content")
    );

    let cold = temp.path().join("selector-eligibility-cold.pact");
    crate::file_format::file_repack_json(&path, &cold).expect("cold repack");
    let cold_snapshot =
        Snapshot::open(&cold, Arc::new(IoCounterSet::default())).expect("cold snapshot");
    for selector in ["{tool_call_id=call_1}", "[success=true]"] {
        let plan = execute(&cold_snapshot, "main", selector, false).expect("cold selector");
        assert!(matches!(plan, SearchSelectorPlan::IndexedBitmap(_)));
        assert_eq!(
            planned_ids(&cold, selector).1,
            canonical_ids(&tree, selector)
        );
    }
}

#[test]
fn internal_eligibility_exceeds_public_selector_limit_without_changing_it() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("large-eligibility.pact");
    let mut tree = crate::ContextTree::with_id("large_selector_root").expect("tree");
    for index in 0..1_101 {
        let options = tree.next_authoring_component_options();
        tree.insert(
            crate::Component::text_authoring(format!("eligible {index}"), options),
            "root",
        )
        .expect("insert eligible component");
    }
    tree.send(&crate::ProviderThreadOptions::default())
        .expect("commit in-memory eligible components");
    tree.save_file(&path).expect("save large cold baseline");

    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let plan = execute(&snapshot, "main", ".text_content", false).expect("eligibility");
    assert_eq!(plan.bitmap().expect("eligible bitmap").count(), 1_101);

    let mut query = crate::file_format::PactFileSelectorQuery::new(".text_content");
    query.limit = 5_000;
    let public: serde_json::Value = serde_json::from_str(
        &crate::file_format::file_select_json(&path, query).expect("public selector"),
    )
    .expect("selector JSON");
    assert_eq!(public["hits"].as_array().map(Vec::len), Some(1_000));
}

fn marked_fixture_with_stale_facet_depths() -> (tempfile::TempDir, std::path::PathBuf) {
    use crate::append_writer::{
        PactAppendBatch, PactAppendMessage, PactAppendOptions, PactAppendOutcome, PactAppendWriter,
    };
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("marked-search.pact");
    let options = crate::ContextTreeOptions {
        derived_depth: true,
        deterministic_authoring: Some(
            crate::DeterministicAuthoringOptions::new("ms_".to_string(), 1_000_000, 1)
                .expect("options"),
        ),
    };
    let mut tree = crate::ContextTree::with_id_and_options("ms_root", options).expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..2 {
        let role = if turn % 2 == 0 { "user" } else { "assistant" };
        let text = crate::Component::text_authoring(
            format!("eager turn {turn}"),
            tree.next_authoring_component_options(),
        );
        tree.add_message(role, vec![text], crate::PactAttrs::new())
            .expect("stage");
        tree.seal_staged(crate::trace::BoundaryKind::Adhoc)
            .expect("seal");
    }
    drop(tree);
    let mut writer = PactAppendWriter::open_with_options(
        &path,
        "main",
        crate::PactMemoryOptions::default(),
        PactAppendOptions {
            derived_depth: true,
        },
    )
    .expect("open writer");
    for row in 0..3 {
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
                        crate::Component::from_component_value(&serde_json::json!({
                            "componentType": "text_content",
                            "content": format!("bounded row {row} needle"),
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
    (temp, path)
}

#[test]
fn depth_selectors_on_a_marked_branch_rank_from_the_newest_not_from_the_facet() {
    let (_temp, path) = marked_fixture_with_stale_facet_depths();
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    assert!(
        snapshot
            .metadata()
            .expect("metadata")
            .derived_depth_branches
            .contains("main")
    );

    let mut facet_depths = BTreeSet::new();
    let (total, components) = snapshot
        .components_by_type("main", "message_turn", 0, 16)
        .expect("turns");
    assert_eq!(total, 5);
    for component in &components {
        facet_depths.insert(component.value["pact_depth"].as_i64());
    }
    assert!(
        facet_depths.contains(&Some(0)) && facet_depths.len() < 5,
        "the bounded rows entered the facets at depth 0: {facet_depths:?}"
    );
    drop(snapshot);

    let tree = crate::ContextTree::open_file(&path, None).expect("open");
    for selector in [
        "d0",
        "d1",
        "d3",
        "d4",
        "(d2)",
        "(d3,0)",
        "d3 .text_content",
        "d1 > .text_content",
        "{pact_depth=2}",
        ".text_content",
        "[role=user]",
    ] {
        let expected = canonical_ids(&tree, selector);
        let (plan, actual) = planned_ids(&path, selector);
        assert_eq!(actual, expected, "{selector} ({})", plan.kind());
        if crate::selector::selector_reads_depth(selector).expect("parse") {
            assert!(
                matches!(plan, SearchSelectorPlan::BoundedStructuralTraversal(_)),
                "{selector} takes the ranked tree on a marked branch, got {}",
                plan.kind()
            );
        }
    }
    assert!(!canonical_ids(&tree, "d3").is_empty(), "d3 names a turn");
    assert!(!canonical_ids(&tree, "d1 > .text_content").is_empty());
}
