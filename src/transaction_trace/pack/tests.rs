use super::super::TransactionTrace;
use super::*;
use serde_json::{Value, json};

fn root(children: Value) -> Value {
    json!({ "id": "root", "componentType": "root", "children": children })
}

fn tool_revision(rev: u64) -> TransactionTrace {
    let before = root(json!([{
        "id": format!("call_{rev}"), "componentType": "tool_call",
        "attrs": { "tool_name": "shell_execute", "status": "pending" },
        "content": format!("$ run step {rev}")
    }]));
    let after = root(json!([
        {
            "id": format!("call_{rev}"), "componentType": "tool_call",
            "attrs": { "tool_name": "shell_execute", "status": "ok" },
            "content": format!("$ run step {rev}")
        },
        {
            "id": format!("result_{rev}"), "componentType": "tool_result",
            "attrs": { "tool_name": "shell_execute", "success": true },
            "content": format!("output of step {rev} completed successfully")
        },
    ]));
    TransactionTrace::diff(rev, rev + 1, &before, &after)
}

#[test]
fn pack_round_trips() {
    let traces: Vec<_> = (0..8).map(tool_revision).collect();
    let pack = pack_transaction_traces(&traces);
    let restored = unpack_transaction_traces(&pack).expect("unpack");
    assert_eq!(traces, restored);
}

#[test]
fn unpack_rejects_truncated_pack() {
    let pack = pack_transaction_traces(&[tool_revision(0)]);
    assert!(unpack_transaction_traces(&pack[..pack.len() - 1]).is_err());
}

#[cfg(feature = "compression-zstd")]
#[test]
fn compressed_pack_round_trips() {
    let traces: Vec<_> = (0..16).map(tool_revision).collect();
    let pack = pack_transaction_traces(&traces);
    let compressed = compress_pack(&pack).expect("compress");
    let restored = decompress_pack(&compressed).expect("decompress");
    assert_eq!(restored, pack);
    assert_eq!(
        unpack_transaction_traces(&restored).expect("unpack"),
        traces
    );
}

#[cfg(feature = "compression-zstd")]
#[test]
fn compression_shrinks_repetitive_packs() {
    let traces: Vec<_> = (0..32).map(tool_revision).collect();
    let pack = pack_transaction_traces(&traces);
    let compressed = compress_pack(&pack).expect("compress");
    assert!(
        compressed.len() < pack.len(),
        "zstd should shrink the repetitive pack: {} -> {}",
        pack.len(),
        compressed.len()
    );
}

#[cfg(feature = "compression-zstd")]
#[test]
fn previous_pack_dictionary_beats_no_dictionary() {
    let first: Vec<_> = (0..16).map(tool_revision).collect();
    let second: Vec<_> = (16..32).map(tool_revision).collect();
    let first_pack = pack_transaction_traces(&first);
    let second_pack = pack_transaction_traces(&second);

    let no_dict = compress_pack(&second_pack).expect("compress");
    let with_dict = compress_pack_with_dict(&second_pack, &first_pack).expect("compress dict");

    let restored = decompress_pack_with_dict(&with_dict, &first_pack).expect("decompress dict");
    assert_eq!(restored, second_pack);
    assert_eq!(
        unpack_transaction_traces(&restored).expect("unpack"),
        second
    );

    assert!(
        with_dict.len() <= no_dict.len(),
        "previous-pack dictionary should not be worse: dict {} vs none {}",
        with_dict.len(),
        no_dict.len()
    );
}
