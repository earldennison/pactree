use super::layout::{
    BINARY_BYTES_SENTINEL, FILE_FOOTER_LEN, FILE_FOOTER_MAGIC, FILE_FOOTER_VERSION,
    PACT_RECORD_ZSTD_DICT_V1, PAGE_DICT_MAGIC, PageKind, RECORD_HEADER_LEN, RecordKind,
    encode_page_kind,
};
use super::record_codec::{compact_canonical_record_payload, expand_canonical_record_payload};
use super::util::{
    base64_decode, read_u16, read_u32, read_u64, stable_checksum, write_u16, write_u32, write_u64,
};
use super::value_codec::{
    decode_pact_value_payload_with_symbols, decode_trace_value_payload_with_symbols_filtered,
    encode_pact_value_payload,
};
use super::{
    EncodedRecord, PactFileFooter, ScannedRecord, SymbolTable, compress_zstd_chunk,
    compress_zstd_with_dict,
};
use crate::error::{PactError, PactResult};
use serde::Serialize;
use serde_json::Value;

pub(super) fn write_footer(
    out: &mut Vec<u8>,
    manifest_offset: u64,
    branch_head_offset: u64,
    hot_index_offset: u64,
    last_commit_offset: u64,
) {
    let mut fields = Vec::new();
    fields.extend_from_slice(FILE_FOOTER_MAGIC);
    write_u16(&mut fields, FILE_FOOTER_VERSION);
    write_u16(&mut fields, FILE_FOOTER_LEN as u16);
    write_u64(&mut fields, manifest_offset);
    write_u64(&mut fields, branch_head_offset);
    write_u64(&mut fields, hot_index_offset);
    write_u64(&mut fields, last_commit_offset);
    let checksum = footer_checksum(&fields);
    out.extend_from_slice(&fields);
    out.extend_from_slice(&checksum);
}

pub(super) fn latest_valid_footer(bytes: &[u8]) -> Option<PactFileFooter> {
    if bytes.len() < FILE_FOOTER_LEN {
        return None;
    }
    let mut offset = bytes.len() - FILE_FOOTER_LEN;
    loop {
        if bytes
            .get(offset..offset + FILE_FOOTER_MAGIC.len())
            .is_some_and(|magic| magic == FILE_FOOTER_MAGIC)
            && let Some(footer) = read_footer_at(bytes, offset)
        {
            return Some(footer);
        }
        if offset == 0 {
            break;
        }
        offset -= 1;
    }
    None
}

pub(super) fn read_footer_at(bytes: &[u8], offset: usize) -> Option<PactFileFooter> {
    let footer = bytes.get(offset..offset + FILE_FOOTER_LEN)?;
    if &footer[..4] != FILE_FOOTER_MAGIC {
        return None;
    }
    let version = read_u16(footer, 4)?;
    let footer_len = read_u16(footer, 6)?;
    if version != FILE_FOOTER_VERSION || footer_len as usize != FILE_FOOTER_LEN {
        return None;
    }
    let manifest_offset = read_u64(footer, 8)?;
    let branch_head_offset = read_u64(footer, 16)?;
    let hot_index_offset = read_u64(footer, 24)?;
    let last_commit_offset = read_u64(footer, 32)?;
    let expected = footer.get(40..72)?;
    let actual = footer_checksum(&footer[..40]);
    if expected != actual {
        return None;
    }
    Some(PactFileFooter {
        version,
        footer_len,
        manifest_offset,
        branch_head_offset,
        hot_index_offset,
        last_commit_offset,
        checksum: checksum_hex(&actual),
        offset: offset as u64,
    })
}

fn footer_checksum(fields: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    for index in 0..4 {
        let checksum = stable_checksum(&[b"pact.footer", &[index as u8], fields]);
        out[index * 8..(index + 1) * 8].copy_from_slice(&checksum.to_le_bytes());
    }
    out
}

fn checksum_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn encode_record_payload<T: Serialize>(
    kind: RecordKind,
    payload: &T,
) -> PactResult<Vec<u8>> {
    let mut value = serde_json::to_value(payload)
        .map_err(|err| PactError::codec(format!("failed to encode record payload value: {err}")))?;
    prepare_binary_payload_slots(kind, &mut value)?;
    let value = compact_canonical_record_payload(kind, value);
    encode_pact_value_payload(&value)
}

pub(super) fn prepare_binary_payload_slots(
    kind: RecordKind,
    payload: &mut Value,
) -> PactResult<()> {
    if kind != RecordKind::ContentChunk {
        return Ok(());
    }
    let Some(object) = payload.as_object_mut() else {
        return Ok(());
    };
    if object.get("encoding").and_then(Value::as_str) != Some("base64") {
        return Ok(());
    }
    let Some(text) = object.get("text").and_then(Value::as_str) else {
        return Ok(());
    };
    let bytes = base64_decode(text)?;
    object.insert("text".to_string(), binary_bytes_value(bytes));
    Ok(())
}

fn binary_bytes_value(bytes: Vec<u8>) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(
        BINARY_BYTES_SENTINEL.to_string(),
        Value::Array(
            bytes
                .into_iter()
                .map(|byte| Value::Number(u64::from(byte).into()))
                .collect(),
        ),
    );
    Value::Object(object)
}

pub(super) fn decode_record_payload_with_symbols(
    kind: &str,
    payload: &[u8],
    symbols: &SymbolTable,
) -> PactResult<Value> {
    let value = decode_pact_value_payload_with_symbols(payload, symbols)?;
    expand_canonical_record_payload(kind, value)
}

pub(super) fn decode_trace_record_payload_filtered(
    payload: &[u8],
    symbols: &SymbolTable,
) -> PactResult<Value> {
    let value = decode_trace_value_payload_with_symbols_filtered(payload, symbols)?;
    expand_canonical_record_payload("trace", value)
}

pub(super) fn parse_page_records(payload: &[u8]) -> PactResult<Vec<ScannedRecord>> {
    parse_page_records_with_symbols(payload, &SymbolTable::default())
}

pub(super) fn parse_page_records_with_symbols(
    payload: &[u8],
    symbols: &SymbolTable,
) -> PactResult<Vec<ScannedRecord>> {
    parse_page_records_with_symbols_filter(payload, symbols, |_| true)
}

pub(super) fn parse_page_records_with_symbols_filter(
    payload: &[u8],
    symbols: &SymbolTable,
    accept: impl Fn(&str) -> bool,
) -> PactResult<Vec<ScannedRecord>> {
    let mut records = Vec::new();
    let mut record_offset = 0;
    while record_offset < payload.len() {
        if payload.len() - record_offset < RECORD_HEADER_LEN {
            return Err(PactError::corrupt(format!(
                "truncated record header at page payload offset {record_offset}"
            )));
        }
        let raw_record_kind = read_u16(payload, record_offset).unwrap_or_default();
        record_offset += 2;
        let record_seq = u64::from(read_u32(payload, record_offset).unwrap_or_default());
        record_offset += 4;
        let record_payload_len = read_u64(payload, record_offset).unwrap_or_default() as usize;
        record_offset += 8;
        let expected_record_checksum = read_u64(payload, record_offset).unwrap_or_default();
        record_offset += 8;
        if payload.len() - record_offset < record_payload_len {
            return Err(PactError::corrupt(format!(
                "truncated record payload for record {record_seq}"
            )));
        }
        let record_payload = &payload[record_offset..record_offset + record_payload_len];
        record_offset += record_payload_len;
        let actual_record_checksum = stable_checksum(&[
            &raw_record_kind.to_le_bytes(),
            &(record_seq as u32).to_le_bytes(),
            &(record_payload_len as u64).to_le_bytes(),
            record_payload,
        ]);
        if actual_record_checksum != expected_record_checksum {
            return Err(PactError::corrupt(format!(
                "record checksum mismatch for record {record_seq}: expected {expected_record_checksum:016x}, got {actual_record_checksum:016x}"
            )));
        }
        let kind = RecordKind::from_u16(raw_record_kind)
            .map(RecordKind::as_str)
            .unwrap_or("unknown")
            .to_string();
        if !accept(&kind) {
            continue;
        }
        let payload =
            decode_record_payload_with_symbols(&kind, record_payload, symbols).map_err(|err| {
                PactError::corrupt(format!(
                    "invalid record payload for record {record_seq}: {err}"
                ))
            })?;
        records.push(ScannedRecord {
            kind,
            payload,
            payload_bytes: record_payload_len,
            sequence: record_seq,
        });
    }
    Ok(records)
}

pub(super) fn write_page(
    out: &mut Vec<u8>,
    page_kind: PageKind,
    page_seq: u64,
    records: &[&EncodedRecord],
) -> PactResult<()> {
    let mut payload = Vec::new();
    for record in records {
        write_record(&mut payload, record)?;
    }
    let (payload, compressed) = page_storage_payload(page_kind, payload);
    let raw_page_kind = encode_page_kind(page_kind, compressed);
    let payload_len = payload.len() as u64;
    let record_count = u32::try_from(records.len())
        .map_err(|_| PactError::codec("record count exceeds u32 on-disk field"))?;
    let checksum = stable_checksum(&[
        &raw_page_kind.to_le_bytes(),
        &page_seq.to_le_bytes(),
        &record_count.to_le_bytes(),
        &payload_len.to_le_bytes(),
        &payload,
    ]);

    out.extend_from_slice(b"PG");
    write_u16(out, raw_page_kind);
    write_u64(out, page_seq);
    write_u32(out, record_count);
    write_u64(out, payload_len);
    write_u64(out, checksum);
    out.extend_from_slice(&payload);
    Ok(())
}

pub(super) fn page_storage_payload(page_kind: PageKind, payload: Vec<u8>) -> (Vec<u8>, bool) {
    if !page_kind_should_compress(page_kind) {
        return (payload, false);
    }
    if page_kind == PageKind::Record
        && let Ok(compressed) = compress_zstd_with_dict(&payload, PACT_RECORD_ZSTD_DICT_V1)
    {
        let mut framed = Vec::with_capacity(PAGE_DICT_MAGIC.len() + compressed.len());
        framed.extend_from_slice(PAGE_DICT_MAGIC);
        framed.extend_from_slice(&compressed);
        if framed.len() + 16 < payload.len() {
            return (framed, true);
        }
    }
    let Ok(compressed) = compress_zstd_chunk(&payload) else {
        return (payload, false);
    };
    if compressed.len() + 16 < payload.len() {
        (compressed, true)
    } else {
        (payload, false)
    }
}

fn page_kind_should_compress(page_kind: PageKind) -> bool {
    matches!(
        page_kind,
        PageKind::Symbol | PageKind::Content | PageKind::Record | PageKind::Index
    )
}

fn write_record(out: &mut Vec<u8>, record: &EncodedRecord) -> PactResult<()> {
    let payload_len = record.payload.len() as u64;
    let record_seq = u32::try_from(record.seq)
        .map_err(|_| PactError::codec("record sequence exceeds u32 on-disk field"))?;
    let checksum = stable_checksum(&[
        &(record.kind as u16).to_le_bytes(),
        &record_seq.to_le_bytes(),
        &payload_len.to_le_bytes(),
        &record.payload,
    ]);
    write_u16(out, record.kind as u16);
    write_u32(out, record_seq);
    write_u64(out, payload_len);
    write_u64(out, checksum);
    out.extend_from_slice(&record.payload);
    Ok(())
}
