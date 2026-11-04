use std::io::Write;

use super::codec::{
    MAX_RECORD_PAYLOAD_BYTES, RECORD_HEADER_BYTES, RECORD_MAGIC, RECORD_VERSION, RecordKind,
};
use super::compact_codec::{
    MAGIC as CORE_MAGIC, MAX_SECTION_BYTES, MAX_SYMBOL_BYTES, STANDALONE_MAGIC, STANDALONE_VERSION,
    VERSION as CORE_VERSION, encode_boundary, write_len, write_varint,
};
use super::scratch::{ByteSpanSource, ScratchFile, SpoolSpan};
use super::transaction::PactTransaction;
use crate::error::{PactError, PactResult};
use crate::transaction_trace::stream_codec::TraceSpool;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpooledRecord {
    pub(crate) kind: RecordKind,
    pub(crate) generation: u64,
    pub(crate) flags: u16,
    pub(crate) payload: SpoolSpan,
    pub(crate) checksum: [u8; 8],
}

impl SpooledRecord {
    pub(crate) fn header(&self) -> [u8; RECORD_HEADER_BYTES] {
        let mut header = [0_u8; RECORD_HEADER_BYTES];
        header[..4].copy_from_slice(RECORD_MAGIC);
        header[4] = self.kind as u8;
        header[5] = RECORD_VERSION;
        header[6..8].copy_from_slice(&self.flags.to_le_bytes());
        header[8..16].copy_from_slice(&self.generation.to_le_bytes());
        header[16..24].copy_from_slice(&self.payload.len.to_le_bytes());
        header[24..32].copy_from_slice(&self.checksum);
        header
    }

    pub(crate) fn encoded_len(&self) -> u64 {
        RECORD_HEADER_BYTES as u64 + self.payload.len
    }
}

pub(crate) struct RecordSpool {
    spool: ScratchFile,
}

impl RecordSpool {
    pub(crate) fn create(io_chunk_bytes: usize) -> PactResult<Self> {
        Ok(Self {
            spool: ScratchFile::create(None, io_chunk_bytes.max(1))?,
        })
    }

    pub(crate) fn spool(&self) -> &ScratchFile {
        &self.spool
    }

    pub(crate) fn begin(
        &mut self,
        kind: RecordKind,
        generation: u64,
        flags: u16,
    ) -> RecordWriter<'_> {
        RecordWriter {
            spool: &mut self.spool,
            kind,
            generation,
            flags,
            start: None,
            hasher: blake3::Hasher::new(),
            len: 0,
            error: None,
        }
    }

    pub(crate) fn push(
        &mut self,
        kind: RecordKind,
        generation: u64,
        flags: u16,
        payload: &[u8],
    ) -> PactResult<SpooledRecord> {
        let mut writer = self.begin(kind, generation, flags);
        writer.write_all(payload)?;
        writer.finish()
    }

    pub(crate) fn emit(&self, record: &SpooledRecord, out: &mut dyn Write) -> PactResult<()> {
        out.write_all(&record.header())?;
        self.spool.copy_span(record.payload, out)
    }
}

pub(crate) struct RecordWriter<'a> {
    spool: &'a mut ScratchFile,
    kind: RecordKind,
    generation: u64,
    flags: u16,
    start: Option<u64>,
    hasher: blake3::Hasher,
    len: u64,
    error: Option<PactError>,
}

impl Write for RecordWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.start.is_none() {
            self.start = Some(self.spool.len());
        }
        match self.spool.append(buf) {
            Ok(_) => {
                self.hasher.update(buf);
                self.len = self.len.saturating_add(buf.len() as u64);
                Ok(buf.len())
            }
            Err(error) => {
                let message = error.to_string();
                self.error = Some(error);
                Err(std::io::Error::other(message))
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl RecordWriter<'_> {
    pub(crate) fn finish(self) -> PactResult<SpooledRecord> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.len > MAX_RECORD_PAYLOAD_BYTES {
            return Err(PactError::invalid(format!(
                "Pact record payload exceeds {MAX_RECORD_PAYLOAD_BYTES} bytes"
            )));
        }
        let mut checksum = [0_u8; 8];
        checksum.copy_from_slice(&self.hasher.finalize().as_bytes()[..8]);
        Ok(SpooledRecord {
            kind: self.kind,
            generation: self.generation,
            flags: self.flags,
            payload: SpoolSpan {
                offset: self.start.unwrap_or(self.spool.len()),
                len: self.len,
            },
            checksum,
        })
    }
}

pub(crate) fn spool_standalone_core(
    transaction: &PactTransaction,
    trace: &TraceSpool,
    records: &mut RecordSpool,
    generation: u64,
    io_chunk_bytes: usize,
) -> PactResult<SpooledRecord> {
    let trace_len = trace.check_limits()?;
    for symbol in [&transaction.transaction_id, &transaction.branch_id] {
        if symbol.len() > MAX_SYMBOL_BYTES {
            return Err(PactError::invalid(
                "compact standalone symbol exceeds fixed limit",
            ));
        }
    }

    let mut sections = ScratchFile::create(None, io_chunk_bytes.max(1))?;
    let raw_span = {
        let mut writer = std::io::BufWriter::with_capacity(
            io_chunk_bytes.max(1),
            SpoolAppend {
                spool: &mut sections,
            },
        );
        serde_json::to_writer(
            &mut writer,
            &super::compact_codec::BorrowedSections {
                public_trace: &transaction.public_trace,
                new_content: &transaction.new_content,
                head_checkpoint: &transaction.head_checkpoint,
                ledger_delta: &transaction.ledger_delta,
                head_metadata: &transaction.head_metadata,
            },
        )
        .map_err(|error| {
            PactError::codec(format!("encode compact transaction sections: {error}"))
        })?;
        writer.flush()?;
        drop(writer);
        SpoolSpan {
            offset: 0,
            len: sections.len(),
        }
    };
    if raw_span.len > MAX_SECTION_BYTES as u64 {
        return Err(PactError::invalid(
            "compact transaction sections exceed fixed limit",
        ));
    }
    let compressed_start = sections.len();

    sections.flush()?;
    let snapshot = sections_snapshot(&sections)?;
    {
        let mut encoder = zstd::stream::write::Encoder::new(
            SpoolAppend {
                spool: &mut sections,
            },
            7,
        )
        .map_err(|error| {
            PactError::codec(format!("compress compact transaction sections: {error}"))
        })?;
        let mut source = SpoolReader {
            spool: &snapshot,
            span: raw_span,
            position: 0,
        };
        std::io::copy(&mut source, &mut encoder)?;
        encoder.finish().map_err(|error| {
            PactError::codec(format!("compress compact transaction sections: {error}"))
        })?;
    }
    let compressed_len = sections.len() - compressed_start;
    let compressed_span = SpoolSpan {
        offset: compressed_start,
        len: compressed_len,
    };
    let (section_flags, stored) = if compressed_span.len < raw_span.len {
        (1_u8, compressed_span)
    } else {
        (0_u8, raw_span)
    };

    let mut writer = records.begin(RecordKind::CompactTransaction, generation, 0);
    let mut prefix = Vec::new();
    prefix.extend_from_slice(STANDALONE_MAGIC);
    prefix.push(STANDALONE_VERSION);
    write_len(&mut prefix, 2)?;
    for symbol in [&transaction.transaction_id, &transaction.branch_id] {
        write_len(&mut prefix, symbol.len())?;
        prefix.extend_from_slice(symbol.as_bytes());
    }
    let mut core_header = Vec::new();
    core_header.extend_from_slice(CORE_MAGIC);
    core_header.push(CORE_VERSION);
    write_varint(&mut core_header, 0);
    write_varint(&mut core_header, 1);
    core_header.push(encode_boundary(transaction.boundary));
    write_len(&mut core_header, trace_len as usize)?;
    let mut core_tail = vec![section_flags];
    write_len(&mut core_tail, raw_span.len as usize)?;
    write_len(&mut core_tail, stored.len as usize)?;
    let core_len = core_header.len() as u64 + trace_len + core_tail.len() as u64 + stored.len;
    if core_len > MAX_SECTION_BYTES as u64 {
        return Err(PactError::invalid(
            "compact transaction exceeds fixed limit",
        ));
    }
    write_len(&mut prefix, core_len as usize)?;
    writer.write_all(&prefix)?;
    writer.write_all(&core_header)?;
    trace.emit(&mut writer)?;
    writer.write_all(&core_tail)?;
    sections.copy_span(stored, &mut writer)?;
    writer.finish()
}

fn sections_snapshot(spool: &ScratchFile) -> PactResult<std::fs::File> {
    let path = spool
        .path()
        .ok_or_else(|| PactError::internal("Pact section spool has no path"))?;
    Ok(std::fs::File::open(path)?)
}

struct SpoolReader<'a> {
    spool: &'a std::fs::File,
    span: SpoolSpan,
    position: u64,
}

impl std::io::Read for SpoolReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let remaining = self.span.len.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(0);
        }
        let take = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = self
            .spool
            .read_at(&mut buf[..take], self.span.offset + self.position)?;
        self.position += read as u64;
        Ok(read)
    }
}

struct SpoolAppend<'a> {
    spool: &'a mut ScratchFile,
}

impl Write for SpoolAppend<'_> {
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
