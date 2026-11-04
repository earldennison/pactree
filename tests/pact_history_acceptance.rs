use std::path::PathBuf;

fn history_copy() -> Option<PathBuf> {
    std::env::var_os("PACT_HISTORY_COPY").map(PathBuf::from)
}

#[test]
#[ignore = "opt-in: PACT_HISTORY_COPY=<disposable copy of a real .pact>"]
fn real_history_copy_accepts_one_more_turn_and_reopens() {
    let Some(source) = history_copy() else {
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("history.pact");
    std::fs::copy(&source, &path).expect("copy history");

    let before = pactree::PactFileReadHandle::open(&path, None).expect("indexed open before");
    let info_before = before.info().expect("info");
    let revision_before = info_before["revision"].as_u64().expect("revision");
    let components_before = info_before["stats"]["component_count"]
        .as_u64()
        .expect("count");
    drop(before);

    let mut tree = pactree::ContextTree::open_file(&path, None).expect("materialized open");
    let options = tree.next_authoring_component_options();
    tree.add_message(
        "user",
        vec![pactree::Component::text_authoring(
            "acceptance turn".to_string(),
            options,
        )],
        pactree::PactAttrs::new(),
    )
    .expect("stage message");
    tree.seal_staged(pactree::trace::BoundaryKind::Adhoc)
        .expect("seal must not hit an index batch or message order limit");
    drop(tree);

    let after = pactree::PactFileReadHandle::open(&path, None).expect("indexed reopen");
    let info_after = after.info().expect("info");
    assert_eq!(info_after["revision"].as_u64(), Some(revision_before + 1));
    assert!(
        info_after["stats"]["component_count"]
            .as_u64()
            .expect("count")
            >= components_before + 2
    );

    let report = pactree::file_format::file_message_order_report(&path).expect("order report");
    eprintln!("order report: {report}");
    if let Some(out) = std::env::var_os("PACT_ACCEPTANCE_OUT") {
        std::fs::copy(&path, out).expect("keep appended copy");
    }
}

#[test]
#[ignore = "opt-in: PACT_HISTORY_COPY=<disposable copy of a real .pact>"]
fn real_history_copy_message_order_repair_reindexes_every_row() {
    let Some(source) = history_copy() else {
        return;
    };
    let branch = std::env::var("PACT_HISTORY_BRANCH").unwrap_or_else(|_| "main".to_string());
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("history.pact");
    std::fs::copy(&source, &path).expect("copy history");
    let original = std::fs::read(&path).expect("read copy");

    let before = pactree::file_format::file_message_order_report(&path).expect("order report");
    eprintln!("order report before: {before}");
    let mut writer =
        pactree::PactAppendWriter::open(&path, &branch, pactree::PactMemoryOptions::default())
            .expect("writer");
    writer.set_automatic_maintenance(false);
    let repair = writer.repair_message_order().expect("repair");
    eprintln!("repair: {repair:?}");
    writer.close();

    let after = pactree::file_format::file_message_order_report(&path).expect("order report");
    eprintln!("order report after: {after}");
    assert_eq!(after["head_index_matches"]["neither"], 0);
    assert_eq!(after["carried_rows"], before["carried_rows"]);
    assert_eq!(after["message_rows"], before["message_rows"]);
    let repaired = std::fs::read(&path).expect("read repaired");
    const HEADER_BYTES: usize = 4096;
    assert!(repaired.len() >= original.len());
    assert_eq!(
        &repaired[HEADER_BYTES..original.len()],
        &original[HEADER_BYTES..],
        "the repair only appends"
    );
    let handle = pactree::PactFileReadHandle::open(&path, None).expect("indexed reopen");
    let page = handle
        .stream("message_surface", None, None, "before", 20)
        .expect("stream");
    let rows = page["rows"].as_array().expect("rows");
    assert!(!rows.is_empty());
    eprintln!(
        "newest rows: {:?}",
        rows.iter()
            .map(|row| row["component_id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
    );
    pactree::file_format::file_validate_json(&path).expect("validate");
    if let Some(out) = std::env::var_os("PACT_ACCEPTANCE_OUT") {
        std::fs::copy(&path, out).expect("keep repaired copy");
    }
}
