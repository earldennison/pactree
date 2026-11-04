use super::{
    PttrStream, ScannedField, ShallowScan, StreamOpKind, SymbolLocation, decode_zstd_to_spool,
};
use crate::pact_file::scratch::{ScratchFile, ScratchPages};
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace, encode_transaction_trace};

#[test]
fn zstd_streams_into_the_spool_and_rejects_bad_lengths_or_bytes() {
    let raw = b"pact ".repeat(200_000);
    let compressed = zstd::bulk::compress(&raw, 3).expect("compress");
    let mut spool = ScratchFile::create(None, 4096).expect("scratch");
    let span = decode_zstd_to_spool(&compressed, raw.len() as u64, &mut spool).expect("decode");
    assert_eq!(span.len, raw.len() as u64);
    assert!(spool.span_eq(span, &raw).expect("eq"));

    let wrong_len = decode_zstd_to_spool(&compressed, raw.len() as u64 - 1, &mut spool);
    assert!(
        wrong_len.is_err(),
        "a short advertised length is an overrun"
    );
    let too_long = decode_zstd_to_spool(&compressed, raw.len() as u64 + 1, &mut spool);
    assert!(too_long.is_err(), "a long advertised length is a mismatch");
    let truncated = decode_zstd_to_spool(
        &compressed[..compressed.len() / 2],
        raw.len() as u64,
        &mut spool,
    );
    assert!(
        truncated.is_err(),
        "a truncated stream fails without panicking"
    );
    let garbage = decode_zstd_to_spool(b"definitely not zstd", 5, &mut spool);
    assert!(garbage.is_err());
}

#[test]
fn shallow_scan_keeps_scalars_and_spans_large_or_nested_values() {
    let long = "x".repeat(5000);
    let object = serde_json::json!({
        "id": "c1",
        "componentType": "text_content",
        "offset": 3,
        "hidden": false,
        "content": long,
        "aux": {"note": "n", "tags": [1, 2, 3]},
        "child_ids": ["a", "b"],
        "nothing": null,
    });
    let bytes = serde_json::to_vec(&object).expect("encode");
    let mut spool = ScratchFile::create(None, 256).expect("scratch");
    let scan = ShallowScan::scan(&bytes, 1024, &mut spool).expect("scan");
    assert_eq!(
        scan.fields["id"],
        ScannedField::Scalar(serde_json::json!("c1"))
    );
    assert_eq!(
        scan.fields["offset"],
        ScannedField::Scalar(serde_json::json!(3))
    );
    assert_eq!(
        scan.fields["hidden"],
        ScannedField::Scalar(serde_json::json!(false))
    );
    assert_eq!(
        scan.fields["nothing"],
        ScannedField::Scalar(serde_json::Value::Null)
    );
    let ScannedField::Span(content) = scan.fields["content"] else {
        panic!("long content must be a span");
    };
    assert_eq!(
        spool.read_span(content).expect("read"),
        serde_json::to_vec(&serde_json::Value::String("x".repeat(5000))).expect("json string")
    );
    let ScannedField::Span(aux) = scan.fields["aux"] else {
        panic!("nested object must be a span");
    };
    let aux_value: serde_json::Value =
        serde_json::from_slice(&spool.read_span(aux).expect("read")).expect("nested json");
    assert_eq!(
        aux_value,
        serde_json::json!({"note": "n", "tags": [1, 2, 3]})
    );
    assert!(matches!(scan.fields["child_ids"], ScannedField::Span(_)));
    assert!(
        ShallowScan::scan(b"[1,2]", 1024, &mut spool).is_err(),
        "not an object"
    );
    assert!(
        ShallowScan::scan(b"{\"a\": }", 1024, &mut spool).is_err(),
        "invalid json"
    );
}

fn sample_trace(huge_symbol: Option<String>) -> TransactionTrace {
    let mut ops = vec![
        TraceOp::new(
            "m1",
            TraceOpKind::InsertComponent {
                parent_id: "root".into(),
                index: 2,
                component: serde_json::json!({
                    "componentType": "message_turn",
                    "role": "user",
                    "attrs": {"note": "hello"},
                    "children": []
                }),
            },
        ),
        TraceOp::new(
            "old",
            TraceOpKind::SetField {
                key: "pact_depth".into(),
                value: serde_json::json!(4),
            },
        ),
        TraceOp::new("old", TraceOpKind::UnsetField { key: "cad".into() }),
        TraceOp::new(
            "m1",
            TraceOpKind::SetAttr {
                key: "note".into(),
                value: serde_json::json!("x"),
            },
        ),
        TraceOp::new("m1", TraceOpKind::UnsetAttr { key: "note".into() }),
        TraceOp::new(
            "m1",
            TraceOpKind::MoveComponent {
                parent_id: "root".into(),
                index: 0,
            },
        ),
        TraceOp::new(
            "t1",
            TraceOpKind::SetContent {
                content: Some("body".into()),
                content_ref: None,
                content_preview: Some("bo".into()),
            },
        ),
        TraceOp::new(
            "t1",
            TraceOpKind::ReplaceContentRange {
                start: 1,
                end: 2,
                replacement: "Z".into(),
            },
        ),
        TraceOp::new(
            "m1",
            TraceOpKind::SetLifecycle {
                value: serde_json::json!({"ttl": 1}),
            },
        ),
        TraceOp::new("gone", TraceOpKind::DeleteComponent),
    ];
    if let Some(huge) = huge_symbol {
        ops.push(TraceOp::new(
            "big",
            TraceOpKind::SetField {
                key: "content".into(),
                value: serde_json::Value::String(huge),
            },
        ));
    }
    TransactionTrace {
        from_rev: 7,
        to_rev: 8,
        ops,
    }
}

#[test]
fn pttr_stream_matches_the_eager_decoder_op_by_op() {
    let trace = sample_trace(None);
    let bytes = encode_transaction_trace(&trace);
    let mut pages = ScratchPages::create(None, 4096, 1 << 16).expect("pages");
    let mut stream = PttrStream::open(&bytes, &mut pages).expect("open");
    assert_eq!((stream.from_rev, stream.to_rev), (7, 8));
    assert_eq!(stream.op_count(), trace.ops.len() as u64);

    let mut streamed = Vec::new();
    while let Some(op) = stream.next_op().expect("next op") {
        streamed.push(op);
    }
    assert_eq!(streamed.len(), trace.ops.len());

    for (expected, op) in trace.ops.iter().zip(&streamed) {
        assert_eq!(
            stream.symbol_text(&pages, op.component).expect("id"),
            expected.component_id
        );
    }
    let StreamOpKind::Insert {
        parent,
        index,
        has_children,
        fields,
        attrs,
    } = &streamed[0].kind
    else {
        panic!("first op is an insert");
    };
    assert_eq!(stream.symbol_text(&pages, *parent).expect("parent"), "root");
    assert_eq!(*index, 2);
    assert!(has_children);
    let field_names = fields
        .iter()
        .map(|(key, _)| stream.symbol_text(&pages, *key).expect("key"))
        .collect::<Vec<_>>();
    assert_eq!(field_names, vec!["componentType", "role"]);
    assert_eq!(
        stream.symbol_text(&pages, fields[0].1).expect("value"),
        "\"message_turn\""
    );
    assert_eq!(attrs.len(), 1);
    assert!(matches!(
        stream.symbol(&pages, 0).expect("well known"),
        SymbolLocation::WellKnown("id")
    ));
    assert!(matches!(streamed[9].kind, StreamOpKind::Delete));
    assert!(stream.next_op().expect("exhausted").is_none());
}

#[test]
fn an_enormous_symbol_is_a_span_and_absent_ordinals_are_errors() {
    let huge = "h".repeat(6 * 1024 * 1024);
    let trace = sample_trace(Some(huge.clone()));
    let bytes = encode_transaction_trace(&trace);
    let mut pages = ScratchPages::create(None, 8192, 1 << 16).expect("pages");
    let mut stream = PttrStream::open(&bytes, &mut pages).expect("open");
    let mut last = None;
    while let Some(op) = stream.next_op().expect("op") {
        last = Some(op);
    }
    let StreamOpKind::SetField { value, .. } = last.expect("last op").kind else {
        panic!("last op sets the huge content");
    };
    let SymbolLocation::Span(span) = stream.symbol(&pages, value).expect("symbol") else {
        panic!("huge value is spooled");
    };
    assert_eq!(
        span.len as usize,
        huge.len() + 2,
        "spooled as its JSON string text"
    );
    assert!(
        stream.symbol(&pages, stream.symbol_count()).is_err(),
        "absent ordinal"
    );
    assert!(stream.symbol(&pages, u64::MAX).is_err());
}

#[test]
fn damaged_trace_bytes_fail_without_panicking() {
    let bytes = encode_transaction_trace(&sample_trace(None));
    let mut pages = ScratchPages::create(None, 4096, 1 << 16).expect("pages");
    assert!(PttrStream::open(b"PTTX\x02", &mut pages).is_err());
    assert!(PttrStream::open(&bytes[..3], &mut pages).is_err());
    let mut wrong_version = bytes.clone();
    wrong_version[4] = 9;
    assert!(PttrStream::open(&wrong_version, &mut pages).is_err());

    let truncated = &bytes[..bytes.len() - 3];
    let mut stream = PttrStream::open(truncated, &mut pages).expect("header parses");
    let mut result = Ok(Some(()));
    while let Ok(Some(_)) = result {
        result = stream.next_op().map(|op| op.map(|_| ()));
    }
    assert!(result.is_err(), "truncated body errors when reached");
}
