use crate::{
    Component, ContextTree, PactFileReadHandle, ProviderThreadOptions, SearchFieldLocator,
    file_format::{PactForkMode, file_fork_json},
};

fn source_fixture() -> (tempfile::TempDir, std::path::PathBuf, ContextTree) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("search-source.pact");
    let mut tree = ContextTree::with_id("search_source_root").expect("tree");
    tree.save_file(&path).expect("save");

    for (index, role) in ["user", "assistant"].into_iter().enumerate() {
        let mut turn = Component::message_turn_authoring(
            Some(role.to_string()),
            None,
            tree.next_authoring_component_options(),
        );
        turn.children.push(Component::text_authoring(
            format!("turn {index} bounded text ").repeat(20_000),
            tree.next_authoring_component_options(),
        ));
        if index == 1 {
            turn.children.push(Component::tool_call_authoring(
                "call_1".to_string(),
                "lookup".to_string(),
                Some(serde_json::json!({"query": "bounded".repeat(20_000)})),
                tree.next_authoring_component_options(),
            ));
            turn.children.push(Component::tool_result_authoring(
                "call_1".to_string(),
                Some("lookup".to_string()),
                Some(true),
                Some(serde_json::json!({"rows": ["result".repeat(20_000)]})),
                None,
                None,
                tree.next_authoring_component_options(),
            ));
        }
        tree.insert(turn, "root").expect("insert turn");
    }
    tree.send(&ProviderThreadOptions::default())
        .expect("commit fixture");
    (temp, path, tree)
}

#[test]
fn source_pages_are_pinned_payload_free_and_match_selector_ordinals() {
    let (_temp, path, mut tree) = source_fixture();
    crate::tests::instrumentation::reset_ledger_hydration_count();
    let mut handle = PactFileReadHandle::open(&path, None).expect("read handle");
    let pinned = handle.search_snapshot_info().expect("pinned info");
    let eligible = handle
        .search_selector_eligibility("*", false)
        .expect("all selector eligibility");

    let first = handle
        .search_source_page(None, 2, 64 * 1024)
        .expect("first source page");
    assert_eq!(first.components.len(), 2);
    assert_eq!(first.components[0].ordinal, 1);
    assert_eq!(first.components[1].ordinal, 2);
    assert_eq!(first.next_ordinal, Some(2));
    assert_eq!(first.total_components + 1, eligible.total_components);
    assert!(
        serde_json::to_vec(&first).expect("source page JSON").len() <= 64 * 1024,
        "metadata page exceeded its byte budget"
    );

    let later_options = tree.next_authoring_component_options();
    tree.insert(
        Component::text_authoring("appended later".to_string(), later_options),
        "root",
    )
    .expect("insert later");
    tree.send(&ProviderThreadOptions::default())
        .expect("commit later");

    let mut ordinals = first
        .components
        .iter()
        .map(|component| component.ordinal)
        .collect::<Vec<_>>();
    let mut cursor = first.next_ordinal;
    while let Some(after) = cursor {
        let page = handle
            .search_source_page(Some(after), 2, 64 * 1024)
            .expect("next source page");
        ordinals.extend(page.components.iter().map(|component| component.ordinal));
        cursor = page.next_ordinal;
    }
    assert_eq!(ordinals, (1..=first.total_components).collect::<Vec<_>>());
    assert_eq!(handle.search_snapshot_info().expect("still pinned"), pinned);
    assert_eq!(crate::tests::instrumentation::ledger_hydration_count(), 0);

    handle.refresh().expect("refresh");
    let refreshed = handle
        .search_source_page(None, 1, 64 * 1024)
        .expect("refreshed source page");
    assert_eq!(refreshed.total_components, first.total_components + 1);
}

#[test]
fn logical_search_fields_follow_externalized_tool_slots() {
    let (_temp, path, _tree) = source_fixture();
    let handle = PactFileReadHandle::open(&path, None).expect("read handle");
    let mut cursor = None;
    let mut tool_fields = Vec::new();
    loop {
        let page = handle
            .search_source_page(cursor, 2, 64 * 1024)
            .expect("source page");
        for component in page.components {
            if component.component_type == "tool_call" {
                tool_fields.push((component.component_id, "parameters".to_string()));
            } else if component.component_type == "tool_result" {
                tool_fields.push((component.component_id, "result".to_string()));
            }
        }
        let Some(next) = page.next_ordinal else {
            break;
        };
        cursor = Some(next);
    }
    assert_eq!(tool_fields.len(), 2);
    for (component_id, field) in tool_fields {
        let first = handle
            .read_search_field(
                &SearchFieldLocator {
                    component_id,
                    field,
                    offset: 0,
                },
                4096,
            )
            .expect("bounded logical field");
        assert!(!first.data.is_empty());
        assert!(first.data.len() <= 4096);
        assert!(first.next_offset.is_some());
    }
}

#[test]
fn source_page_rejects_a_budget_smaller_than_one_metadata_row() {
    let (_temp, path, _tree) = source_fixture();
    let handle = PactFileReadHandle::open(&path, None).expect("read handle");
    let error = handle
        .search_source_page(None, 10, 1)
        .expect_err("one byte cannot hold a source row");
    assert!(error.to_string().contains("metadata budget"));
}

#[test]
fn terminal_source_page_still_enforces_its_metadata_budget() {
    let (_temp, path, _tree) = source_fixture();
    let handle = PactFileReadHandle::open(&path, None).expect("read handle");
    let total = handle
        .search_source_page(None, 1, 64 * 1024)
        .expect("source page")
        .total_components;
    let error = handle
        .search_source_page(Some(total), 10, 1)
        .expect_err("terminal metadata must obey the byte budget");
    assert!(error.to_string().contains("metadata budget"));
}

#[test]
fn source_page_rejects_composed_branches_instead_of_indexing_only_local_rows() {
    let (temp, path, _tree) = source_fixture();
    let overlay = temp.path().join("search-source-overlay.pact");
    file_fork_json(
        &path,
        Some(&overlay),
        PactForkMode::Delta { base_pinned: false },
        Some("main"),
        None,
        None,
        1024,
    )
    .expect("create overlay");
    let handle = PactFileReadHandle::open(&overlay, Some("main")).expect("overlay read handle");

    let error = handle
        .search_source_page(None, 10, 64 * 1024)
        .expect_err("composed source must not expose an incomplete local-only surface");
    assert!(matches!(error, crate::PactError::Unsupported(_)));
    assert!(error.to_string().contains("directly indexed branch"));
}
