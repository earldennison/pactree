use std::collections::HashMap;
use std::io::Write;

use serde_json::Value;

use super::encode::{FORMAT_VERSION, MAGIC, WELL_KNOWN, content_flags, insert_flags, opcode};
use super::{TraceOp, TraceOpKind};
use crate::error::{PactError, PactResult};
use crate::pact_file::btree::CowBTree;
use crate::pact_file::scratch::{ByteSpanSource, ScratchFile, ScratchPages, SpanTable, SpoolSpan};

#[cfg(test)]
mod tests;

pub(crate) const MAX_TRACE_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct TraceSpool {
    pages: ScratchPages,
    body: ScratchFile,
    from_rev: u64,
    to_rev: u64,
    well_known: HashMap<&'static str, u64>,
    symbols: SpanTable,
    order: CowBTree,
    next_ordinal: u64,
    op_count: u64,
    symbol_bytes: u64,
}

impl TraceSpool {
    pub(crate) fn new(
        from_rev: u64,
        to_rev: u64,
        io_chunk_bytes: usize,
        scratch_cache_bytes: usize,
    ) -> PactResult<Self> {
        Ok(Self {
            pages: ScratchPages::create(None, io_chunk_bytes, scratch_cache_bytes)?,
            body: ScratchFile::create(None, io_chunk_bytes.max(1))?,
            from_rev,
            to_rev,
            well_known: WELL_KNOWN
                .iter()
                .enumerate()
                .map(|(index, symbol)| (*symbol, index as u64))
                .collect(),
            symbols: SpanTable::new(),
            order: CowBTree::from_root(None),
            next_ordinal: WELL_KNOWN.len() as u64,
            op_count: 0,
            symbol_bytes: 0,
        })
    }

    pub(crate) fn op_count(&self) -> u64 {
        self.op_count
    }

    pub(crate) fn symbol_count(&self) -> u64 {
        self.next_ordinal
    }

    pub(crate) fn scratch_bytes(&self) -> u64 {
        self.pages.scratch_bytes() + self.body.len()
    }

    fn record_ordinal(&mut self, span: SpoolSpan) -> PactResult<u64> {
        let ordinal = self.next_ordinal;
        let mut value = Vec::with_capacity(16);
        value.extend_from_slice(&span.offset.to_le_bytes());
        value.extend_from_slice(&span.len.to_le_bytes());
        let (order, _) = std::mem::replace(&mut self.order, CowBTree::from_root(None)).insert(
            &mut self.pages,
            ordinal.to_be_bytes().to_vec(),
            value,
        )?;
        self.order = order;
        self.next_ordinal += 1;
        self.symbol_bytes = self.symbol_bytes.saturating_add(span.len);
        Ok(ordinal)
    }

    pub(crate) fn intern(&mut self, text: &str) -> PactResult<u64> {
        if let Some(index) = self.well_known.get(text) {
            return Ok(*index);
        }
        if let Some(existing) = self.symbols.lookup(&self.pages, text.as_bytes())? {
            return Ok(existing);
        }
        let ordinal = self.next_ordinal;
        let span = self.pages.spool_mut().append(text.as_bytes())?;
        self.symbols.insert_span(&mut self.pages, span, ordinal)?;
        self.record_ordinal(span)
    }

    pub(crate) fn intern_value(&mut self, value: &Value) -> PactResult<u64> {
        let start = self.pages.spool().len();
        {
            let chunk = self.pages.spool().chunk_bytes();
            let mut writer = std::io::BufWriter::with_capacity(
                chunk,
                SpoolWrite {
                    spool: self.pages.spool_mut(),
                },
            );
            match serde_json::to_writer(&mut writer, value) {
                Ok(()) => {}
                Err(_) => {
                    writer.write_all(b"null")?;
                }
            }
            writer.flush()?;
        }
        let end = self.pages.spool().len();
        let span = SpoolSpan {
            offset: start,
            len: end - start,
        };
        if let Some(text) = self.well_known_for_span(span)? {
            return Ok(text);
        }
        let ordinal = self.next_ordinal;
        let applied = self.symbols.insert_span(&mut self.pages, span, ordinal)?;
        if applied != ordinal {
            return Ok(applied);
        }
        self.record_ordinal(span)
    }

    fn well_known_for_span(&self, span: SpoolSpan) -> PactResult<Option<u64>> {
        if span.len > 16 {
            return Ok(None);
        }
        let bytes = self.pages.spool().read_span(span)?;
        Ok(std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| self.well_known.get(text).copied()))
    }

    fn write_varint(&mut self, value: u64) -> PactResult<()> {
        let mut buffer = Vec::with_capacity(10);
        super::encode::write_varint(&mut buffer, value);
        self.body.append(&buffer)?;
        Ok(())
    }

    pub(crate) fn push_op(&mut self, op: &TraceOp) -> PactResult<()> {
        let node = self.intern(&op.component_id)?;
        match &op.kind {
            TraceOpKind::InsertComponent {
                parent_id,
                index,
                component,
            } => {
                let parent = self.intern(parent_id)?;
                self.body.append(&[opcode::INSERT_COMPONENT])?;
                self.write_varint(node)?;
                self.write_varint(parent)?;
                self.write_varint(*index as u64)?;
                let mut fields = Vec::new();
                let mut attrs = Vec::new();
                let mut flags = 0_u8;
                if let Some(object) = component.as_object() {
                    if object.contains_key("children") {
                        flags |= insert_flags::HAS_CHILDREN;
                    }
                    for (key, value) in object {
                        match key.as_str() {
                            "id" | "children" => {}
                            "attrs" => {
                                if let Some(attr_object) = value.as_object() {
                                    for (attr_key, attr_value) in attr_object {
                                        let key = self.intern(attr_key)?;
                                        let value = self.intern_value(attr_value)?;
                                        attrs.push((key, value));
                                    }
                                } else {
                                    let key = self.intern(key)?;
                                    let value = self.intern_value(value)?;
                                    fields.push((key, value));
                                }
                            }
                            _ => {
                                let key = self.intern(key)?;
                                let value = self.intern_value(value)?;
                                fields.push((key, value));
                            }
                        }
                    }
                }
                self.body.append(&[flags])?;
                self.write_varint(fields.len() as u64)?;
                for (key, value) in fields {
                    self.write_varint(key)?;
                    self.write_varint(value)?;
                }
                self.write_varint(attrs.len() as u64)?;
                for (key, value) in attrs {
                    self.write_varint(key)?;
                    self.write_varint(value)?;
                }
            }
            TraceOpKind::DeleteComponent => {
                self.body.append(&[opcode::DELETE_COMPONENT])?;
                self.write_varint(node)?;
            }
            TraceOpKind::MoveComponent { parent_id, index } => {
                let parent = self.intern(parent_id)?;
                self.body.append(&[opcode::MOVE_COMPONENT])?;
                self.write_varint(node)?;
                self.write_varint(parent)?;
                self.write_varint(*index as u64)?;
            }
            TraceOpKind::SetField { key, value } => {
                let key = self.intern(key)?;
                let value = self.intern_value(value)?;
                self.body.append(&[opcode::SET_FIELD])?;
                self.write_varint(node)?;
                self.write_varint(key)?;
                self.write_varint(value)?;
            }
            TraceOpKind::UnsetField { key } => {
                let key = self.intern(key)?;
                self.body.append(&[opcode::UNSET_FIELD])?;
                self.write_varint(node)?;
                self.write_varint(key)?;
            }
            TraceOpKind::SetAttr { key, value } => {
                let key = self.intern(key)?;
                let value = self.intern_value(value)?;
                self.body.append(&[opcode::SET_ATTR])?;
                self.write_varint(node)?;
                self.write_varint(key)?;
                self.write_varint(value)?;
            }
            TraceOpKind::UnsetAttr { key } => {
                let key = self.intern(key)?;
                self.body.append(&[opcode::UNSET_ATTR])?;
                self.write_varint(node)?;
                self.write_varint(key)?;
            }
            TraceOpKind::SetContent {
                content,
                content_ref,
                content_preview,
            } => {
                let mut flags = 0_u8;
                if content.is_some() {
                    flags |= content_flags::CONTENT;
                }
                if content_ref.is_some() {
                    flags |= content_flags::CONTENT_REF;
                }
                if content_preview.is_some() {
                    flags |= content_flags::PREVIEW;
                }
                self.body.append(&[opcode::SET_CONTENT])?;
                self.write_varint(node)?;
                self.body.append(&[flags])?;
                if let Some(text) = content {
                    let symbol = self.intern(text)?;
                    self.write_varint(symbol)?;
                }
                if let Some(value) = content_ref {
                    let symbol = self.intern_value(value)?;
                    self.write_varint(symbol)?;
                }
                if let Some(text) = content_preview {
                    let symbol = self.intern(text)?;
                    self.write_varint(symbol)?;
                }
            }
            TraceOpKind::ReplaceContentRange {
                start,
                end,
                replacement,
            } => {
                let symbol = self.intern(replacement)?;
                self.body.append(&[opcode::REPLACE_CONTENT_RANGE])?;
                self.write_varint(node)?;
                self.write_varint(*start as u64)?;
                self.write_varint(*end as u64)?;
                self.write_varint(symbol)?;
            }
            TraceOpKind::SetLifecycle { value } => {
                let symbol = self.intern_value(value)?;
                self.body.append(&[opcode::SET_LIFECYCLE])?;
                self.write_varint(node)?;
                self.write_varint(symbol)?;
            }
        }
        self.op_count += 1;
        Ok(())
    }

    fn symbol_span(&self, ordinal: u64) -> PactResult<SpoolSpan> {
        let value = self
            .order
            .get(&self.pages, &ordinal.to_be_bytes())?
            .ok_or_else(|| {
                PactError::internal(format!("Pact trace spool lost symbol {ordinal}"))
            })?;
        let mut raw = [0_u8; 8];
        raw.copy_from_slice(&value[..8]);
        let offset = u64::from_le_bytes(raw);
        raw.copy_from_slice(&value[8..]);
        Ok(SpoolSpan {
            offset,
            len: u64::from_le_bytes(raw),
        })
    }

    fn varint_len(value: u64) -> u64 {
        let mut buffer = Vec::with_capacity(10);
        super::encode::write_varint(&mut buffer, value);
        buffer.len() as u64
    }

    pub(crate) fn encoded_len(&self) -> PactResult<u64> {
        let session = self.next_ordinal - WELL_KNOWN.len() as u64;
        let mut total = MAGIC.len() as u64 + 1;
        total += Self::varint_len(self.from_rev) + Self::varint_len(self.to_rev);
        total += Self::varint_len(session);
        for ordinal in WELL_KNOWN.len() as u64..self.next_ordinal {
            let span = self.symbol_span(ordinal)?;
            total = total
                .checked_add(Self::varint_len(span.len))
                .and_then(|total| total.checked_add(span.len))
                .ok_or_else(|| PactError::invalid("Pact trace spool length overflow"))?;
        }
        total = total
            .checked_add(Self::varint_len(self.op_count))
            .and_then(|total| total.checked_add(self.body.len()))
            .ok_or_else(|| PactError::invalid("Pact trace spool length overflow"))?;
        Ok(total)
    }

    pub(crate) fn check_limits(&self) -> PactResult<u64> {
        let len = self.encoded_len()?;
        if len > MAX_TRACE_BYTES {
            return Err(PactError::invalid(format!(
                "compact transaction trace exceeds fixed limit ({len} > {MAX_TRACE_BYTES} bytes)"
            )));
        }
        Ok(len)
    }

    pub(crate) fn emit(&self, out: &mut dyn Write) -> PactResult<u64> {
        let len = self.check_limits()?;
        let mut header = Vec::with_capacity(32);
        header.extend_from_slice(MAGIC);
        header.push(FORMAT_VERSION);
        super::encode::write_varint(&mut header, self.from_rev);
        super::encode::write_varint(&mut header, self.to_rev);
        super::encode::write_varint(&mut header, self.next_ordinal - WELL_KNOWN.len() as u64);
        out.write_all(&header)?;
        for ordinal in WELL_KNOWN.len() as u64..self.next_ordinal {
            let span = self.symbol_span(ordinal)?;
            let mut prefix = Vec::with_capacity(10);
            super::encode::write_varint(&mut prefix, span.len);
            out.write_all(&prefix)?;
            self.pages.spool().copy_span(span, out)?;
        }
        let mut count = Vec::with_capacity(10);
        super::encode::write_varint(&mut count, self.op_count);
        out.write_all(&count)?;
        self.body.copy_span(
            SpoolSpan {
                offset: 0,
                len: self.body.len(),
            },
            out,
        )?;
        Ok(len)
    }
}

struct SpoolWrite<'a> {
    spool: &'a mut ScratchFile,
}

impl Write for SpoolWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.spool
            .append(buf)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
