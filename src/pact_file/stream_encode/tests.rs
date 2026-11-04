use std::collections::BTreeMap;
use std::io::{Read, Write};

use super::{RecordSpool, SpoolReader, sections_snapshot, spool_standalone_core};
use crate::pact_file::codec::{RECORD_HEADER_BYTES, RecordKind, push_record_with_flags};
use crate::pact_file::compact_codec::{decode_standalone_core, encode_standalone_core};
use crate::pact_file::scratch::{ScratchFile, SpoolSpan};
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::stream_codec::TraceSpool;
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

#[test]
fn snapshot_readers_are_independent_bounded_and_do_not_disturb_appends() {
    let mut spool = ScratchFile::create(None, 4).expect("scratch");
    spool.append(b"abcdefghijklmnop").expect("append");
    spool.flush().expect("flush");
    let mut first = SpoolReader {
        spool: sections_snapshot(&spool).expect("first snapshot"),
        span: SpoolSpan { offset: 2, len: 5 },
        position: 0,
    };
    let mut second = SpoolReader {
        spool: sections_snapshot(&spool).expect("second snapshot"),
        span: SpoolSpan { offset: 10, len: 3 },
        position: 0,
    };
    assert_eq!(first.read(&mut []).expect("empty read"), 0);
    let mut prefix = [0; 2];
    first.read_exact(&mut prefix).expect("prefix");
    assert_eq!(&prefix, b"cd");
    let mut other = Vec::new();
    second.read_to_end(&mut other).expect("second span");
    assert_eq!(other, b"klm");
    spool.append(b"qrst").expect("append during snapshot reads");
    let mut suffix = Vec::new();
    first.read_to_end(&mut suffix).expect("first remainder");
    assert_eq!(suffix, b"efg");
    assert_eq!(first.read(&mut prefix).expect("span EOF"), 0);
    assert_eq!(second.read(&mut prefix).expect("span EOF"), 0);
    assert_eq!(
        std::fs::read(spool.path().expect("path")).expect("physical bytes"),
        b"abcdefghijklmnopqrst"
    );
}

#[test]
fn snapshot_reader_preserves_short_reads_and_physical_eof() {
    let mut spool = ScratchFile::create(None, 4).expect("scratch");
    spool.append(b"abcd").expect("append");
    let mut reader = SpoolReader {
        spool: sections_snapshot(&spool).expect("snapshot"),
        span: SpoolSpan { offset: 2, len: 5 },
        position: 0,
    };
    let mut bytes = [0; 8];
    assert_eq!(reader.read(&mut bytes).expect("short read"), 2);
    assert_eq!(&bytes[..2], b"cd");
    assert_eq!(reader.position, 2);
    assert_eq!(reader.read(&mut bytes).expect("physical EOF"), 0);
    assert_eq!(reader.position, 2);
}

#[test]
fn spooled_record_framing_equals_push_record_bytes() {
    let payload = b"payload bytes ".repeat(300);
    let mut expected = Vec::new();
    push_record_with_flags(&mut expected, RecordKind::Content, 9, 1, &payload).expect("eager");

    let mut records = RecordSpool::create(64).expect("spool");
    let mut writer = records.begin(RecordKind::Content, 9, 1);
    for chunk in payload.chunks(100) {
        writer.write_all(chunk).expect("write chunk");
    }
    let record = writer.finish().expect("finish");
    assert_eq!(record.encoded_len() as usize, expected.len());
    assert_eq!(&record.header()[..], &expected[..RECORD_HEADER_BYTES]);
    let mut out = Vec::new();
    records.emit(&record, &mut out).expect("emit");
    assert_eq!(out, expected);

    let pushed = records
        .push(RecordKind::IndexDelta, 9, 0, &7_u64.to_le_bytes())
        .expect("push");
    let mut eager = Vec::new();
    push_record_with_flags(
        &mut eager,
        RecordKind::IndexDelta,
        9,
        0,
        &7_u64.to_le_bytes(),
    )
    .unwrap();
    let mut out = Vec::new();
    records.emit(&pushed, &mut out).expect("emit");
    assert_eq!(out, eager);
}

#[test]
fn oversized_record_payloads_are_refused_at_finish() {
    let mut records = RecordSpool::create(1 << 20).expect("spool");
    let mut writer = records.begin(RecordKind::ContentBody, 1, 0);
    let chunk = vec![0_u8; 8 * 1024 * 1024];
    for _ in 0..9 {
        writer.write_all(&chunk).expect("write");
    }
    assert!(
        writer.finish().is_err(),
        "72 MiB exceeds the 64 MiB record limit"
    );
}

fn transaction(ops: Vec<TraceOp>) -> PactTransaction {
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(5),
        transaction_id: Some("tx_5".to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransaction::new(
        "tx_5".to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: 4,
            to_rev: 5,
            ops,
        },
        PactTransactionTrace {
            schema: TRANSACTION_TRACE_SCHEMA.to_string(),
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            transaction_id: "tx_5".to_string(),
            authored_ref: pact_ref.clone(),
            pre_request_ref: pact_ref.clone(),
            post_request_alteration_ref: pact_ref,
            request_ref: None,
            response_ref: None,
            post_response_alteration_ref: None,
            post_response_ref: None,
            alteration_ids: Vec::new(),
            commit: None,
            replay_trace: None,
            aux: BTreeMap::from([("note".to_string(), serde_json::json!("x".repeat(4096)))]),
        },
        Vec::new(),
    )
}

#[test]
fn spooled_standalone_core_decodes_to_the_same_transaction_as_the_eager_core() {
    let ops = vec![
        TraceOp::new(
            "m1",
            TraceOpKind::InsertComponent {
                parent_id: "root".into(),
                index: 0,
                component: serde_json::json!({"componentType": "message_turn", "role": "user"}),
            },
        ),
        TraceOp::new(
            "old",
            TraceOpKind::SetField {
                key: "pact_depth".into(),
                value: serde_json::json!(2),
            },
        ),
    ];
    let transaction = transaction(ops.clone());
    let eager = encode_standalone_core(&transaction).expect("eager core");

    let mut trace = TraceSpool::new(4, 5, 4096, 1 << 16).expect("trace spool");
    for op in &ops {
        trace.push_op(op).expect("push");
    }
    let mut records = RecordSpool::create(4096).expect("records");
    let record = spool_standalone_core(&transaction, &trace, &mut records, 3, 4096).expect("core");
    assert_eq!(record.kind, RecordKind::CompactTransaction);
    let mut framed = Vec::new();
    records.emit(&record, &mut framed).expect("emit");
    let streamed = &framed[RECORD_HEADER_BYTES..];

    let trace_bytes = crate::transaction_trace::encode_transaction_trace(&transaction.trace);
    let pctx = |bytes: &[u8]| bytes.windows(4).position(|w| w == b"PCTX").expect("PCTX");
    let (eager_pctx, streamed_pctx) = (pctx(&eager), pctx(streamed));
    assert_eq!(
        &streamed[..streamed_pctx - 2],
        &eager[..eager_pctx - 2],
        "PCCS symbol prefix"
    );
    let trace_at = eager[eager_pctx..]
        .windows(trace_bytes.len())
        .position(|w| w == trace_bytes)
        .expect("trace inside eager core");
    let core_prefix = trace_at + trace_bytes.len();
    assert_eq!(
        &streamed[streamed_pctx..streamed_pctx + core_prefix],
        &eager[eager_pctx..eager_pctx + core_prefix]
    );

    let decoded = decode_standalone_core(streamed).expect("decode streamed");
    let expected = decode_standalone_core(&eager).expect("decode eager");
    assert_eq!(decoded, expected);
    assert_eq!(decoded.trace.ops.len(), transaction.trace.ops.len());
    assert_eq!(decoded.public_trace.aux, transaction.public_trace.aux);
}
