use std::io::Read;

use super::codec::{RecordKind, read_record};
use super::compact_codec::{
    MAGIC as CORE_MAGIC, MAX_SECTION_BYTES, MAX_SYMBOL_BYTES, MAX_SYMBOLS, MAX_TRACE_BYTES,
    STANDALONE_MAGIC, read_len, read_varint as read_core_varint,
};
use super::scratch::{ScratchFile, ScratchPages, SpoolSpan};
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

pub(crate) fn decode_zstd_to_spool(
    compressed: &[u8],
    advertised_len: u64,
    spool: &mut ScratchFile,
) -> PactResult<SpoolSpan> {
    let mut decoder = zstd::stream::read::Decoder::new(compressed)
        .map_err(|error| PactError::corrupt(format!("invalid Pact zstd stream: {error}")))?;
    let span = spool
        .append_from_reader(&mut decoder, advertised_len)
        .map_err(|error| match error {
            PactError::Io(io) => PactError::corrupt(format!("invalid Pact zstd stream: {io}")),
            other => other,
        })?;
    if span.len != advertised_len {
        return Err(PactError::corrupt(format!(
            "Pact zstd stream decoded {} bytes, header advertised {advertised_len}",
            span.len
        )));
    }
    Ok(span)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ScannedField {
    Scalar(serde_json::Value),

    Span(SpoolSpan),
}

#[derive(Debug, Default)]
pub(crate) struct ShallowScan {
    pub(crate) fields: std::collections::BTreeMap<String, ScannedField>,
}

impl ShallowScan {
    pub(crate) fn scan(
        bytes: &[u8],
        span_threshold: usize,
        spool: &mut ScratchFile,
    ) -> PactResult<Self> {
        let object: std::collections::BTreeMap<String, &serde_json::value::RawValue> =
            serde_json::from_slice(bytes).map_err(|error| {
                PactError::corrupt(format!("invalid Pact shallow object: {error}"))
            })?;
        let mut fields = std::collections::BTreeMap::new();
        for (key, raw) in object {
            let text = raw.get();
            let first = text.as_bytes().first().copied().unwrap_or(b' ');
            let field = match first {
                b'{' | b'[' => ScannedField::Span(spool.append(text.as_bytes())?),
                b'"' if text.len() >= span_threshold => {
                    ScannedField::Span(spool.append(text.as_bytes())?)
                }
                _ => ScannedField::Scalar(serde_json::from_str(text).map_err(|error| {
                    PactError::corrupt(format!("invalid Pact scalar field {key}: {error}"))
                })?),
            };
            fields.insert(key, field);
        }
        Ok(Self { fields })
    }
}

const PTTR_MAGIC: &[u8; 4] = b"PTTR";
const PTTR_VERSION: u8 = 2;

pub(crate) const WELL_KNOWN_SYMBOLS: &[&str] = &[
    "id",
    "componentType",
    "type",
    "content",
    "content_ref",
    "content_preview",
    "children",
    "attrs",
    "offset",
    "parent_id",
    "pact_depth",
    "depth",
    "role",
    "lifecycle",
    "tool_name",
    "tool_call_id",
    "success",
    "status",
    "result",
    "error_message",
    "ttl",
    "cad",
    "key",
    "tags",
    "root",
];

mod opcode {
    pub(super) const DELETE_COMPONENT: u8 = 1;
    pub(super) const MOVE_COMPONENT: u8 = 2;
    pub(super) const SET_FIELD: u8 = 3;
    pub(super) const UNSET_FIELD: u8 = 4;
    pub(super) const SET_ATTR: u8 = 5;
    pub(super) const UNSET_ATTR: u8 = 6;
    pub(super) const SET_CONTENT: u8 = 7;
    pub(super) const REPLACE_CONTENT_RANGE: u8 = 8;
    pub(super) const SET_LIFECYCLE: u8 = 9;
    pub(super) const INSERT_COMPONENT: u8 = 10;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamOpKind {
    Insert {
        parent: u64,
        index: u64,
        has_children: bool,
        fields: Vec<(u64, u64)>,
        attrs: Vec<(u64, u64)>,
    },
    Delete,
    Move {
        parent: u64,
        index: u64,
    },
    SetField {
        key: u64,
        value: u64,
    },
    UnsetField {
        key: u64,
    },
    SetAttr {
        key: u64,
        value: u64,
    },
    UnsetAttr {
        key: u64,
    },
    SetContent {
        content: Option<u64>,
        content_ref: Option<u64>,
        preview: Option<u64>,
    },
    ReplaceContentRange {
        start: u64,
        end: u64,
        replacement: u64,
    },
    SetLifecycle {
        value: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamOp {
    pub(crate) component: u64,
    pub(crate) kind: StreamOpKind,
}

pub(crate) struct PttrStream<'a> {
    data: &'a [u8],
    position: usize,
    pub(crate) from_rev: u64,
    pub(crate) to_rev: u64,
    symbol_count: u64,

    table: ScratchFile,
    op_count: u64,
    ops_read: u64,
}

const SYMBOL_ENTRY_BYTES: u64 = 16;

impl<'a> PttrStream<'a> {
    pub(crate) fn open(data: &'a [u8], pages: &mut ScratchPages) -> PactResult<Self> {
        if data.len() < 5 || &data[..4] != PTTR_MAGIC {
            return Err(PactError::codec("not a binary transaction trace"));
        }
        if data[4] != PTTR_VERSION {
            return Err(PactError::codec(format!(
                "unsupported trace version: {}",
                data[4]
            )));
        }
        let mut position = 5;
        let from_rev = read_varint(data, &mut position)?;
        let to_rev = read_varint(data, &mut position)?;
        let session_symbols = read_varint(data, &mut position)?;
        let mut table = ScratchFile::create(None, pages.spool().chunk_bytes())?;
        let mut symbol_count = WELL_KNOWN_SYMBOLS.len() as u64;
        for _ in 0..session_symbols {
            let len = usize::try_from(read_varint(data, &mut position)?)
                .map_err(|_| PactError::codec("symbol length exceeds platform limits"))?;
            let end = position
                .checked_add(len)
                .filter(|end| *end <= data.len())
                .ok_or_else(|| PactError::codec("symbol length overruns trace bytes"))?;
            std::str::from_utf8(&data[position..end])
                .map_err(|_| PactError::codec("symbol is not valid utf-8"))?;
            let span = pages
                .spool_mut()
                .append_from_reader(&mut &data[position..end], len as u64)?;
            let mut value = [0_u8; SYMBOL_ENTRY_BYTES as usize];
            value[..8].copy_from_slice(&span.offset.to_le_bytes());
            value[8..].copy_from_slice(&span.len.to_le_bytes());
            table.append(&value)?;
            symbol_count += 1;
            position = end;
        }
        let op_count = read_varint(data, &mut position)?;
        Ok(Self {
            data,
            position,
            from_rev,
            to_rev,
            symbol_count,
            table,
            op_count,
            ops_read: 0,
        })
    }

    pub(crate) fn op_count(&self) -> u64 {
        self.op_count
    }

    pub(crate) fn symbol_count(&self) -> u64 {
        self.symbol_count
    }

    pub(crate) fn symbol(&self, pages: &ScratchPages, ordinal: u64) -> PactResult<SymbolLocation> {
        if let Some(text) = usize::try_from(ordinal)
            .ok()
            .and_then(|index| WELL_KNOWN_SYMBOLS.get(index))
        {
            return Ok(SymbolLocation::WellKnown(text));
        }
        if ordinal >= self.symbol_count {
            return Err(PactError::codec(format!(
                "symbol index {ordinal} out of range"
            )));
        }
        let _ = pages;
        let entry = (ordinal - WELL_KNOWN_SYMBOLS.len() as u64)
            .checked_mul(SYMBOL_ENTRY_BYTES)
            .ok_or_else(|| PactError::codec("symbol table offset overflow"))?;
        let value = self.table.read_span(SpoolSpan {
            offset: entry,
            len: SYMBOL_ENTRY_BYTES,
        })?;
        let mut raw = [0_u8; 8];
        raw.copy_from_slice(&value[..8]);
        let offset = u64::from_le_bytes(raw);
        raw.copy_from_slice(&value[8..]);
        let len = u64::from_le_bytes(raw);
        Ok(SymbolLocation::Span(SpoolSpan { offset, len }))
    }

    pub(crate) fn symbol_text(&self, pages: &ScratchPages, ordinal: u64) -> PactResult<String> {
        match self.symbol(pages, ordinal)? {
            SymbolLocation::WellKnown(text) => Ok(text.to_string()),
            SymbolLocation::Span(span) => String::from_utf8(pages.spool().read_span(span)?)
                .map_err(|_| PactError::codec("symbol is not valid utf-8")),
        }
    }

    pub(crate) fn next_op(&mut self) -> PactResult<Option<StreamOp>> {
        if self.ops_read >= self.op_count {
            return Ok(None);
        }
        let data = self.data;
        let pos = &mut self.position;
        let op = *data
            .get(*pos)
            .ok_or_else(|| PactError::codec("unexpected end of trace body"))?;
        *pos += 1;
        let component = read_varint(data, pos)?;
        let kind = match op {
            opcode::INSERT_COMPONENT => {
                let parent = read_varint(data, pos)?;
                let index = read_varint(data, pos)?;
                let flags = *data
                    .get(*pos)
                    .ok_or_else(|| PactError::codec("unexpected end of insert_component flags"))?;
                *pos += 1;
                let field_count = read_varint(data, pos)?;
                let mut fields = Vec::new();
                for _ in 0..field_count {
                    fields.push((read_varint(data, pos)?, read_varint(data, pos)?));
                }
                let attr_count = read_varint(data, pos)?;
                let mut attrs = Vec::new();
                for _ in 0..attr_count {
                    attrs.push((read_varint(data, pos)?, read_varint(data, pos)?));
                }
                StreamOpKind::Insert {
                    parent,
                    index,
                    has_children: flags & 0b001 != 0,
                    fields,
                    attrs,
                }
            }
            opcode::DELETE_COMPONENT => StreamOpKind::Delete,
            opcode::MOVE_COMPONENT => StreamOpKind::Move {
                parent: read_varint(data, pos)?,
                index: read_varint(data, pos)?,
            },
            opcode::SET_FIELD => StreamOpKind::SetField {
                key: read_varint(data, pos)?,
                value: read_varint(data, pos)?,
            },
            opcode::UNSET_FIELD => StreamOpKind::UnsetField {
                key: read_varint(data, pos)?,
            },
            opcode::SET_ATTR => StreamOpKind::SetAttr {
                key: read_varint(data, pos)?,
                value: read_varint(data, pos)?,
            },
            opcode::UNSET_ATTR => StreamOpKind::UnsetAttr {
                key: read_varint(data, pos)?,
            },
            opcode::SET_CONTENT => {
                let flags = *data
                    .get(*pos)
                    .ok_or_else(|| PactError::codec("unexpected end of set_content flags"))?;
                *pos += 1;
                let content = if flags & 0b001 != 0 {
                    Some(read_varint(data, pos)?)
                } else {
                    None
                };
                let content_ref = if flags & 0b010 != 0 {
                    Some(read_varint(data, pos)?)
                } else {
                    None
                };
                let preview = if flags & 0b100 != 0 {
                    Some(read_varint(data, pos)?)
                } else {
                    None
                };
                StreamOpKind::SetContent {
                    content,
                    content_ref,
                    preview,
                }
            }
            opcode::REPLACE_CONTENT_RANGE => StreamOpKind::ReplaceContentRange {
                start: read_varint(data, pos)?,
                end: read_varint(data, pos)?,
                replacement: read_varint(data, pos)?,
            },
            opcode::SET_LIFECYCLE => StreamOpKind::SetLifecycle {
                value: read_varint(data, pos)?,
            },
            other => {
                return Err(PactError::codec(format!("unknown trace opcode {other}")));
            }
        };
        self.ops_read += 1;
        Ok(Some(StreamOp { component, kind }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolLocation {
    WellKnown(&'static str),
    Span(SpoolSpan),
}

fn read_varint(data: &[u8], pos: &mut usize) -> PactResult<u64> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    loop {
        let byte = *data
            .get(*pos)
            .ok_or_else(|| PactError::codec("unexpected end of varint"))?;
        *pos += 1;
        if shift >= 64 {
            return Err(PactError::codec("varint overflow"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

pub(crate) fn spool_reader(
    reader: &mut dyn Read,
    limit: u64,
    spool: &mut ScratchFile,
) -> PactResult<SpoolSpan> {
    spool.append_from_reader(reader, limit)
}

pub(crate) fn compact_core_sections(payload: &[u8]) -> PactResult<serde_json::Value> {
    let (flags, raw_len, stored) = compact_core_stored_sections(payload)?;
    let raw = if flags & 1 != 0 {
        zstd::bulk::decompress(stored, raw_len).map_err(|error| {
            PactError::corrupt(format!("invalid compact transaction sections: {error}"))
        })?
    } else {
        stored.to_vec()
    };
    if raw.len() != raw_len {
        return Err(PactError::corrupt(
            "compact transaction sections length mismatch",
        ));
    }
    serde_json::from_slice(&raw).map_err(|error| {
        PactError::corrupt(format!("invalid compact transaction sections: {error}"))
    })
}

pub(crate) fn compact_core_sections_reader(
    payload: &[u8],
) -> PactResult<Box<dyn std::io::Read + '_>> {
    let (flags, raw_len, stored) = compact_core_stored_sections(payload)?;
    if flags & 1 != 0 {
        let decoder = zstd::stream::read::Decoder::new(stored).map_err(|error| {
            PactError::corrupt(format!("invalid compact transaction sections: {error}"))
        })?;
        Ok(Box::new(std::io::Read::take(decoder, raw_len as u64)))
    } else {
        if stored.len() != raw_len {
            return Err(PactError::corrupt(
                "compact transaction sections length mismatch",
            ));
        }
        Ok(Box::new(stored))
    }
}

pub(crate) fn baseline_checkpoint_reader(
    file: &[u8],
    address: u64,
) -> PactResult<Option<(Box<dyn std::io::Read + '_>, bool)>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact transaction address exceeds platform limits"))?;
    loop {
        let record = read_record(file, &mut position)?;
        match record.kind {
            RecordKind::CompactTransaction => {
                return compact_core_sections_reader(record.payload)
                    .map(|reader| Some((reader, true)));
            }
            RecordKind::Transaction => {
                let payload = record.payload;
                let mut cursor = 0;
                super::codec::read_string(payload, &mut cursor)?;
                super::codec::read_string(payload, &mut cursor)?;
                let genesis_code = *payload
                    .get(cursor + 1)
                    .ok_or_else(|| PactError::corrupt("truncated Pact genesis marker"))?;
                cursor += 2;
                if genesis_code != 3 {
                    return Ok(None);
                }
                let raw_len: [u8; 8] = super::codec::read_bytes(payload, &mut cursor)?
                    .try_into()
                    .map_err(|_| {
                    PactError::corrupt("Pact head checkpoint length has invalid width")
                })?;
                let raw_len = u64::from_le_bytes(raw_len);
                let compressed = super::codec::read_bytes(payload, &mut cursor)?;
                let decoder = zstd::stream::read::Decoder::new(compressed).map_err(|error| {
                    PactError::corrupt(format!("invalid compressed Pact head checkpoint: {error}"))
                })?;
                return Ok(Some((
                    Box::new(std::io::Read::take(decoder, raw_len)),
                    false,
                )));
            }
            RecordKind::Commit => return Ok(None),
            _ => {}
        }
    }
}

fn compact_core_stored_sections(payload: &[u8]) -> PactResult<(u8, usize, &[u8])> {
    let mut cursor = 0;
    let take = |cursor: &mut usize, len: usize| -> PactResult<&[u8]> {
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| PactError::corrupt("truncated compact transaction"))?;
        let bytes = &payload[*cursor..end];
        *cursor = end;
        Ok(bytes)
    };
    if take(&mut cursor, 4)? != STANDALONE_MAGIC {
        return Err(PactError::corrupt("invalid compact standalone core magic"));
    }
    take(&mut cursor, 1)?;
    let symbols = read_len(payload, &mut cursor, MAX_SYMBOLS)?;
    for _ in 0..symbols {
        let len = read_len(payload, &mut cursor, MAX_SYMBOL_BYTES)?;
        take(&mut cursor, len)?;
    }
    let transaction_len = read_len(payload, &mut cursor, MAX_SECTION_BYTES)?;
    let core = take(&mut cursor, transaction_len)?;
    let mut inner = 0;
    let take_core = |cursor: &mut usize, len: usize| -> PactResult<&[u8]> {
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= core.len())
            .ok_or_else(|| PactError::corrupt("truncated compact transaction core"))?;
        let bytes = &core[*cursor..end];
        *cursor = end;
        Ok(bytes)
    };
    if take_core(&mut inner, 4)? != CORE_MAGIC {
        return Err(PactError::corrupt("invalid compact transaction core magic"));
    }
    take_core(&mut inner, 1)?;
    read_core_varint(core, &mut inner)?;
    read_core_varint(core, &mut inner)?;
    take_core(&mut inner, 1)?;
    let trace_len = read_len(core, &mut inner, MAX_TRACE_BYTES)?;
    take_core(&mut inner, trace_len)?;
    let flags = take_core(&mut inner, 1)?[0];
    let raw_len = read_len(core, &mut inner, MAX_SECTION_BYTES)?;
    let stored_len = read_len(core, &mut inner, MAX_SECTION_BYTES)?;
    let stored = take_core(&mut inner, stored_len)?;
    Ok((flags, raw_len, stored))
}

pub(crate) fn compact_core_trace(payload: &[u8]) -> PactResult<&[u8]> {
    let mut cursor = 0;
    let take = |cursor: &mut usize, len: usize| -> PactResult<&[u8]> {
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| PactError::corrupt("truncated compact transaction"))?;
        let bytes = &payload[*cursor..end];
        *cursor = end;
        Ok(bytes)
    };
    if take(&mut cursor, 4)? != STANDALONE_MAGIC {
        return Err(PactError::corrupt("invalid compact standalone core magic"));
    }
    take(&mut cursor, 1)?;
    let symbols = read_len(payload, &mut cursor, MAX_SYMBOLS)?;
    for _ in 0..symbols {
        let len = read_len(payload, &mut cursor, MAX_SYMBOL_BYTES)?;
        take(&mut cursor, len)?;
    }
    let transaction_len = read_len(payload, &mut cursor, MAX_SECTION_BYTES)?;
    let core = take(&mut cursor, transaction_len)?;
    let mut inner = 0;
    let take_core = |cursor: &mut usize, len: usize| -> PactResult<&[u8]> {
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= core.len())
            .ok_or_else(|| PactError::corrupt("truncated compact transaction core"))?;
        let bytes = &core[*cursor..end];
        *cursor = end;
        Ok(bytes)
    };
    if take_core(&mut inner, 4)? != CORE_MAGIC {
        return Err(PactError::corrupt("invalid compact transaction core magic"));
    }
    take_core(&mut inner, 1)?;
    read_core_varint(core, &mut inner)?;
    read_core_varint(core, &mut inner)?;
    take_core(&mut inner, 1)?;
    let trace_len = read_len(core, &mut inner, MAX_TRACE_BYTES)?;
    take_core(&mut inner, trace_len)
}

pub(crate) fn transaction_trace_bytes(
    file: &[u8],
    address: u64,
) -> PactResult<std::borrow::Cow<'_, [u8]>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact transaction address exceeds platform limits"))?;
    loop {
        let record = read_record(file, &mut position)?;
        match record.kind {
            RecordKind::CompactTransaction => {
                return compact_core_trace(record.payload).map(std::borrow::Cow::Borrowed);
            }
            RecordKind::Trace => {
                return super::codec::decompressed_payload(record.payload)
                    .map(std::borrow::Cow::Owned);
            }
            RecordKind::Commit => {
                return Err(PactError::corrupt(
                    "Pact transaction has no canonical trace",
                ));
            }
            _ => {}
        }
    }
}

pub(crate) fn transaction_ledger_delta(
    file: &[u8],
    address: u64,
) -> PactResult<Option<serde_json::Value>> {
    let mut position = usize::try_from(address)
        .map_err(|_| PactError::corrupt("Pact transaction address exceeds platform limits"))?;
    loop {
        let record = read_record(file, &mut position)?;
        match record.kind {
            RecordKind::CompactTransaction => {
                let sections = compact_core_sections(record.payload)?;
                return Ok(sections
                    .get("ledger_delta")
                    .filter(|value| !value.is_null())
                    .cloned());
            }
            RecordKind::LedgerDelta => {
                let payload = super::codec::decompressed_payload(record.payload)?;
                return serde_json::from_slice(&payload).map(Some).map_err(|error| {
                    PactError::corrupt(format!("invalid Pact ledger delta: {error}"))
                });
            }
            RecordKind::Commit => return Ok(None),
            _ => {}
        }
    }
}
