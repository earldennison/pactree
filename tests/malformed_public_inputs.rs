use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use pactree::file_format::{file_open_json, file_validate_json};
use pactree::pact_pack::PackedV1;
use pactree::selector::select_json;
use pactree::transaction_trace::{
    TransactionTrace, decode_transaction_trace, encode_transaction_trace, unpack_transaction_traces,
};
use pactree::{
    PactErrorKind, PactResult, component_from_json, component_prepare_for_context_json,
    component_project_json, normalize_pact_json, pact_canonical_component_json,
    transaction_trace_validate_json, validate_mutation_request_json,
};

fn no_panic<T>(label: &str, call: impl FnOnce() -> PactResult<T>) -> PactResult<T> {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| panic!("{label} panicked"))
}

fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn empty_packed_container() -> Vec<u8> {
    let mut body = b"PACTPKV1".to_vec();
    body.push(1);
    write_varint(&mut body, 4);
    body.extend_from_slice(b"root");
    write_varint(&mut body, 0);
    write_varint(&mut body, 0);
    body.extend_from_slice(blake3::hash(&[]).as_bytes());
    write_varint(&mut body, 0);
    write_varint(&mut body, 0);
    let checksum = blake3::hash(&body);
    body.extend_from_slice(checksum.as_bytes());
    body.extend_from_slice(b"PKV1FOOT");
    body
}

#[test]
fn malformed_json_public_boundaries_return_errors_without_panicking() {
    let malformed = ["", "{", "[1,", "\0", "{\"children\":["];

    for input in malformed {
        assert!(no_panic("normalize_pact_json", || normalize_pact_json(input, 4096)).is_err());
        assert!(no_panic("component_from_json", || component_from_json(input)).is_err());
        assert!(no_panic("component_project_json", || component_project_json(input)).is_err());
        assert!(
            no_panic("component_prepare_for_context_json", || {
                component_prepare_for_context_json(input, "{}")
            })
            .is_err()
        );
        assert!(
            no_panic("pact_canonical_component_json", || {
                pact_canonical_component_json("text_content", input)
            })
            .is_err()
        );
        assert!(no_panic("select_json", || select_json(input, "*")).is_err());
        assert!(
            no_panic("transaction_trace_validate_json", || {
                transaction_trace_validate_json(input)
            })
            .is_err()
        );
        assert!(
            no_panic("validate_mutation_request_json", || {
                validate_mutation_request_json(input)
            })
            .is_err()
        );
    }
}

#[test]
fn binary_trace_parsers_reject_prefixes_counts_and_flips_without_panicking() {
    let trace = TransactionTrace {
        from_rev: 0,
        to_rev: 1,
        ops: Vec::new(),
    };
    let valid = encode_transaction_trace(&trace);

    for end in 0..valid.len() {
        assert!(
            no_panic("decode_transaction_trace prefix", || {
                decode_transaction_trace(&valid[..end])
            })
            .is_err()
        );
    }

    let forged_huge_count = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(
        no_panic("unpack_transaction_traces huge count", || {
            unpack_transaction_traces(&forged_huge_count)
        })
        .is_err()
    );

    for index in 0..valid.len() {
        let mut corrupt = valid.clone();
        corrupt[index] ^= 0xa5;
        let _ = no_panic("decode_transaction_trace byte flip", || {
            decode_transaction_trace(&corrupt)
        });
    }
}

#[test]
fn removed_packed_container_surface_always_returns_unsupported_without_panicking() {
    let valid = empty_packed_container();
    assert_eq!(
        no_panic("PackedV1::read removed format", || PackedV1::read(&valid))
            .err()
            .expect("removed format must fail")
            .kind(),
        PactErrorKind::Unsupported
    );

    for end in 0..valid.len() {
        assert_eq!(
            no_panic("PackedV1::read prefix", || PackedV1::read(&valid[..end]))
                .err()
                .expect("removed prefix must fail")
                .kind(),
            PactErrorKind::Unsupported
        );
    }

    for index in (0..valid.len()).step_by(7) {
        let mut corrupt = valid.clone();
        corrupt[index] ^= 0x5a;
        assert_eq!(
            no_panic("PackedV1::read byte flip", || PackedV1::read(&corrupt))
                .err()
                .expect("removed corrupt bytes must fail")
                .kind(),
            PactErrorKind::Unsupported,
            "byte flip {index} returned the wrong error family"
        );
    }

    let forged_lengths = [0xff; 256];
    assert_eq!(
        no_panic("PackedV1::read forged lengths", || {
            PackedV1::read(&forged_lengths)
        })
        .err()
        .expect("removed forged bytes must fail")
        .kind(),
        PactErrorKind::Unsupported
    );
}

#[test]
fn file_readers_handle_truncation_and_corruption_without_panicking() {
    let fixture = include_bytes!("fixtures/unsupported/current-pre-v0.1-minimal.pact");
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("malformed.pact");

    let mut corpus = Vec::new();
    corpus.push(Vec::new());
    corpus.extend((1..fixture.len().min(128)).map(|end| fixture[..end].to_vec()));
    corpus.push([b"PACTBIN1".as_slice(), &[0xff; 64]].concat());
    for index in (0..fixture.len()).step_by(97).take(24) {
        let mut corrupt = fixture.to_vec();
        corrupt[index] ^= 0x3c;
        corpus.push(corrupt);
    }

    for bytes in corpus {
        fs::write(&path, bytes).expect("write malformed fixture");
        let _ = no_panic("file_validate_json", || file_validate_json(&path));
        let _ = no_panic("file_open_json", || file_open_json(&path, None, None, 4096));
    }
}
