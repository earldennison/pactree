use serde_json::json;

use super::{MAX_TRACE_BYTES, TraceSpool};
use crate::transaction_trace::{
    TraceOp, TraceOpKind, TransactionTrace, decode_transaction_trace, encode_transaction_trace,
};

fn corpus() -> Vec<TransactionTrace> {
    let insert = |id: &str, component: serde_json::Value| {
        TraceOp::new(
            id,
            TraceOpKind::InsertComponent {
                parent_id: "root".into(),
                index: 0,
                component,
            },
        )
    };
    vec![
        TransactionTrace {
            from_rev: 0,
            to_rev: 1,
            ops: Vec::new(),
        },
        TransactionTrace {
            from_rev: 3,
            to_rev: 4,
            ops: vec![
                insert(
                    "m1",
                    json!({
                        "componentType": "message_turn",
                        "role": "user",
                        "attrs": {"zeta": 1, "alpha": "dup"},
                        "children": [],
                        "offset": 2,
                        "tags": ["x", "y"],
                        "id": "ignored"
                    }),
                ),
                insert(
                    "t1",
                    json!({"componentType": "text_content", "content": "dup", "attrs": "not an object"}),
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::SetField {
                        key: "pact_depth".into(),
                        value: json!(1),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::SetField {
                        key: "custom".into(),
                        value: json!({"a": [1, 2, {"b": null}]}),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::UnsetField {
                        key: "custom".into(),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::SetAttr {
                        key: "alpha".into(),
                        value: json!("dup"),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::UnsetAttr {
                        key: "alpha".into(),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::MoveComponent {
                        parent_id: "root".into(),
                        index: 5,
                    },
                ),
                TraceOp::new(
                    "t1",
                    TraceOpKind::SetContent {
                        content: Some("héllo wörld".into()),
                        content_ref: Some(json!({"id": "content_1"})),
                        content_preview: Some("hé".into()),
                    },
                ),
                TraceOp::new(
                    "t1",
                    TraceOpKind::SetContent {
                        content: None,
                        content_ref: None,
                        content_preview: None,
                    },
                ),
                TraceOp::new(
                    "t1",
                    TraceOpKind::ReplaceContentRange {
                        start: 0,
                        end: 2,
                        replacement: "HE".into(),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::SetLifecycle {
                        value: json!({"ttl": 3, "cad": null}),
                    },
                ),
                TraceOp::new("gone", TraceOpKind::DeleteComponent),
                TraceOp::new(
                    "root",
                    TraceOpKind::SetField {
                        key: "child_ids".into(),
                        value: json!(["m1", "t1"]),
                    },
                ),
                TraceOp::new(
                    "m1",
                    TraceOpKind::SetField {
                        key: "\"dup\"".into(),
                        value: json!("dup"),
                    },
                ),
            ],
        },
    ]
}

fn spool(trace: &TransactionTrace) -> TraceSpool {
    let mut spool = TraceSpool::new(trace.from_rev, trace.to_rev, 4096, 1 << 16).expect("spool");
    for op in &trace.ops {
        spool.push_op(op).expect("push op");
    }
    spool
}

#[test]
fn spooled_trace_bytes_equal_the_eager_encoder_exactly() {
    for trace in corpus() {
        let eager = encode_transaction_trace(&trace);
        let spooled = spool(&trace);
        assert_eq!(spooled.encoded_len().expect("len"), eager.len() as u64);
        let mut out = Vec::new();
        spooled.emit(&mut out).expect("emit");
        assert_eq!(out, eager, "ops={}", trace.ops.len());

        assert_eq!(
            decode_transaction_trace(&out).expect("decode streamed"),
            decode_transaction_trace(&eager).expect("decode eager")
        );
        assert_eq!(spooled.op_count(), trace.ops.len() as u64);
    }
}

#[test]
fn huge_values_stay_on_scratch_and_still_match_the_eager_bytes() {
    let huge = "v".repeat(3 * 1024 * 1024);
    let trace = TransactionTrace {
        from_rev: 8,
        to_rev: 9,
        ops: vec![
            TraceOp::new(
                "big",
                TraceOpKind::SetField {
                    key: "content".into(),
                    value: json!(huge),
                },
            ),
            TraceOp::new(
                "big",
                TraceOpKind::SetField {
                    key: "content".into(),
                    value: json!(huge),
                },
            ),
        ],
    };
    let spooled = spool(&trace);
    assert!(
        spooled.scratch_bytes() >= 3 * 1024 * 1024,
        "the value lives on scratch"
    );
    assert_eq!(
        spooled.symbol_count(),
        crate::transaction_trace::encode::WELL_KNOWN.len() as u64 + 2,
        "id + one deduplicated value"
    );
    let mut out = Vec::new();
    spooled.emit(&mut out).expect("emit");
    assert_eq!(out, encode_transaction_trace(&trace));
}

#[test]
fn traces_over_the_fixed_limit_are_refused_before_any_byte_is_emitted() {
    let mut spooled = TraceSpool::new(1, 2, 1 << 20, 1 << 16).expect("spool");
    let chunk = "w".repeat(16 * 1024 * 1024);
    for n in 0..5 {
        let value = json!(format!("{n}{chunk}"));
        spooled
            .push_op(&TraceOp::new(
                "big",
                TraceOpKind::SetField {
                    key: "content".into(),
                    value,
                },
            ))
            .expect("push");
    }
    assert!(spooled.encoded_len().expect("len") > MAX_TRACE_BYTES);
    let mut out = Vec::new();
    let error = spooled.emit(&mut out).expect_err("over the limit");
    assert!(error.to_string().contains("exceeds fixed limit"), "{error}");
    assert!(out.is_empty(), "nothing was emitted");
}
