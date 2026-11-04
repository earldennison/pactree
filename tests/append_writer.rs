use pactree::{
    Component, ContextTree, PactAppendBatch, PactAppendFailure, PactAppendMessage,
    PactAppendOutcome, PactAppendWriter, PactAttrs, PactMemoryOptions,
};
use serde_json::json;

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

fn recorded_messages(path: &std::path::Path) -> Vec<(String, String)> {
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
fn public_writer_records_batches_with_native_identities() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("recording.pact");
    let mut writer =
        PactAppendWriter::open(&path, "main", PactMemoryOptions::default()).expect("open");
    let opened = writer.version().clone();
    assert_eq!(opened.revision, 0);

    let first = PactAppendBatch {
        source: "lens".to_string(),
        batch_id: "batch-1".to_string(),
        expected: opened.clone(),
        previous_checkpoint: None,
        checkpoint: Some("seq-1".to_string()),
        messages: vec![
            message("row-1", "user", "How do I open a file?"),
            message("row-2", "assistant", "Use the read handle."),
        ],
    };
    let committed = writer.append(first.clone()).expect("first batch");
    let PactAppendOutcome::Committed(receipt) = &committed else {
        panic!("a new batch commits");
    };
    assert_eq!(receipt.version.revision, 1);
    assert_eq!(receipt.row_ids, vec!["row-1", "row-2"]);

    for round in 2..=4 {
        let batch = PactAppendBatch {
            source: "lens".to_string(),
            batch_id: format!("batch-{round}"),
            expected: writer.version().clone(),
            previous_checkpoint: Some(format!("seq-{}", round - 1)),
            checkpoint: Some(format!("seq-{round}")),
            messages: vec![message(
                &format!("row-{}", round + 10),
                "user",
                &format!("turn {round}"),
            )],
        };
        let outcome = writer.append(batch).expect("later batch");
        assert!(matches!(outcome, PactAppendOutcome::Committed(_)));
        assert_eq!(outcome.receipt().version.revision, round);
    }
    assert_eq!(
        writer.checkpoint("lens").expect("checkpoint").as_deref(),
        Some("seq-4")
    );

    let retry = writer.append(first).expect("retry");
    assert_eq!(retry, PactAppendOutcome::AlreadyCommitted(receipt.clone()));

    let stale = PactAppendBatch {
        source: "lens".to_string(),
        batch_id: "batch-9".to_string(),
        expected: opened,
        previous_checkpoint: Some("seq-4".to_string()),
        checkpoint: Some("seq-9".to_string()),
        messages: vec![message("row-9", "user", "late")],
    };
    match writer.append(stale) {
        Err(PactAppendFailure::Conflict { actual }) => assert_eq!(actual, *writer.version()),
        other => panic!("expected a conflict, got {other:?}"),
    }
    writer.close();

    let recorded = recorded_messages(&path);
    assert_eq!(recorded.len(), 5);
    assert_eq!(recorded[0].1, "How do I open a file?");
    assert_eq!(recorded[1].1, "Use the read handle.");
    assert_eq!(recorded[4].1, "turn 4");

    let reopened =
        PactAppendWriter::open(&path, "main", PactMemoryOptions::default()).expect("reopen");
    assert_eq!(reopened.version().revision, 4);
    assert_eq!(
        reopened
            .lookup("lens", "batch-3")
            .expect("lookup")
            .map(|receipt| receipt.row_ids),
        Some(vec!["row-13".to_string()])
    );
}
