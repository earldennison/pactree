use super::{EnvelopeKind, RootSlot, classify, encode_header};
use crate::PactErrorKind;
use crate::pact_file::layout::{
    FILE_MAGIC, FILE_VERSION, HEADER_BYTES, ROOT_SLOT_BYTES, ROOT_SLOT_COUNT, ROOT_SLOT_START,
};

const PUBLISHED_001: &[u8] =
    include_bytes!("../../../tests/fixtures/unsupported/published-0.0.1-minimal.pact");
const CURRENT_PRE_V01: &[u8] =
    include_bytes!("../../../tests/fixtures/unsupported/current-pre-v0.1-minimal.pact");

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn write_slot(header: &mut [u8], slot: usize, generation: u64, watermark: u64) {
    let start = ROOT_SLOT_START + slot * ROOT_SLOT_BYTES;
    write_u64(header, start, generation);
    write_u64(header, start + 8, HEADER_BYTES as u64);
    write_u64(
        header,
        start + 16,
        crate::pact_file::codec::pack_index_page_address(HEADER_BYTES as u64, 0)
            .expect("pack fixture page address"),
    );
    write_u64(header, start + 24, watermark);
    write_u64(header, start + 32, HEADER_BYTES as u64);
    write_u64(header, start + 40, 48);
    write_u64(header, start + 48, HEADER_BYTES as u64 + 48);
    write_u64(header, start + 56, 48);
    header[start + 64..start + 80].fill(1);
    header[start + 80..start + 96].fill(2);
    let digest = blake3::hash(&header[start..start + 96]);
    header[start + 96..start + 128].copy_from_slice(digest.as_bytes());
}

fn valid_header() -> Vec<u8> {
    let mut header = vec![0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(FILE_MAGIC);
    write_u16(&mut header, 8, FILE_VERSION);
    write_u16(&mut header, 10, HEADER_BYTES as u16);
    header[16..48].copy_from_slice(blake3::hash(b"pactree-v0.1-test-file").as_bytes());
    write_slot(&mut header, 0, 1, HEADER_BYTES as u64 + 1024);
    header
}

#[test]
fn unsupported_pre_v0_1_fixtures_return_typed_error_without_mutation() {
    for fixture in [PUBLISHED_001, CURRENT_PRE_V01] {
        let before = fixture.to_vec();
        let error = classify(fixture).expect_err("PACTBIN1 must be rejected");
        assert_eq!(error.kind(), PactErrorKind::Unsupported);
        assert_eq!(error.to_string(), "pre-v0.1 Pact files are unsupported");
        assert_eq!(fixture, before);
    }
}

#[test]
fn pact_file_envelope_accepts_one_valid_root_slot() {
    let header = valid_header();
    assert_eq!(classify(&header).expect("classify"), EnvelopeKind::V0_1);
}

#[test]
fn pact_file_envelope_writer_emits_pactbin2_only() {
    let identity = *blake3::hash(b"candidate-writer").as_bytes();
    let header = encode_header(
        identity,
        &[RootSlot {
            generation: 1,
            committed_transaction_address: HEADER_BYTES as u64,
            index_root_address: crate::pact_file::codec::pack_index_page_address(
                HEADER_BYTES as u64,
                0,
            )
            .expect("pack fixture page address"),
            file_length_watermark: HEADER_BYTES as u64 + 1024,
            segment_directory_address: HEADER_BYTES as u64,
            segment_directory_len: 48,
            allocator_address: HEADER_BYTES as u64 + 48,
            allocator_len: 48,
            segment_directory_digest: [1; 16],
            allocator_digest: [2; 16],
        }],
    )
    .expect("encode header");

    assert_eq!(&header[..8], b"PACTBIN2");
    assert_ne!(&header[..8], b"PACTBIN1");
    assert_eq!(classify(&header).expect("classify"), EnvelopeKind::V0_1);
}

#[test]
fn pact_file_envelope_reads_only_the_fixed_header() {
    let mut bytes = valid_header();
    bytes.resize(HEADER_BYTES + 8 * 1024 * 1024, 0xa5);
    assert_eq!(classify(&bytes).expect("classify"), EnvelopeKind::V0_1);
}

#[test]
fn pact_file_envelope_rejects_empty_truncated_and_unknown_inputs() {
    for bytes in [Vec::new(), b"PACT".to_vec(), b"NOTPACT!".to_vec()] {
        let error = classify(&bytes).expect_err("invalid envelope");
        assert_eq!(error.kind(), PactErrorKind::CorruptFile);
    }
}

#[test]
fn public_open_classifies_a_short_unknown_file_as_corrupt() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("unknown.pact");
    std::fs::write(&path, [b"PACT1\0\0\0".as_slice(), &[0xff; 40]].concat())
        .expect("write unknown file");

    let error = crate::pact_file::open_ledger(&path).expect_err("unknown file must fail");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn pact_file_envelope_rejects_non_v2_versions() {
    for version in [0_u16, 1, 3] {
        let mut header = valid_header();
        write_u16(&mut header, 8, version);
        let error = classify(&header).expect_err("unsupported version");
        assert_eq!(error.kind(), PactErrorKind::Unsupported);
    }
}

#[test]
fn pact_file_envelope_rejects_corrupt_root_slot_checksum() {
    let mut header = valid_header();
    header[ROOT_SLOT_START + 32] ^= 0xff;
    let error = classify(&header).expect_err("corrupt slot");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn pact_file_envelope_rejects_absurd_watermark() {
    let mut header = valid_header();
    write_slot(&mut header, 0, 1, u64::MAX);
    let error = classify(&header).expect_err("absurd watermark");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn pact_file_envelope_rejects_two_invalid_slots() {
    let mut header = valid_header();
    for slot in 0..ROOT_SLOT_COUNT {
        write_slot(&mut header, slot, slot as u64 + 1, HEADER_BYTES as u64);
        let start = ROOT_SLOT_START + slot * ROOT_SLOT_BYTES;
        header[start + 40] ^= 0xff;
    }
    let error = classify(&header).expect_err("both slots invalid");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}
