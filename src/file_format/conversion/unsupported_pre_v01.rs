use super::*;

const REQUIRED_KINDS: &[&str] = &[
    "symbol",
    "meta",
    "content",
    "content_chunk",
    "root",
    "branch",
    "node",
    "trace",
    "commit",
    "head",
];
const LEGACY_MAGIC: &[u8; 8] = b"PACTBIN1";

pub(super) fn read_pre_v01_records(path: &Path) -> PactResult<Vec<ScannedRecord>> {
    let bytes = fs::read(path).map_err(|error| {
        PactError::storage(format!("failed to read pre-v0.1 Pact file: {error}"))
    })?;
    if bytes.len() < FILE_HEADER_LEN || bytes.get(..8) != Some(LEGACY_MAGIC.as_slice()) {
        return Err(PactError::invalid("source is not a PACTBIN1 file"));
    }
    let header_len = read_u16(&bytes, 10).unwrap_or_default() as usize;
    if header_len != FILE_HEADER_LEN {
        return Err(PactError::corrupt(format!(
            "unsupported pre-v0.1 Pact header length: {header_len}"
        )));
    }

    let mut records = Vec::new();
    let mut symbols = SymbolTable::default();
    let mut offset = FILE_HEADER_LEN;
    while offset < bytes.len() {
        if bytes
            .get(offset..offset + FILE_FOOTER_MAGIC.len())
            .is_some_and(|magic| magic == FILE_FOOTER_MAGIC)
        {
            if page_codec::read_footer_at(&bytes, offset).is_none() {
                return Err(PactError::corrupt(format!(
                    "invalid pre-v0.1 Pact footer at offset {offset}"
                )));
            }
            offset += FILE_FOOTER_LEN;
            continue;
        }
        let page_start = offset;
        if bytes.get(offset..offset + 2) != Some(b"PG") {
            return Err(PactError::corrupt(format!(
                "invalid pre-v0.1 Pact page at offset {offset}"
            )));
        }
        let raw_page_kind = read_u16(&bytes, offset + 2)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 Pact page kind"))?;
        let page_sequence = read_u64(&bytes, offset + 4)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 Pact page sequence"))?;
        let record_count = read_u32(&bytes, offset + 12)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 Pact record count"))?
            as usize;
        let payload_len = read_u64(&bytes, offset + 16)
            .and_then(|len| usize::try_from(len).ok())
            .ok_or_else(|| PactError::corrupt("invalid pre-v0.1 Pact page length"))?;
        let expected_checksum = read_u64(&bytes, offset + 24)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 Pact page checksum"))?;
        offset += PAGE_HEADER_LEN;
        let payload = bytes
            .get(offset..offset.saturating_add(payload_len))
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 Pact page payload"))?;
        let actual_checksum = stable_checksum(&[
            &raw_page_kind.to_le_bytes(),
            &page_sequence.to_le_bytes(),
            &(record_count as u32).to_le_bytes(),
            &(payload_len as u64).to_le_bytes(),
            payload,
        ]);
        if actual_checksum != expected_checksum {
            return Err(PactError::corrupt(format!(
                "pre-v0.1 Pact page checksum mismatch at offset {page_start}"
            )));
        }
        let (_, compressed) = layout::decode_page_kind(raw_page_kind).ok_or_else(|| {
            PactError::corrupt(format!("unknown pre-v0.1 Pact page kind {raw_page_kind}"))
        })?;
        let decoded;
        let payload = if compressed {
            decoded = content::decompress_page_payload(payload)?;
            decoded.as_slice()
        } else {
            payload
        };
        decode_page_records(
            payload,
            record_count,
            page_sequence,
            &mut symbols,
            &mut records,
        )?;
        offset += payload_len;
    }
    Ok(records)
}

fn decode_page_records(
    payload: &[u8],
    record_count: usize,
    page_sequence: u64,
    symbols: &mut SymbolTable,
    records: &mut Vec<ScannedRecord>,
) -> PactResult<()> {
    let mut offset = 0;
    for _ in 0..record_count {
        let raw_kind = read_u16(payload, offset)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 record kind"))?;
        let sequence = read_u32(payload, offset + 2)
            .map(u64::from)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 record sequence"))?;
        let payload_len = read_u64(payload, offset + 6)
            .and_then(|len| usize::try_from(len).ok())
            .ok_or_else(|| PactError::corrupt("invalid pre-v0.1 record length"))?;
        let expected_checksum = read_u64(payload, offset + 14)
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 record checksum"))?;
        offset += RECORD_HEADER_LEN;
        let encoded = payload
            .get(offset..offset.saturating_add(payload_len))
            .ok_or_else(|| PactError::corrupt("truncated pre-v0.1 record payload"))?;
        offset += payload_len;
        let actual_checksum = stable_checksum(&[
            &raw_kind.to_le_bytes(),
            &(sequence as u32).to_le_bytes(),
            &(payload_len as u64).to_le_bytes(),
            encoded,
        ]);
        if actual_checksum != expected_checksum {
            return Err(PactError::corrupt(format!(
                "pre-v0.1 record {sequence} checksum mismatch on page {page_sequence}"
            )));
        }
        let kind = layout::RecordKind::from_u16(raw_kind)
            .map(layout::RecordKind::as_str)
            .unwrap_or("unknown");
        if !REQUIRED_KINDS.contains(&kind) {
            continue;
        }
        let decoded = if kind == "trace" {
            page_codec::decode_trace_record_payload_filtered(encoded, symbols)?
        } else {
            page_codec::decode_record_payload_with_symbols(kind, encoded, symbols)?
        };
        records.push(ScannedRecord {
            kind: kind.to_string(),
            payload: decoded,
            payload_bytes: payload_len,
            sequence,
        });
        if kind == "symbol" {
            *symbols = SymbolTable::from_symbol_records(records);
        }
    }
    if offset != payload.len() {
        return Err(PactError::corrupt(format!(
            "pre-v0.1 page {page_sequence} has trailing record bytes"
        )));
    }
    Ok(())
}
