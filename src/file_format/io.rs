use super::{
    FILE_FOOTER_LEN, FILE_HEADER_LEN, PAGE_HEADER_LEN, PactFileFooter, PactRecordAddress, PageKind,
    RECORD_HEADER_LEN, RecordKind, ScannedRecord, SymbolTable, decode_page_kind,
    decode_record_payload_with_symbols, decompress_page_payload, invalidate_scan_cache,
    parse_page_records, parse_page_records_with_symbols, parse_page_records_with_symbols_filter,
    read_footer_at, read_u16, read_u32, read_u64, stable_checksum,
};
use crate::error::{PactError, PactResult};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(super) fn append_bytes(path: &Path, appended: &[u8]) -> PactResult<()> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|err| {
            PactError::storage(format!("failed to open .pact file for append: {err}"))
        })?;
    file.write_all(appended)
        .map_err(|err| PactError::storage(format!("failed to append to .pact file: {err}")))?;
    file.flush()
        .map_err(|err| PactError::storage(format!("failed to flush .pact file: {err}")))?;
    file.sync_all()
        .map_err(|err| PactError::storage(format!("failed to sync .pact file: {err}")))?;
    invalidate_scan_cache(path);
    Ok(())
}

pub(super) fn read_footer_from_end(
    file: &mut fs::File,
    file_len: u64,
    bytes_read: &mut u64,
) -> PactResult<Option<PactFileFooter>> {
    if file_len < FILE_FOOTER_LEN as u64 {
        return Ok(None);
    }
    let mut footer_bytes = vec![0_u8; FILE_FOOTER_LEN];
    file.seek(SeekFrom::End(-(FILE_FOOTER_LEN as i64)))
        .map_err(|err| PactError::storage(format!("failed to seek .pact footer: {err}")))?;
    file.read_exact(&mut footer_bytes)
        .map_err(|err| PactError::storage(format!("failed to read .pact footer: {err}")))?;
    *bytes_read += FILE_FOOTER_LEN as u64;
    let mut footer = read_footer_at(&footer_bytes, 0);
    if let Some(footer) = &mut footer {
        footer.offset = file_len - FILE_FOOTER_LEN as u64;
    }
    Ok(footer)
}

pub(super) fn read_symbol_table_prefix_from_file(
    file: &mut fs::File,
    bytes_read: &mut u64,
) -> PactResult<SymbolTable> {
    file.seek(SeekFrom::Start(FILE_HEADER_LEN as u64))
        .map_err(|err| PactError::storage(format!("failed to seek .pact prefix: {err}")))?;
    let mut symbol_records = Vec::new();
    loop {
        let mut header = [0_u8; PAGE_HEADER_LEN];
        match file.read_exact(&mut header) {
            Ok(()) => *bytes_read += PAGE_HEADER_LEN as u64,
            Err(_) => break,
        }
        if &header[..2] != b"PG" {
            break;
        }
        let Some(raw_page_kind) = read_u16(&header, 2) else {
            break;
        };
        let Some((page_kind, page_compressed)) = decode_page_kind(raw_page_kind) else {
            break;
        };
        let payload_len = read_u64(&header, 16)
            .ok_or_else(|| PactError::corrupt("invalid page header"))?
            as usize;
        match page_kind {
            PageKind::Meta => {
                file.seek(SeekFrom::Current(payload_len as i64))
                    .map_err(|err| {
                        PactError::storage(format!("failed to skip .pact meta page: {err}"))
                    })?;
            }
            PageKind::Symbol => {
                let mut payload = vec![0_u8; payload_len];
                file.read_exact(&mut payload).map_err(|err| {
                    PactError::storage(format!("failed to read .pact symbol page: {err}"))
                })?;
                *bytes_read += payload_len as u64;
                if page_compressed {
                    payload = decompress_page_payload(&payload)?;
                }
                symbol_records.extend(parse_page_records(&payload)?);
            }
            _ => break,
        }
    }
    Ok(SymbolTable::from_symbol_records(&symbol_records))
}

pub(super) fn read_page_records_from_file(
    file: &mut fs::File,
    offset: u64,
    bytes_read: &mut u64,
    symbols: &SymbolTable,
) -> PactResult<Vec<ScannedRecord>> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| PactError::storage(format!("failed to seek page at {offset}: {err}")))?;
    let mut header = [0_u8; PAGE_HEADER_LEN];
    file.read_exact(&mut header).map_err(|err| {
        PactError::storage(format!("failed to read page header at {offset}: {err}"))
    })?;
    *bytes_read += PAGE_HEADER_LEN as u64;
    let raw_page_kind =
        read_u16(&header, 2).ok_or_else(|| PactError::corrupt("invalid page header"))?;
    let page_compressed = decode_page_kind(raw_page_kind)
        .ok_or_else(|| PactError::corrupt(format!("unknown page kind {raw_page_kind}")))?
        .1;
    let payload_len =
        read_u64(&header, 16).ok_or_else(|| PactError::corrupt("invalid page header"))? as usize;
    let mut payload = vec![0_u8; payload_len];
    file.read_exact(&mut payload).map_err(|err| {
        PactError::storage(format!("failed to read page payload at {offset}: {err}"))
    })?;
    *bytes_read += payload_len as u64;
    if page_compressed {
        payload = decompress_page_payload(&payload)?;
    }
    parse_page_records_with_symbols(&payload, symbols)
}

pub(super) fn read_page_records_matching_from_file(
    file: &mut fs::File,
    offset: u64,
    bytes_read: &mut u64,
    symbols: &SymbolTable,
    wanted: &[RecordKind],
) -> PactResult<Vec<ScannedRecord>> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| PactError::storage(format!("failed to seek page at {offset}: {err}")))?;
    let mut header = [0_u8; PAGE_HEADER_LEN];
    file.read_exact(&mut header).map_err(|err| {
        PactError::storage(format!("failed to read page header at {offset}: {err}"))
    })?;
    *bytes_read += PAGE_HEADER_LEN as u64;
    let raw_page_kind =
        read_u16(&header, 2).ok_or_else(|| PactError::corrupt("invalid page header"))?;
    let page_compressed = decode_page_kind(raw_page_kind)
        .ok_or_else(|| PactError::corrupt(format!("unknown page kind {raw_page_kind}")))?
        .1;
    let payload_len =
        read_u64(&header, 16).ok_or_else(|| PactError::corrupt("invalid page header"))? as usize;
    let mut payload = vec![0_u8; payload_len];
    file.read_exact(&mut payload).map_err(|err| {
        PactError::storage(format!("failed to read page payload at {offset}: {err}"))
    })?;
    *bytes_read += payload_len as u64;
    if page_compressed {
        payload = decompress_page_payload(&payload)?;
    }
    parse_page_records_with_symbols_filter(&payload, symbols, |kind| {
        wanted.iter().any(|wanted| wanted.as_str() == kind)
    })
}

pub(super) fn read_index_record_named_from_file(
    file: &mut fs::File,
    offset: u64,
    bytes_read: &mut u64,
    symbols: &SymbolTable,
    name: &str,
) -> PactResult<Option<ScannedRecord>> {
    file.seek(SeekFrom::Start(offset)).map_err(|err| {
        PactError::storage(format!("failed to seek index page at {offset}: {err}"))
    })?;
    let mut header = [0_u8; PAGE_HEADER_LEN];
    file.read_exact(&mut header).map_err(|err| {
        PactError::storage(format!(
            "failed to read index page header at {offset}: {err}"
        ))
    })?;
    *bytes_read += PAGE_HEADER_LEN as u64;
    let raw_page_kind =
        read_u16(&header, 2).ok_or_else(|| PactError::corrupt("invalid page header"))?;
    let (page_kind, page_compressed) = decode_page_kind(raw_page_kind)
        .ok_or_else(|| PactError::corrupt(format!("unknown page kind {raw_page_kind}")))?;
    if page_kind != PageKind::Index {
        return Ok(None);
    }
    let payload_len =
        read_u64(&header, 16).ok_or_else(|| PactError::corrupt("invalid page header"))? as usize;
    let mut payload = vec![0_u8; payload_len];
    file.read_exact(&mut payload).map_err(|err| {
        PactError::storage(format!(
            "failed to read index page payload at {offset}: {err}"
        ))
    })?;
    *bytes_read += payload_len as u64;
    if page_compressed {
        payload = decompress_page_payload(&payload)?;
    }

    let mut record_offset = 0_usize;
    while record_offset < payload.len() {
        if payload.len() - record_offset < RECORD_HEADER_LEN {
            return Err(PactError::corrupt(format!(
                "truncated record header at page payload offset {record_offset}"
            )));
        }
        let raw_record_kind = read_u16(&payload, record_offset).unwrap_or_default();
        record_offset += 2;
        let record_seq = u64::from(read_u32(&payload, record_offset).unwrap_or_default());
        record_offset += 4;
        let record_payload_len = read_u64(&payload, record_offset).unwrap_or_default() as usize;
        record_offset += 8;
        let expected_record_checksum = read_u64(&payload, record_offset).unwrap_or_default();
        record_offset += 8;
        if payload.len() - record_offset < record_payload_len {
            return Err(PactError::corrupt(format!(
                "truncated record payload for record {record_seq}"
            )));
        }
        let record_payload = &payload[record_offset..record_offset + record_payload_len];
        record_offset += record_payload_len;
        if raw_record_kind != RecordKind::Index as u16 {
            continue;
        }
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
        let payload =
            decode_record_payload_with_symbols(RecordKind::Index.as_str(), record_payload, symbols)
                .map_err(|err| {
                    PactError::corrupt(format!(
                        "invalid index record payload for record {record_seq}: {err}"
                    ))
                })?;
        if payload.get("name").and_then(serde_json::Value::as_str) != Some(name) {
            continue;
        }
        return Ok(Some(ScannedRecord {
            kind: RecordKind::Index.as_str().to_string(),
            payload,
            payload_bytes: record_payload_len,
            sequence: record_seq,
        }));
    }
    Ok(None)
}

pub(super) fn read_record_from_file(
    file: &mut fs::File,
    address: &PactRecordAddress,
    bytes_read: &mut u64,
    symbols: &SymbolTable,
) -> PactResult<Option<ScannedRecord>> {
    file.seek(SeekFrom::Start(address.page_offset))
        .map_err(|err| {
            PactError::storage(format!(
                "failed to seek page at {} for record {}: {err}",
                address.page_offset, address.record_seq
            ))
        })?;
    let mut header = [0_u8; PAGE_HEADER_LEN];
    file.read_exact(&mut header).map_err(|err| {
        PactError::storage(format!(
            "failed to read page header at {} for record {}: {err}",
            address.page_offset, address.record_seq
        ))
    })?;
    *bytes_read += PAGE_HEADER_LEN as u64;
    if &header[..2] != b"PG" {
        return Err(PactError::corrupt(format!(
            "invalid page header at {} for record {}",
            address.page_offset, address.record_seq
        )));
    }
    let raw_page_kind =
        read_u16(&header, 2).ok_or_else(|| PactError::corrupt("invalid page header"))?;
    let (_page_kind, page_compressed) = decode_page_kind(raw_page_kind)
        .ok_or_else(|| PactError::corrupt(format!("unknown page kind {raw_page_kind}")))?;
    let page_seq = read_u64(&header, 4).ok_or_else(|| PactError::corrupt("invalid page header"))?;
    if page_seq != address.page_seq {
        return Err(PactError::corrupt(format!(
            "page sequence mismatch for record {}: expected {}, got {}",
            address.record_seq, address.page_seq, page_seq
        )));
    }
    let payload_len =
        read_u64(&header, 16).ok_or_else(|| PactError::corrupt("invalid page header"))? as usize;
    let mut payload = vec![0_u8; payload_len];
    file.read_exact(&mut payload).map_err(|err| {
        PactError::storage(format!(
            "failed to read page payload at {} for record {}: {err}",
            address.page_offset, address.record_seq
        ))
    })?;
    *bytes_read += payload_len as u64;
    if page_compressed {
        payload = decompress_page_payload(&payload)?;
    }

    let mut record_offset = 0_usize;
    while record_offset < payload.len() {
        if payload.len() - record_offset < RECORD_HEADER_LEN {
            return Err(PactError::corrupt(format!(
                "truncated record header at page payload offset {record_offset}"
            )));
        }
        let raw_record_kind = read_u16(&payload, record_offset).unwrap_or_default();
        record_offset += 2;
        let record_seq = u64::from(read_u32(&payload, record_offset).unwrap_or_default());
        record_offset += 4;
        let record_payload_len = read_u64(&payload, record_offset).unwrap_or_default() as usize;
        record_offset += 8;
        let expected_record_checksum = read_u64(&payload, record_offset).unwrap_or_default();
        record_offset += 8;
        if payload.len() - record_offset < record_payload_len {
            return Err(PactError::corrupt(format!(
                "truncated record payload for record {record_seq}"
            )));
        }
        let record_payload = &payload[record_offset..record_offset + record_payload_len];
        record_offset += record_payload_len;
        if record_seq != address.record_seq {
            continue;
        }
        if raw_record_kind != address.record_kind {
            return Err(PactError::corrupt(format!(
                "record kind mismatch for record {record_seq}: expected {}, got {}",
                address.record_kind, raw_record_kind
            )));
        }
        if let Some(expected_len) = address.payload_len
            && expected_len as usize != record_payload_len
        {
            return Err(PactError::corrupt(format!(
                "record payload length mismatch for record {record_seq}: expected {}, got {}",
                expected_len, record_payload_len
            )));
        }
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
        let payload =
            decode_record_payload_with_symbols(&kind, record_payload, symbols).map_err(|err| {
                PactError::corrupt(format!(
                    "invalid record payload for record {record_seq}: {err}"
                ))
            })?;
        return Ok(Some(ScannedRecord {
            kind,
            payload,
            payload_bytes: record_payload_len,
            sequence: record_seq,
        }));
    }

    Ok(None)
}
