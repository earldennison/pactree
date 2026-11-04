use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::btree::{CowBTree, PAGE_BYTES, PageReader, PageWriter};
use super::byte_lru::ByteLru;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpoolSpan {
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

impl SpoolSpan {
    pub(crate) fn end(self) -> PactResult<u64> {
        self.offset
            .checked_add(self.len)
            .ok_or_else(|| PactError::invalid("Pact scratch span end overflow"))
    }
}

pub(crate) trait ByteSpanSource {
    fn copy_span(&self, span: SpoolSpan, out: &mut dyn Write) -> PactResult<()>;
}

pub(crate) trait ScratchBackend: Send + Sync {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()>;
}

struct TempFileBackend {
    file: tempfile::NamedTempFile,
}

impl ScratchBackend for TempFileBackend {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.as_file_mut().write_all(bytes)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        self.file.as_file().read_exact_at(buf, offset)
    }
}

pub(crate) struct ScratchFile {
    backend: Box<dyn ScratchBackend>,
    path: Option<PathBuf>,

    flushed: u64,

    pending: Vec<u8>,

    window: Mutex<Option<(u64, Vec<u8>)>>,
    len: u64,
    chunk: usize,
}

impl ScratchFile {
    pub(crate) fn create(dir: Option<&Path>, io_chunk_bytes: usize) -> PactResult<Self> {
        if io_chunk_bytes == 0 {
            return Err(PactError::invalid(
                "Pact scratch chunk size must be positive",
            ));
        }
        let mut builder = tempfile::Builder::new();
        builder.prefix(".pact-scratch-").suffix(".tmp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o600));
        }
        let file = match dir {
            Some(dir) => builder.tempfile_in(dir)?,
            None => builder.tempfile()?,
        };
        let path = file.path().to_path_buf();
        Ok(Self {
            backend: Box::new(TempFileBackend { file }),
            path: Some(path),
            flushed: 0,
            pending: Vec::new(),
            window: Mutex::new(None),
            len: 0,
            chunk: io_chunk_bytes,
        })
    }

    pub(crate) fn with_backend(backend: Box<dyn ScratchBackend>, io_chunk_bytes: usize) -> Self {
        Self {
            backend,
            path: None,
            flushed: 0,
            pending: Vec::new(),
            window: Mutex::new(None),
            len: 0,
            chunk: io_chunk_bytes.max(1),
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    pub(crate) fn chunk_bytes(&self) -> usize {
        self.chunk
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) -> PactResult<SpoolSpan> {
        let span = SpoolSpan {
            offset: self.len,
            len: u64::try_from(bytes.len())
                .map_err(|_| PactError::invalid("Pact scratch append exceeds u64"))?,
        };
        let end = span.end()?;
        if self.pending.len() + bytes.len() > self.chunk {
            self.flush()?;
        }
        if bytes.len() >= self.chunk {
            self.backend.append(bytes)?;
            self.flushed = end;
        } else {
            self.pending.extend_from_slice(bytes);
        }
        self.len = end;
        Ok(span)
    }

    pub(crate) fn flush(&mut self) -> PactResult<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.backend.append(&self.pending)?;
        self.flushed += self.pending.len() as u64;
        self.pending.clear();
        Ok(())
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> PactResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(|| PactError::invalid("Pact scratch read end overflow"))?;
        if end > self.len {
            return Err(PactError::corrupt("Pact scratch read is out of range"));
        }
        if offset >= self.flushed {
            let start = (offset - self.flushed) as usize;
            buf.copy_from_slice(&self.pending[start..start + buf.len()]);
            return Ok(());
        }
        if end > self.flushed {
            let split = (self.flushed - offset) as usize;
            let tail = buf.len() - split;
            self.read_flushed(&mut buf[..split], offset)?;
            buf[split..].copy_from_slice(&self.pending[..tail]);
            return Ok(());
        }
        self.read_flushed(buf, offset)
    }

    fn read_flushed(&self, buf: &mut [u8], offset: u64) -> PactResult<()> {
        if buf.len() >= self.chunk {
            self.backend.read_at(buf, offset)?;
            return Ok(());
        }
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let end = offset + buf.len() as u64;
        if let Some((start, bytes)) = window.as_ref()
            && offset >= *start
            && end <= *start + bytes.len() as u64
        {
            let from = (offset - *start) as usize;
            buf.copy_from_slice(&bytes[from..from + buf.len()]);
            return Ok(());
        }
        let start = offset;
        let take = usize::try_from((self.flushed - start).min(self.chunk as u64))
            .map_err(|_| PactError::invalid("Pact scratch chunk exceeds platform limits"))?;
        let mut bytes = vec![0_u8; take];
        self.backend.read_at(&mut bytes, start)?;
        buf.copy_from_slice(&bytes[..buf.len()]);
        *window = Some((start, bytes));
        Ok(())
    }

    pub(crate) fn append_from_reader(
        &mut self,
        reader: &mut dyn Read,
        limit: u64,
    ) -> PactResult<SpoolSpan> {
        let offset = self.len;
        let mut buffer = vec![0_u8; self.chunk];
        let mut total = 0_u64;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| PactError::invalid("Pact scratch stream length overflow"))?;
            if total > limit {
                return Err(PactError::invalid(format!(
                    "Pact scratch stream exceeds its {limit}-byte limit"
                )));
            }
            self.append(&buffer[..read])?;
        }
        Ok(SpoolSpan { offset, len: total })
    }

    fn check_span(&self, span: SpoolSpan) -> PactResult<()> {
        if span.end()? > self.len {
            return Err(PactError::corrupt("Pact scratch span is out of range"));
        }
        Ok(())
    }

    pub(crate) fn read_span(&self, span: SpoolSpan) -> PactResult<Vec<u8>> {
        self.check_span(span)?;
        let len = usize::try_from(span.len)
            .map_err(|_| PactError::invalid("Pact scratch span exceeds platform limits"))?;
        let mut bytes = vec![0_u8; len];
        if len > 0 {
            self.read_at(&mut bytes, span.offset)?;
        }
        Ok(bytes)
    }

    pub(crate) fn span_eq(&self, span: SpoolSpan, bytes: &[u8]) -> PactResult<bool> {
        self.check_span(span)?;
        if span.len != bytes.len() as u64 {
            return Ok(false);
        }
        let mut buffer = vec![0_u8; self.chunk];
        let mut offset = span.offset;
        let mut compared = 0_usize;
        while compared < bytes.len() {
            let take = (bytes.len() - compared).min(self.chunk);
            self.read_at(&mut buffer[..take], offset)?;
            if buffer[..take] != bytes[compared..compared + take] {
                return Ok(false);
            }
            compared += take;
            offset += take as u64;
        }
        Ok(true)
    }
}

impl ScratchFile {
    pub(crate) fn hash_span(&self, span: SpoolSpan) -> PactResult<[u8; 16]> {
        self.check_span(span)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; self.chunk];
        let mut remaining = span.len;
        let mut offset = span.offset;
        while remaining > 0 {
            let take = usize::try_from(remaining.min(self.chunk as u64))
                .map_err(|_| PactError::invalid("Pact scratch chunk exceeds platform limits"))?;
            self.read_at(&mut buffer[..take], offset)?;
            hasher.update(&buffer[..take]);
            remaining -= take as u64;
            offset += take as u64;
        }
        let mut prefix = [0_u8; 16];
        prefix.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        Ok(prefix)
    }

    pub(crate) fn span_eq_span(&self, left: SpoolSpan, right: SpoolSpan) -> PactResult<bool> {
        self.check_span(left)?;
        self.check_span(right)?;
        if left.len != right.len {
            return Ok(false);
        }
        let mut a = vec![0_u8; self.chunk];
        let mut b = vec![0_u8; self.chunk];
        let mut remaining = left.len;
        let (mut la, mut lb) = (left.offset, right.offset);
        while remaining > 0 {
            let take = usize::try_from(remaining.min(self.chunk as u64))
                .map_err(|_| PactError::invalid("Pact scratch chunk exceeds platform limits"))?;
            self.read_at(&mut a[..take], la)?;
            self.read_at(&mut b[..take], lb)?;
            if a[..take] != b[..take] {
                return Ok(false);
            }
            remaining -= take as u64;
            la += take as u64;
            lb += take as u64;
        }
        Ok(true)
    }
}

impl ByteSpanSource for ScratchFile {
    fn copy_span(&self, span: SpoolSpan, out: &mut dyn Write) -> PactResult<()> {
        self.check_span(span)?;
        let mut buffer = vec![0_u8; self.chunk];
        let mut remaining = span.len;
        let mut offset = span.offset;
        while remaining > 0 {
            let take = usize::try_from(remaining.min(self.chunk as u64))
                .map_err(|_| PactError::invalid("Pact scratch chunk exceeds platform limits"))?;
            self.read_at(&mut buffer[..take], offset)?;
            out.write_all(&buffer[..take])?;
            remaining -= take as u64;
            offset += take as u64;
        }
        Ok(())
    }
}

pub(crate) struct ScratchPages {
    spool: ScratchFile,

    cache: Mutex<ByteLru<u64, Box<[u8; PAGE_BYTES]>>>,
    pages_written: u64,
}

impl ScratchPages {
    pub(crate) fn create(
        dir: Option<&Path>,
        io_chunk_bytes: usize,
        cache_bytes: usize,
    ) -> PactResult<Self> {
        let mut spool = ScratchFile::create(dir, io_chunk_bytes.max(PAGE_BYTES))?;
        spool.append(&[0_u8; PAGE_BYTES])?;
        Ok(Self {
            spool,
            cache: Mutex::new(ByteLru::new(cache_bytes)),
            pages_written: 0,
        })
    }

    pub(crate) fn pages_written(&self) -> u64 {
        self.pages_written
    }

    pub(crate) fn scratch_bytes(&self) -> u64 {
        self.spool.len()
    }

    pub(crate) fn spool(&self) -> &ScratchFile {
        &self.spool
    }

    pub(crate) fn spool_mut(&mut self) -> &mut ScratchFile {
        &mut self.spool
    }

    pub(crate) fn cache_resident_bytes(&self) -> usize {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resident_bytes()
    }
}

impl PageReader for ScratchPages {
    fn read_page(&self, address: u64) -> PactResult<[u8; PAGE_BYTES]> {
        if let Some(page) = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&address)
        {
            return Ok(**page);
        }
        let span = SpoolSpan {
            offset: address,
            len: PAGE_BYTES as u64,
        };
        let mut page = [0_u8; PAGE_BYTES];
        self.spool.read_at(&mut page, span.offset)?;
        let _ = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(address, Box::new(page), PAGE_BYTES);
        Ok(page)
    }
}

impl ScratchPages {
    pub(crate) fn read_page_cached(&mut self, address: u64) -> PactResult<[u8; PAGE_BYTES]> {
        self.read_page(address)
    }
}

impl PageWriter for ScratchPages {
    fn write_page(&mut self, page: [u8; PAGE_BYTES]) -> PactResult<u64> {
        let span = self.spool.append(&page)?;
        self.pages_written += 1;
        let _ = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(span.offset, Box::new(page), PAGE_BYTES);
        Ok(span.offset)
    }
}

pub(crate) struct SpanTable {
    tree: CowBTree,
    hasher: fn(&[u8]) -> [u8; 16],
}

fn blake3_prefix(bytes: &[u8]) -> [u8; 16] {
    let mut prefix = [0_u8; 16];
    prefix.copy_from_slice(&blake3::hash(bytes).as_bytes()[..16]);
    prefix
}

impl SpanTable {
    pub(crate) fn new() -> Self {
        Self {
            tree: CowBTree::from_root(None),
            hasher: blake3_prefix,
        }
    }

    pub(crate) fn with_hasher(hasher: fn(&[u8]) -> [u8; 16]) -> Self {
        Self {
            tree: CowBTree::from_root(None),
            hasher,
        }
    }

    fn bucket_bounds(&self, bytes: &[u8]) -> ([u8; 20], [u8; 20]) {
        let hash = (self.hasher)(bytes);
        let mut start = [0_u8; 20];
        start[..16].copy_from_slice(&hash);
        let mut end = start;
        end[16..].copy_from_slice(&u32::MAX.to_be_bytes());
        (start, end)
    }

    fn decode_value(value: &[u8]) -> PactResult<(SpoolSpan, u64)> {
        if value.len() != 24 {
            return Err(PactError::corrupt(
                "Pact span table value has invalid width",
            ));
        }
        let word = |at: usize| -> u64 {
            let mut raw = [0_u8; 8];
            raw.copy_from_slice(&value[at..at + 8]);
            u64::from_le_bytes(raw)
        };
        Ok((
            SpoolSpan {
                offset: word(0),
                len: word(8),
            },
            word(16),
        ))
    }

    pub(crate) fn lookup(&self, pages: &ScratchPages, bytes: &[u8]) -> PactResult<Option<u64>> {
        let (start, end) = self.bucket_bounds(bytes);
        for (_, value) in self.tree.range(pages, &start, &end)? {
            let (span, ordinal) = Self::decode_value(&value)?;
            if pages.spool().span_eq(span, bytes)? {
                return Ok(Some(ordinal));
            }
        }
        Ok(None)
    }

    pub(crate) fn lookup_span(
        &self,
        pages: &ScratchPages,
        span: SpoolSpan,
    ) -> PactResult<Option<u64>> {
        let hash = pages.spool().hash_span(span)?;
        let (start, end) = Self::bucket_bounds_for_hash(hash);
        for (_, value) in self.tree.range(pages, &start, &end)? {
            let (candidate, ordinal) = Self::decode_value(&value)?;
            if pages.spool().span_eq_span(candidate, span)? {
                return Ok(Some(ordinal));
            }
        }
        Ok(None)
    }

    pub(crate) fn insert_span(
        &mut self,
        pages: &mut ScratchPages,
        span: SpoolSpan,
        ordinal: u64,
    ) -> PactResult<u64> {
        if let Some(existing) = self.lookup_span(pages, span)? {
            return Ok(existing);
        }
        let hash = pages.spool().hash_span(span)?;
        let (start, end) = Self::bucket_bounds_for_hash(hash);
        let seq = u32::try_from(self.tree.range(pages, &start, &end)?.len())
            .map_err(|_| PactError::invalid("Pact span table bucket overflow"))?;
        self.insert_entry(pages, start, seq, span, ordinal)
    }

    fn bucket_bounds_for_hash(hash: [u8; 16]) -> ([u8; 20], [u8; 20]) {
        let mut start = [0_u8; 20];
        start[..16].copy_from_slice(&hash);
        let mut end = start;
        end[16..].copy_from_slice(&u32::MAX.to_be_bytes());
        (start, end)
    }

    fn insert_entry(
        &mut self,
        pages: &mut ScratchPages,
        start: [u8; 20],
        seq: u32,
        span: SpoolSpan,
        ordinal: u64,
    ) -> PactResult<u64> {
        let mut key = start.to_vec();
        key[16..].copy_from_slice(&seq.to_be_bytes());
        let mut value = Vec::with_capacity(24);
        value.extend_from_slice(&span.offset.to_le_bytes());
        value.extend_from_slice(&span.len.to_le_bytes());
        value.extend_from_slice(&ordinal.to_le_bytes());
        let (tree, _) = std::mem::replace(&mut self.tree, CowBTree::from_root(None))
            .insert(pages, key, value)?;
        self.tree = tree;
        Ok(ordinal)
    }

    pub(crate) fn insert(
        &mut self,
        pages: &mut ScratchPages,
        bytes: &[u8],
        ordinal: u64,
    ) -> PactResult<u64> {
        if let Some(existing) = self.lookup(pages, bytes)? {
            return Ok(existing);
        }
        let (start, end) = self.bucket_bounds(bytes);
        let seq = u32::try_from(self.tree.range(pages, &start, &end)?.len())
            .map_err(|_| PactError::invalid("Pact span table bucket overflow"))?;
        let span = pages.spool_mut().append(bytes)?;
        self.insert_entry(pages, start, seq, span, ordinal)
    }
}

pub(crate) struct ScratchRange {
    root: Option<u64>,
    next_start: Option<Vec<u8>>,
    end: Vec<u8>,
    page_entries: usize,
}

impl ScratchRange {
    pub(crate) fn new(tree: &CowBTree, start: &[u8], end: &[u8], page_entries: usize) -> Self {
        Self {
            root: tree.root_address(),
            next_start: Some(start.to_vec()),
            end: end.to_vec(),
            page_entries: page_entries.max(1),
        }
    }

    pub(crate) fn next_page(
        &mut self,
        pages: &impl PageReader,
    ) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let Some(start) = self.next_start.take() else {
            return Ok(Vec::new());
        };
        if start >= self.end {
            return Ok(Vec::new());
        }
        let entries = CowBTree::from_root(self.root).range_limit(
            pages,
            &start,
            &self.end,
            self.page_entries,
            false,
        )?;
        if entries.len() == self.page_entries
            && let Some((last, _)) = entries.last()
        {
            let mut successor = last.clone();
            successor.push(0);
            self.next_start = Some(successor);
        }
        Ok(entries)
    }
}
