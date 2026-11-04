use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::transaction::{LedgerDelta, PactTransaction};
use crate::error::{PactError, PactResult};
use crate::store::{ContentRecord, PactTransactionTrace};
use crate::trace::BoundaryKind;

pub(crate) const MAGIC: &[u8; 4] = b"PCTX";
pub(crate) const VERSION: u8 = 1;
pub(crate) const STANDALONE_MAGIC: &[u8; 4] = b"PCCS";
pub(crate) const STANDALONE_VERSION: u8 = 1;
pub(crate) const MAX_SECTION_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_SYMBOLS: usize = 4096;
pub(crate) const MAX_SYMBOL_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SymbolTable {
    values: Vec<String>,
    ids: BTreeMap<String, u32>,
}

impl SymbolTable {
    fn intern(&mut self, value: &str) -> PactResult<u32> {
        if let Some(id) = self.ids.get(value) {
            return Ok(*id);
        }
        let id = u32::try_from(self.values.len())
            .map_err(|_| PactError::invalid("compact symbol table exceeds u32 entries"))?;
        self.values.push(value.to_string());
        self.ids.insert(value.to_string(), id);
        Ok(id)
    }

    fn resolve(&self, id: u64) -> PactResult<String> {
        let index = usize::try_from(id)
            .map_err(|_| PactError::corrupt("compact symbol id exceeds platform limits"))?;
        self.values
            .get(index)
            .cloned()
            .ok_or_else(|| PactError::corrupt("compact transaction references unknown symbol"))
    }
}

pub(crate) fn encode_standalone_core(transaction: &PactTransaction) -> PactResult<Vec<u8>> {
    let mut core = transaction.clone();
    core.new_content.clear();
    core.head_metadata = None;

    let mut symbols = SymbolTable::default();
    let transaction = encode_transaction(&core, &mut symbols)?;
    if symbols.values.len() > MAX_SYMBOLS {
        return Err(PactError::invalid(
            "compact standalone core exceeds symbol limit",
        ));
    }

    let mut out = Vec::with_capacity(transaction.len() + 64);
    out.extend_from_slice(STANDALONE_MAGIC);
    out.push(STANDALONE_VERSION);
    write_len(&mut out, symbols.values.len())?;
    for symbol in &symbols.values {
        if symbol.len() > MAX_SYMBOL_BYTES {
            return Err(PactError::invalid(
                "compact standalone symbol exceeds fixed limit",
            ));
        }
        write_len(&mut out, symbol.len())?;
        out.extend_from_slice(symbol.as_bytes());
    }
    write_len(&mut out, transaction.len())?;
    out.extend_from_slice(&transaction);
    Ok(out)
}

pub(crate) fn decode_standalone_core(bytes: &[u8]) -> PactResult<PactTransaction> {
    decode_standalone_core_with(bytes, None)
}

pub(crate) fn decode_standalone_core_with(
    bytes: &[u8],
    derived_branches: Option<&std::collections::BTreeSet<String>>,
) -> PactResult<PactTransaction> {
    let mut cursor = 0;
    if take(bytes, &mut cursor, STANDALONE_MAGIC.len())? != STANDALONE_MAGIC {
        return Err(PactError::corrupt("unknown compact standalone core magic"));
    }
    if read_u8(bytes, &mut cursor)? != STANDALONE_VERSION {
        return Err(PactError::corrupt(
            "unsupported compact standalone core version",
        ));
    }
    let symbol_count = read_len(bytes, &mut cursor, MAX_SYMBOLS)?;
    let mut symbols = SymbolTable::default();
    for _ in 0..symbol_count {
        let len = read_len(bytes, &mut cursor, MAX_SYMBOL_BYTES)?;
        let symbol = std::str::from_utf8(take(bytes, &mut cursor, len)?)
            .map_err(|_| PactError::corrupt("compact standalone symbol is not UTF-8"))?;
        if symbols.ids.contains_key(symbol) {
            return Err(PactError::corrupt(
                "compact standalone core has duplicate symbols",
            ));
        }
        symbols.intern(symbol)?;
    }
    let transaction_len = read_len(bytes, &mut cursor, MAX_SECTION_BYTES)?;
    let transaction = decode_transaction_with(
        take(bytes, &mut cursor, transaction_len)?,
        &symbols,
        derived_branches,
    )?;
    if cursor != bytes.len() {
        return Err(PactError::corrupt(
            "compact standalone core has trailing bytes",
        ));
    }
    Ok(transaction)
}

#[derive(Serialize)]
pub(crate) struct BorrowedSections<'a> {
    pub(crate) public_trace: &'a PactTransactionTrace,
    pub(crate) new_content: &'a [ContentRecord],
    pub(crate) head_checkpoint: &'a Option<serde_json::Value>,
    pub(crate) ledger_delta: &'a Option<LedgerDelta>,
    pub(crate) head_metadata: &'a Option<super::index::HeadMetadata>,
}

#[derive(Deserialize)]
struct OwnedSections {
    public_trace: PactTransactionTrace,
    new_content: Vec<ContentRecord>,
    head_checkpoint: Option<serde_json::Value>,
    ledger_delta: Option<LedgerDelta>,
    head_metadata: Option<super::index::HeadMetadata>,
}

pub(crate) fn encode_transaction(
    transaction: &PactTransaction,
    symbols: &mut SymbolTable,
) -> PactResult<Vec<u8>> {
    let transaction_id = symbols.intern(&transaction.transaction_id)?;
    let branch_id = symbols.intern(&transaction.branch_id)?;
    let trace = crate::transaction_trace::encode_transaction_trace(&transaction.trace);
    if trace.len() > MAX_TRACE_BYTES {
        return Err(PactError::invalid(
            "compact transaction trace exceeds fixed limit",
        ));
    }
    let sections = serde_json::to_vec(&BorrowedSections {
        public_trace: &transaction.public_trace,
        new_content: &transaction.new_content,
        head_checkpoint: &transaction.head_checkpoint,
        ledger_delta: &transaction.ledger_delta,
        head_metadata: &transaction.head_metadata,
    })
    .map_err(|error| PactError::codec(format!("encode compact transaction sections: {error}")))?;
    if sections.len() > MAX_SECTION_BYTES {
        return Err(PactError::invalid(
            "compact transaction sections exceed fixed limit",
        ));
    }
    let compressed = zstd::bulk::compress(&sections, 7).map_err(|error| {
        PactError::codec(format!("compress compact transaction sections: {error}"))
    })?;
    let (section_flags, stored_sections) = if compressed.len() < sections.len() {
        (1_u8, compressed.as_slice())
    } else {
        (0_u8, sections.as_slice())
    };

    let mut out = Vec::with_capacity(trace.len() + stored_sections.len() + 32);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    write_varint(&mut out, u64::from(transaction_id));
    write_varint(&mut out, u64::from(branch_id));
    out.push(encode_boundary(transaction.boundary));
    write_len(&mut out, trace.len())?;
    out.extend_from_slice(&trace);
    out.push(section_flags);
    write_len(&mut out, sections.len())?;
    write_len(&mut out, stored_sections.len())?;
    out.extend_from_slice(stored_sections);
    Ok(out)
}

pub(crate) fn decode_transaction(
    bytes: &[u8],
    symbols: &SymbolTable,
) -> PactResult<PactTransaction> {
    decode_transaction_with(bytes, symbols, None)
}

pub(crate) fn decode_transaction_with(
    bytes: &[u8],
    symbols: &SymbolTable,
    derived_branches: Option<&std::collections::BTreeSet<String>>,
) -> PactResult<PactTransaction> {
    let mut cursor = 0;
    if take(bytes, &mut cursor, MAGIC.len())? != MAGIC {
        return Err(PactError::corrupt("unknown compact transaction magic"));
    }
    if read_u8(bytes, &mut cursor)? != VERSION {
        return Err(PactError::corrupt(
            "unsupported compact transaction version",
        ));
    }
    let transaction_id = symbols.resolve(read_varint(bytes, &mut cursor)?)?;
    let branch_id = symbols.resolve(read_varint(bytes, &mut cursor)?)?;
    let boundary = decode_boundary(read_u8(bytes, &mut cursor)?)?;
    let trace_len = read_len(bytes, &mut cursor, MAX_TRACE_BYTES)?;
    let skip_derived_depth = derived_branches.is_some_and(|set| set.contains(&branch_id));
    let trace = crate::transaction_trace::decode_transaction_trace_with(
        take(bytes, &mut cursor, trace_len)?,
        skip_derived_depth,
    )?;
    let section_flags = read_u8(bytes, &mut cursor)?;
    if section_flags & !1 != 0 {
        return Err(PactError::corrupt(
            "compact transaction has unsupported section flags",
        ));
    }
    let raw_len = read_len(bytes, &mut cursor, MAX_SECTION_BYTES)?;
    let stored_len = read_len(bytes, &mut cursor, MAX_SECTION_BYTES)?;
    let stored = take(bytes, &mut cursor, stored_len)?;
    if cursor != bytes.len() {
        return Err(PactError::corrupt("compact transaction has trailing bytes"));
    }
    let sections = if section_flags == 1 {
        let decoded = zstd::bulk::decompress(stored, raw_len).map_err(|error| {
            PactError::corrupt(format!("invalid compact transaction sections: {error}"))
        })?;
        if decoded.len() != raw_len {
            return Err(PactError::corrupt(
                "compact transaction section length mismatch",
            ));
        }
        decoded
    } else {
        if stored_len != raw_len {
            return Err(PactError::corrupt(
                "raw compact transaction section length mismatch",
            ));
        }
        stored.to_vec()
    };
    let sections: OwnedSections = serde_json::from_slice(&sections).map_err(|error| {
        PactError::corrupt(format!("invalid compact transaction sections: {error}"))
    })?;
    Ok(PactTransaction {
        transaction_id,
        branch_id,
        boundary,
        trace,
        public_trace: sections.public_trace,
        new_content: sections.new_content,
        head_checkpoint: sections.head_checkpoint,
        ledger_delta: sections.ledger_delta,
        head_metadata: sections.head_metadata,
    })
}

pub(crate) fn encode_boundary(boundary: BoundaryKind) -> u8 {
    match boundary {
        BoundaryKind::Authored => 0,
        BoundaryKind::PreRequest => 1,
        BoundaryKind::PostRequestAlteration => 2,
        BoundaryKind::Request => 3,
        BoundaryKind::Response => 4,
        BoundaryKind::PostResponseAlteration => 5,
        BoundaryKind::PostResponse => 6,
        BoundaryKind::Adhoc => 7,
    }
}

fn decode_boundary(value: u8) -> PactResult<BoundaryKind> {
    match value {
        0 => Ok(BoundaryKind::Authored),
        1 => Ok(BoundaryKind::PreRequest),
        2 => Ok(BoundaryKind::PostRequestAlteration),
        3 => Ok(BoundaryKind::Request),
        4 => Ok(BoundaryKind::Response),
        5 => Ok(BoundaryKind::PostResponseAlteration),
        6 => Ok(BoundaryKind::PostResponse),
        7 => Ok(BoundaryKind::Adhoc),
        _ => Err(PactError::corrupt("unknown compact transaction boundary")),
    }
}

pub(crate) fn write_len(out: &mut Vec<u8>, value: usize) -> PactResult<()> {
    write_varint(
        out,
        u64::try_from(value).map_err(|_| PactError::invalid("compact length exceeds u64"))?,
    );
    Ok(())
}

pub(crate) fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

pub(crate) fn read_varint(bytes: &[u8], cursor: &mut usize) -> PactResult<u64> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = read_u8(bytes, cursor)?;
        if shift == 63 && byte & 0xfe != 0 {
            return Err(PactError::corrupt("compact varint overflow"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(PactError::corrupt("compact varint overrun"))
}

pub(crate) fn read_len(bytes: &[u8], cursor: &mut usize, limit: usize) -> PactResult<usize> {
    let value = usize::try_from(read_varint(bytes, cursor)?)
        .map_err(|_| PactError::corrupt("compact length exceeds platform limits"))?;
    if value > limit {
        return Err(PactError::corrupt("compact length exceeds fixed limit"));
    }
    Ok(value)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> PactResult<u8> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| PactError::corrupt("truncated compact transaction"))?;
    *cursor += 1;
    Ok(value)
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> PactResult<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| PactError::corrupt("compact transaction length overrun"))?;
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}

#[cfg(test)]
mod tests;
